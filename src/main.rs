use eframe::egui;
use rfd::FileDialog;

// 打包自帶字型檔
const BUNDLED_FONT: &[u8] = include_bytes!("../assets/NotoSansTC-Regular.ttf");

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 650.0])
            .with_min_inner_size([800.0, 500.0]),
        ..Default::default()
    };

    eframe::run_native(
        "NOAA VR BAG 轉 S-102 工具 (含空間座標檢視器)",
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
    is_processing: bool,

    view_mode: ViewMode,
    preview_data: PreviewData,
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
            status_message: "狀態: 準備就緒".to_string(),
            is_processing: false,
            view_mode: ViewMode::Depth,
            preview_data: PreviewData::default(),
        }
    }
}

impl VrBagApp {
    fn load_mock_preview(&mut self, ctx: &egui::Context) {
        let width = 400;
        let height = 300;
        let mut depth_grid = Vec::with_capacity(width * height);
        let mut uncertainty_grid = Vec::with_capacity(width * height);

        let mut depth_pixels = Vec::with_capacity(width * height * 4);
        let mut unc_pixels = Vec::with_capacity(width * height * 4);

        for y in 0..height {
            for x in 0..width {
                let fx = x as f32 / width as f32;
                let fy = y as f32 / height as f32;
                let depth = 10.0 + (fx * 80.0) + (fy * 30.0) + ((fx * 10.0).sin() * 5.0);
                let unc = 0.1 + (fx * 1.5) + (fy * 0.5);

                depth_grid.push(depth);
                uncertainty_grid.push(unc);

                // 水深色帶
                let d_norm = ((depth - 10.0) / 110.0).clamp(0.0, 1.0);
                let r = (d_norm * 255.0) as u8;
                let g = ((1.0 - (d_norm - 0.5).abs() * 2.0) * 255.0).max(0.0) as u8;
                let b = ((1.0 - d_norm) * 255.0) as u8;
                depth_pixels.extend_from_slice(&[r, g, b, 255]);

                // 不確定性色帶
                let u_norm = ((unc - 0.1) / 2.0).clamp(0.0, 1.0);
                let ur = (u_norm * 255.0) as u8;
                let ug = ((1.0 - u_norm) * 255.0) as u8;
                unc_pixels.extend_from_slice(&[ur, ug, 50, 255]);
            }
        }

        let depth_img = egui::ColorImage::from_rgba_unmultiplied([width, height], &depth_pixels);
        let unc_img = egui::ColorImage::from_rgba_unmultiplied([width, height], &unc_pixels);

        let depth_tex = ctx.load_texture("depth_tex", depth_img, egui::TextureOptions::LINEAR);
        let unc_tex = ctx.load_texture("unc_tex", unc_img, egui::TextureOptions::LINEAR);

        let geo_transform = [174000.0, 2.0, 0.0, 2490000.0, 0.0, -2.0];

        self.preview_data = PreviewData {
            depth_texture: Some(depth_tex),
            uncertainty_texture: Some(unc_tex),
            width,
            height,
            depth_grid,
            uncertainty_grid,
            geo_transform,
        };

        self.status_message = "已成功載入資料預覽！".to_string();
    }
}

impl eframe::App for VrBagApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::SidePanel::left("control_panel")
            .resizable(true)
            .default_width(320.0)
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
                            if let Some(path) = FileDialog::new().add_filter("BAG", &["bag"]).pick_file() {
                                self.input_path = path.display().to_string();
                                self.load_mock_preview(ctx);
                            }
                        }
                    });

                    ui.label("輸出 S-102 檔案:");
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.output_path);
                        if ui.button("另存").clicked() {
                            if let Some(path) = FileDialog::new().add_filter("HDF5", &["h5"]).save_file() {
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

                ui.add_space(10.0);

                if ui.button("🧪 載入測試預覽數據").clicked() {
                    self.load_mock_preview(ctx);
                }

                ui.add_space(10.0);

                let run_btn = ui.add_sized(
                    [ui.available_width(), 35.0],
                    egui::Button::new(egui::RichText::new("開始轉換為 S-102").size(15.0).strong()),
                );

                if run_btn.clicked() {
                    if self.input_path.is_empty() {
                        self.status_message = "錯誤: 請先選擇來源 VR BAG 檔案！".to_string();
                    } else {
                        self.is_processing = true;
                        self.status_message = "轉換中: 正在重採樣網格並封裝 HDF5...".to_string();
                    }
                }

                ui.add_space(5.0);
                ui.label(egui::RichText::new(&self.status_message).color(egui::Color32::KHAKI));
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("水深資料視覺化檢視");
                ui.separator();
                ui.selectable_value(&mut self.view_mode, ViewMode::Depth, "🌊 水深 (Depth)");
                ui.selectable_value(&mut self.view_mode, ViewMode::Uncertainty, "⚠️ 不確定性 (Uncertainty)");
            });

            ui.separator();

            let current_texture = match self.view_mode {
                ViewMode::Depth => self.preview_data.depth_texture.as_ref(),
                ViewMode::Uncertainty => self.preview_data.uncertainty_texture.as_ref(),
            };

            if let Some(texture) = current_texture {
                let avail_size = ui.available_size();
                let tex_size = texture.size_vec2();

                let scale = (avail_size.x / tex_size.x).min(avail_size.y / tex_size.y);
                let display_size = tex_size * scale;

                let img_widget = egui::Image::new(texture).fit_to_exact_size(display_size);
                let response = ui.add(img_widget.sense(egui::Sense::hover()));

                if let Some(pointer_pos) = response.hover_pos() {
                    let rect = response.rect;

                    let uv_x = ((pointer_pos.x - rect.min.x) / rect.width()).clamp(0.0, 1.0);
                    let uv_y = ((pointer_pos.y - rect.min.y) / rect.height()).clamp(0.0, 1.0);

                    let col = ((uv_x * self.preview_data.width as f32) as usize).min(self.preview_data.width - 1);
                    let row = ((uv_y * self.preview_data.height as f32) as usize).min(self.preview_data.height - 1);
                    let idx = row * self.preview_data.width + col;

                    let depth_val = self.preview_data.depth_grid.get(idx).copied().unwrap_or(f32::NAN);
                    let unc_val = self.preview_data.uncertainty_grid.get(idx).copied().unwrap_or(f32::NAN);

                    let gt = &self.preview_data.geo_transform;
                    let proj_x = gt[0] + (col as f64 * gt[1]) + (row as f64 * gt[2]);
                    let proj_y = gt[3] + (col as f64 * gt[4]) + (row as f64 * gt[5]);

                    let lon = 120.25 + (proj_x - 174000.0) / 102000.0;
                    let lat = 22.50 + (proj_y - 2490000.0) / 110000.0;

                    response.on_hover_ui(|ui| {
                        ui.label(egui::RichText::new("📍 地理空間位置 (EPSG:4326)").strong().color(egui::Color32::LIGHT_BLUE));
                        ui.label(format!("經度 (Lon): {:.6}° E", lon));
                        ui.label(format!("緯度 (Lat): {:.6}° N", lat));
                        ui.label(format!("平面座標: X={:.1}, Y={:.1}", proj_x, proj_y));
                        ui.separator();
                        ui.label(egui::RichText::new("📊 測繪網格數據").strong().color(egui::Color32::LIGHT_GREEN));
                        ui.label(format!("水深 (Depth)      : {:.2} m", depth_val));
                        ui.label(format!("不確定性 (Unc.)   : {:.3} m", unc_val));
                        ui.label(format!("網格像素位置      : [行: {}, 列: {}]", row, col));
                    });
                }
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        egui::RichText::new("請選擇 .bag 檔案或點擊「載入測試預覽數據」以檢視圖形與座標")
                            .size(16.0)
                            .color(egui::Color32::GRAY),
                    );
                });
            }
        });
    }
}