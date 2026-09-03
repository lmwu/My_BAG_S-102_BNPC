use eframe::egui;
use gdal::Dataset;
use gdal::raster::RasterBand;
use gdal::spatial_ref::{CoordTransform, SpatialRef};
use rfd::FileDialog;

const BUNDLED_FONT: &[u8] = include_bytes!("../assets/FZYTK.ttf");

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 750.0])
            .with_min_inner_size([800.0, 500.0]),
        ..Default::default()
    };

    eframe::run_native(
        "NOAA VR BAG 轉 S-102 工具 (含空間座標與互動檢視器)",
        options,
        Box::new(|cc| {
            setup_custom_font(&cc.egui_ctx);
            Ok(Box::new(VrBagApp::default()))
        }),
    )
}

fn setup_custom_font(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "custom_font".to_owned(),
        egui::FontData::from_static(BUNDLED_FONT),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "custom_font".to_owned());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("custom_font".to_owned());
    ctx.set_fonts(fonts);

    // 🔍 調整整體 UI 字體大小
    ctx.style_mut(|style| {
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(16.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(15.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(22.0, egui::FontFamily::Proportional),
        );
    });
}

#[derive(PartialEq, Clone, Copy)]
enum ViewMode {
    Depth,
    Uncertainty,
}

struct PreviewData {
    pub depth_texture: Option<egui::TextureHandle>,
    pub uncertainty_texture: Option<egui::TextureHandle>,
    pub width: usize,
    pub height: usize,
    pub depth_grid: Vec<f32>,
    pub uncertainty_grid: Vec<f32>,
    pub geo_transform: [f64; 6],
    pub spatial_wkt: String,
    pub crs_name: String,
}

impl Default for PreviewData {
    fn default() -> Self {
        Self {
            depth_texture: None,
            uncertainty_texture: None,
            width: 0,
            height: 0,
            depth_grid: vec![],
            uncertainty_grid: vec![],
            geo_transform: [0.0; 6],
            spatial_wkt: String::new(),
            crs_name: String::new(),
        }
    }
}

struct VrBagApp {
    input_path: String,
    output_path: String,
    resolutions: Vec<&'static str>,
    selected_res_idx: usize,
    datums: Vec<&'static str>,
    selected_datum_idx: usize,
    auto_align_depth: bool,
    status_message: String,

    view_mode: ViewMode,
    preview_data: PreviewData,

    // 🔍 視角控制狀態：縮放倍率與平移偏移量
    zoom: f32,
    pan: egui::Vec2,
}

impl Default for VrBagApp {
    fn default() -> Self {
        Self {
            input_path: String::new(),
            output_path: String::new(),
            resolutions: vec!["自動 (取最細網格)", "0.5", "1.0", "2.0", "5.0"],
            selected_res_idx: 2,
            datums: vec![
                "12 - MLLW (最低平均低潮位)",
                "10 - LAT (最低天文潮位)",
                "1 - MSL (平均海平面)",
            ],
            selected_datum_idx: 0,
            auto_align_depth: true,
            status_message: "狀態: 請選擇 Real NOAA .bag 檔案開啟".to_string(),
            view_mode: ViewMode::Depth,
            preview_data: PreviewData::default(),
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
        }
    }
}

impl VrBagApp {
    fn load_real_bag(&mut self, ctx: &egui::Context, bag_path: &str) {
        match Dataset::open(bag_path) {
            Ok(dataset) => {
                let geo_transform = match dataset.geo_transform() {
                    Ok(gt) => gt,
                    Err(e) => {
                        self.status_message = format!("無法讀取 GeoTransform: {}", e);
                        return;
                    }
                };

                let (width, height) = dataset.raster_size();
                let spatial_wkt = dataset.projection();

                let crs_name = if !spatial_wkt.is_empty() {
                    if let Ok(srs) = SpatialRef::from_wkt(&spatial_wkt) {
                        srs.name().unwrap_or("未知投影座標系".to_string())
                    } else {
                        "無法解析 WKT 坐標系".to_string()
                    }
                } else {
                    "未設定投影坐標系".to_string()
                };

                let band_depth: RasterBand = match dataset.rasterband(1) {
                    Ok(b) => b,
                    Err(e) => {
                        self.status_message = format!("讀取水深波段失敗: {}", e);
                        return;
                    }
                };
                let band_unc: RasterBand = match dataset.rasterband(2) {
                    Ok(b) => b,
                    Err(e) => {
                        self.status_message = format!("讀取不確定性波段失敗: {}", e);
                        return;
                    }
                };

                let nodata_depth = band_depth.no_data_value().unwrap_or(-1000000.0) as f32;
                let nodata_unc = band_unc.no_data_value().unwrap_or(-1000000.0) as f32;

                let mut depth_buffer =
                    match band_depth.read_as::<f32>((0, 0), (width, height), (width, height), None)
                    {
                        Ok(b) => b.data().to_vec(),
                        Err(e) => {
                            self.status_message = format!("讀取水深數據失敗: {}", e);
                            return;
                        }
                    };
                let unc_buffer =
                    match band_unc.read_as::<f32>((0, 0), (width, height), (width, height), None) {
                        Ok(b) => b.data().to_vec(),
                        Err(e) => {
                            self.status_message = format!("讀取不確定性數據失敗: {}", e);
                            return;
                        }
                    };

                if self.auto_align_depth {
                    for val in depth_buffer.iter_mut() {
                        if *val != nodata_depth && !val.is_nan() {
                            *val = -(*val);
                        }
                    }
                }

                let valid_depths: Vec<f32> = depth_buffer
                    .iter()
                    .copied()
                    .filter(|&v| v != nodata_depth && !v.is_nan())
                    .collect();

                let (min_d, max_d) = if !valid_depths.is_empty() {
                    (
                        valid_depths.iter().copied().fold(f32::INFINITY, f32::min),
                        valid_depths
                            .iter()
                            .copied()
                            .fold(f32::NEG_INFINITY, f32::max),
                    )
                } else {
                    (0.0, 100.0)
                };

                let mut depth_pixels = Vec::with_capacity(width * height * 4);
                let mut unc_pixels = Vec::with_capacity(width * height * 4);

                for i in 0..(width * height) {
                    let d = depth_buffer[i];
                    let u = unc_buffer[i];

                    if d == nodata_depth || d.is_nan() {
                        depth_pixels.extend_from_slice(&[0, 0, 0, 0]);
                    } else {
                        let norm = ((d - min_d) / (max_d - min_d + 0.0001)).clamp(0.0, 1.0);
                        let r = ((1.0 - norm) * 255.0) as u8;
                        let g = ((1.0 - (norm - 0.5).abs() * 2.0) * 255.0).max(0.0) as u8;
                        let b = (norm * 255.0) as u8;
                        depth_pixels.extend_from_slice(&[r, g, b, 255]);
                    }

                    if u == nodata_unc || u.is_nan() {
                        unc_pixels.extend_from_slice(&[0, 0, 0, 0]);
                    } else {
                        let u_norm = (u / 2.0).clamp(0.0, 1.0);
                        let ur = (u_norm * 255.0) as u8;
                        let ug = ((1.0 - u_norm) * 255.0) as u8;
                        unc_pixels.extend_from_slice(&[ur, ug, 50, 255]);
                    }
                }

                let depth_img =
                    egui::ColorImage::from_rgba_unmultiplied([width, height], &depth_pixels);
                let unc_img =
                    egui::ColorImage::from_rgba_unmultiplied([width, height], &unc_pixels);

                let depth_tex =
                    ctx.load_texture("real_depth", depth_img, egui::TextureOptions::LINEAR);
                let unc_tex = ctx.load_texture("real_unc", unc_img, egui::TextureOptions::LINEAR);

                self.preview_data = PreviewData {
                    depth_texture: Some(depth_tex),
                    uncertainty_texture: Some(unc_tex),
                    width,
                    height,
                    depth_grid: depth_buffer,
                    uncertainty_grid: unc_buffer,
                    geo_transform,
                    spatial_wkt,
                    crs_name,
                };

                // 載入新檔案時重置縮放與平移
                self.zoom = 1.0;
                self.pan = egui::Vec2::ZERO;

                self.status_message = format!(
                    "成功解析 BAG！網格大小: {}x{}, 水深範圍: {:.1}m ~ {:.1}m",
                    width, height, min_d, max_d
                );
            }
            Err(e) => {
                self.status_message = format!("開啟 BAG 檔案失敗: {}", e);
            }
        }
    }
}

impl eframe::App for VrBagApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::SidePanel::left("control_panel")
            .resizable(true)
            .default_width(340.0)
            .show(ctx, |ui| {
                ui.add_space(5.0);
                ui.heading("NOAA VR BAG 轉檔工具");
                ui.separator();

                ui.group(|ui| {
                    ui.label(egui::RichText::new("📂 檔案路徑").strong());
                    ui.add_space(3.0);

                    ui.label("來源 VR BAG 檔案:");
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.input_path);
                        if ui.button("瀏覽").clicked() {
                            if let Some(path) =
                                FileDialog::new().add_filter("BAG", &["bag"]).pick_file()
                            {
                                let path_str = path.display().to_string();
                                self.input_path = path_str.clone();
                                self.load_real_bag(ctx, &path_str);
                            }
                        }
                    });

                    ui.label("輸出 S-102 檔案:");
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.output_path);
                        if ui.button("另存").clicked() {
                            if let Some(path) =
                                FileDialog::new().add_filter("HDF5", &["h5"]).save_file()
                            {
                                self.output_path = path.display().to_string();
                            }
                        }
                    });
                });

                ui.add_space(8.0);

                ui.group(|ui| {
                    ui.label(egui::RichText::new("⚙️ 轉換參數設定").strong());
                    ui.add_space(3.0);

                    ui.label("輸出解析度 (m):");
                    egui::ComboBox::from_id_source("res_cb")
                        .selected_text(self.resolutions[self.selected_res_idx])
                        .show_ui(ui, |ui| {
                            for (i, r) in self.resolutions.iter().enumerate() {
                                ui.selectable_value(&mut self.selected_res_idx, i, *r);
                            }
                        });

                    ui.add_space(5.0);
                    ui.label("垂直基準面:");
                    egui::ComboBox::from_id_source("datum_cb")
                        .selected_text(self.datums[self.selected_datum_idx])
                        .show_ui(ui, |ui| {
                            for (i, d) in self.datums.iter().enumerate() {
                                ui.selectable_value(&mut self.selected_datum_idx, i, *d);
                            }
                        });

                    ui.add_space(5.0);
                    ui.checkbox(&mut self.auto_align_depth, "自動對齊水深方向 (正值向下)");
                });

                if !self.preview_data.crs_name.is_empty() {
                    ui.add_space(8.0);
                    ui.group(|ui| {
                        ui.label(egui::RichText::new("🌐 來源 BAG 數據資訊").strong());
                        ui.add_space(3.0);
                        ui.label(format!(
                            "網格尺寸: {} × {} 像素",
                            self.preview_data.width, self.preview_data.height
                        ));
                        ui.label(format!("坐標系統: {}", self.preview_data.crs_name));
                        ui.label(format!(
                            "原點 X (Easting) : {:.2}",
                            self.preview_data.geo_transform[0]
                        ));
                        ui.label(format!(
                            "原點 Y (Northing): {:.2}",
                            self.preview_data.geo_transform[3]
                        ));
                        ui.label(format!(
                            "網格解析度      : {:.2} m",
                            self.preview_data.geo_transform[1]
                        ));
                    });
                }

                ui.add_space(15.0);

                let run_btn = ui.add_sized(
                    [ui.available_width(), 38.0],
                    egui::Button::new(egui::RichText::new("開始轉換為 S-102").size(16.0).strong()),
                );

                if run_btn.clicked() {
                    if self.input_path.is_empty() {
                        self.status_message = "錯誤: 請先選擇來源 VR BAG 檔案！".to_string();
                    } else {
                        self.status_message = "轉換中: 正在執行 GDAL 轉碼演算法...".to_string();
                    }
                }

                ui.add_space(5.0);
                ui.label(egui::RichText::new(&self.status_message).color(egui::Color32::KHAKI));
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("水深資料視覺化檢視");
                ui.separator();
                ui.selectable_value(&mut self.view_mode, ViewMode::Depth, "🌊 水深");
                ui.selectable_value(&mut self.view_mode, ViewMode::Uncertainty, "⚠️ 不確定性");

                ui.separator();

                // 🔍 縮放與平移控制按鈕區
                if ui.button("➕ 放大").clicked() {
                    self.zoom = (self.zoom * 1.25).min(10.0);
                }
                if ui.button("➖ 縮小").clicked() {
                    self.zoom = (self.zoom / 1.25).max(0.2);
                }
                if ui.button("🔄 重置視角").clicked() {
                    self.zoom = 1.0;
                    self.pan = egui::Vec2::ZERO;
                }
                ui.label(egui::RichText::new(format!("{:.0}%", self.zoom * 100.0)).weak());

                // 右側座標系統標示
                if !self.preview_data.crs_name.is_empty() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "🌐 坐標系統: {}",
                                self.preview_data.crs_name
                            ))
                            .strong()
                            .color(egui::Color32::LIGHT_BLUE),
                        );
                    });
                }
            });

            ui.separator();

            let current_texture = match self.view_mode {
                ViewMode::Depth => self.preview_data.depth_texture.as_ref(),
                ViewMode::Uncertainty => self.preview_data.uncertainty_texture.as_ref(),
            };

            if let Some(texture) = current_texture {
                let avail_size = ui.available_size();

                // 建立可捕捉拖曳與滾輪事件的互動區域
                let (rect, response) =
                    ui.allocate_exact_size(avail_size, egui::Sense::click_and_drag());

                // 1. 滑鼠滾輪縮放 (Hover 狀態)
                if response.hovered() {
                    let scroll_delta = ui.input(|i| i.smooth_scroll_delta.y);
                    if scroll_delta != 0.0 {
                        let zoom_factor = if scroll_delta > 0.0 { 1.15 } else { 0.85 };
                        self.zoom = (self.zoom * zoom_factor).clamp(0.2, 15.0);
                    }
                }

                // 2. 滑鼠拖曳平移 (Pan)
                if response.dragged() {
                    self.pan += response.drag_delta();
                }

                // 3. 計算圖片縮放與平移後的目標繪製 Rect
                let tex_size = texture.size_vec2();
                let base_scale = (avail_size.x / tex_size.x).min(avail_size.y / tex_size.y);
                let base_display_size = tex_size * base_scale;
                let scaled_size = base_display_size * self.zoom;

                let center = rect.center() + self.pan;
                let image_rect = egui::Rect::from_center_size(center, scaled_size);

                // 4. 繪製畫布背景與縮放平移後的圖片 (裁切在畫布區域內)
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 0.0, egui::Color32::from_gray(25)); // 暗色畫布底色

                let mut child_ui = ui.child_ui(rect, *ui.layout(), None);
                child_ui.set_clip_rect(rect);
                child_ui.painter().image(
                    texture.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );

                // 5. 游標數據檢測（將畫面點映射回原始網格位置）
                if let Some(pointer_pos) = response.hover_pos() {
                    if image_rect.contains(pointer_pos) {
                        let uv_x = ((pointer_pos.x - image_rect.min.x) / image_rect.width())
                            .clamp(0.0, 1.0);
                        let uv_y = ((pointer_pos.y - image_rect.min.y) / image_rect.height())
                            .clamp(0.0, 1.0);

                        let col = ((uv_x * self.preview_data.width as f32) as usize)
                            .min(self.preview_data.width - 1);
                        let row = ((uv_y * self.preview_data.height as f32) as usize)
                            .min(self.preview_data.height - 1);
                        let idx = row * self.preview_data.width + col;

                        let depth_val = self
                            .preview_data
                            .depth_grid
                            .get(idx)
                            .copied()
                            .unwrap_or(f32::NAN);
                        let unc_val = self
                            .preview_data
                            .uncertainty_grid
                            .get(idx)
                            .copied()
                            .unwrap_or(f32::NAN);

                        // 仿射變換計算平面坐標
                        let gt = &self.preview_data.geo_transform;
                        let proj_x = gt[0] + (col as f64 * gt[1]) + (row as f64 * gt[2]);
                        let proj_y = gt[3] + (col as f64 * gt[4]) + (row as f64 * gt[5]);

                        // 經緯度轉換
                        let mut lon = 0.0;
                        let mut lat = 0.0;
                        let mut has_valid_coords = false;

                        if !self.preview_data.spatial_wkt.is_empty() {
                            if let (Ok(src_srs), Ok(target_srs)) = (
                                SpatialRef::from_wkt(&self.preview_data.spatial_wkt),
                                SpatialRef::from_epsg(4326),
                            ) {
                                if let Ok(transform) = CoordTransform::new(&src_srs, &target_srs) {
                                    let mut xs = [proj_x];
                                    let mut ys = [proj_y];
                                    let mut zs = [0.0];
                                    if transform
                                        .transform_coords(&mut xs, &mut ys, &mut zs)
                                        .is_ok()
                                    {
                                        lon = xs[0];
                                        lat = ys[0];
                                        has_valid_coords = true;
                                    }
                                }
                            }
                        }

                        // Tooltip 呈現
                        response.on_hover_ui(|ui| {
                            ui.label(
                                egui::RichText::new("📍 地理空間位置 (EPSG:4326)")
                                    .strong()
                                    .color(egui::Color32::LIGHT_BLUE),
                            );
                            if has_valid_coords {
                                ui.label(format!("經度 (Lon): {:.7}° E", lon));
                                ui.label(format!("緯度 (Lat): {:.7}° N", lat));
                            } else {
                                ui.label("經緯度: (轉換失敗或缺失空間參考)");
                            }
                            ui.label(format!("投影座標: X={:.2}, Y={:.2}", proj_x, proj_y));

                            ui.separator();

                            ui.label(
                                egui::RichText::new("📊 測繪網格數據")
                                    .strong()
                                    .color(egui::Color32::LIGHT_GREEN),
                            );
                            ui.label(format!("水深 (Depth)      : {:.2} m", depth_val));
                            ui.label(format!("不確定性 (Unc.)   : {:.3} m", unc_val));
                            ui.label(format!("網格像素位置      : [行: {}, 列: {}]", row, col));
                        });
                    }
                }
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        egui::RichText::new("請點擊左側「瀏覽」選擇真實的 NOAA .bag 檔案開啟預覽")
                            .size(17.0)
                            .color(egui::Color32::GRAY),
                    );
                });
            }
        });
    }
}
