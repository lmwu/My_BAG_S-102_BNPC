use eframe::egui;
use gdal::spatial_ref::{CoordTransform, SpatialRef};
use rfd::FileDialog;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

use crate::io::bag::{export_to_s102, load_real_bag};
use crate::models::{PreviewData, ViewMode};

/// 背景執行緒任務完成後傳回主 UI 的訊息型態
pub enum AppTaskResult {
    BagLoaded(Result<(PreviewData, Vec<String>), String>),
    ExportFinished(Result<String, String>),
}

pub struct VrBagApp {
    pub input_path: String,
    pub output_path: String,
    pub resolutions: Vec<String>,
    pub selected_res_idx: usize,
    pub auto_align_depth: bool,
    pub status_message: String,
    pub view_mode: ViewMode,
    pub preview_data: PreviewData,
    pub zoom: f32,
    pub pan: egui::Vec2,
    pub is_processing: bool,

    // BNPC 控制參數
    pub enable_bnpc: bool,          // 開啟/關閉 BNPC 等深線
    pub bnpc_depth: f32,            // BNPC 目標水深 (如 10m, 20m)
    pub bnpc_smoothing: usize,      // 平滑度疊代次數 (0~3)
    pub bnpc_stroke_width: f32,     // 線條粗細
    pub bnpc_color: egui::Color32,  // 線條顏色

    // ⚡ BNPC 快取記憶體 (避免拉圖拖慢 CPU，實現 60 FPS 順暢度)
    pub cached_contours: Vec<crate::bnpc::BnpcContour>,
    pub last_computed_depth: f32,
    pub last_computed_smooth: usize,

    // 通訊管道
    rx: Receiver<AppTaskResult>,
    tx: Sender<AppTaskResult>,
}

impl Default for VrBagApp {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            input_path: String::new(),
            output_path: String::new(),
            resolutions: vec![
                "自動 (取最佳原生地形網格)".to_string(),
                "0.50 m (原生網格階層)".to_string(),
                "1.00 m (原生網格階層)".to_string(),
                "2.00 m (原生網格階層)".to_string(),
            ],
            selected_res_idx: 0,
            auto_align_depth: true,
            status_message: "狀態: 請選擇可變網格 NOAA .bag 格式檔案".to_string(),
            view_mode: ViewMode::Depth,
            preview_data: PreviewData::default(),
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            is_processing: false,

            enable_bnpc: false,
            bnpc_depth: 10.0,
            bnpc_smoothing: 1,
            bnpc_stroke_width: 3.0,
            bnpc_color: egui::Color32::from_rgb(255, 30, 30), // 鮮明紅色

            cached_contours: Vec::new(),
            last_computed_depth: f32::NAN,
            last_computed_smooth: 99,
            rx,
            tx,
        }
    }
}

impl VrBagApp {
    /// 從下拉選單文字中解析目標解析度數值 (公尺)
    fn get_selected_resolution_val(&self) -> f64 {
        if self.resolutions.is_empty() || self.selected_res_idx >= self.resolutions.len() {
            return self.preview_data.min_res.max(0.25);
        }
        let text = &self.resolutions[self.selected_res_idx];
        if text.starts_with("自動") {
            return self.preview_data.min_res.max(0.25);
        }
        text.split_whitespace()
            .next()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(self.preview_data.min_res.max(0.25))
    }

    /// 輪詢背景執行緒的回傳結果
    fn check_async_updates(&mut self, ctx: &egui::Context) {
        if let Ok(task_result) = self.rx.try_recv() {
            self.is_processing = false;
            match task_result {
                AppTaskResult::BagLoaded(res) => match res {
                    Ok((preview, resolutions)) => {
                        let base_res = preview.base_res;
                        let epsg_code = preview.epsg_code;
                        let vert_datum_code = preview.vertical_datum_code;

                        self.preview_data = preview;
                        self.resolutions = resolutions;
                        self.selected_res_idx = 0;
                        self.zoom = 1.0;
                        self.pan = egui::Vec2::ZERO;

                        // 載入新檔案時清空舊等深線快取
                        self.cached_contours.clear();
                        self.last_computed_depth = f32::NAN;

                        self.status_message = format!(
                            "✅ 成功載入 VR BAG！母網格: {:.2}m, 水平 EPSG: {}, 垂直基準碼: {}",
                            base_res, epsg_code, vert_datum_code
                        );
                    }
                    Err(err_msg) => {
                        self.status_message = err_msg;
                    }
                },
                AppTaskResult::ExportFinished(res) => match res {
                    Ok(msg) => self.status_message = msg,
                    Err(msg) => self.status_message = msg,
                },
            }
            ctx.request_repaint();
        }
    }

    /// 非阻塞背景讀取 BAG 檔案
    fn handle_load_bag(&mut self, ctx: &egui::Context, path: String) {
        self.is_processing = true;
        self.status_message = "⏳ 正在讀取並解析 VR BAG 檔案中，請稍候...".to_string();

        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let align = self.auto_align_depth;

        thread::spawn(move || {
            let res = load_real_bag(&ctx, &path, align);
            let _ = tx.send(AppTaskResult::BagLoaded(res));
            ctx.request_repaint();
        });
    }

    /// 非阻塞背景執行 S-102 平行轉檔
    fn handle_export(&mut self, ctx: &egui::Context) {
        if self.input_path.is_empty() {
            self.status_message = "❌ 錯誤: 請先選擇來源 VR BAG 檔案！".to_string();
            return;
        }
        if self.output_path.is_empty() {
            self.status_message = "❌ 錯誤: 請先指定輸出 S-102 檔案路徑！".to_string();
            return;
        }

        self.is_processing = true;
        self.status_message =
            "⏳ 正在進行 Rayon 平行化降採樣與 S-102 HDF5 寫入，請稍候...".to_string();

        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let input = self.input_path.clone();
        let output = self.output_path.clone();
        let target_res = self.get_selected_resolution_val();
        let align = self.auto_align_depth;
        let epsg = self.preview_data.epsg_code;
        let datum = self.preview_data.vertical_datum_code;

        thread::spawn(move || {
            let res = export_to_s102(&input, &output, target_res, align, epsg, datum);
            let _ = tx.send(AppTaskResult::ExportFinished(res));
            ctx.request_repaint();
        });
    }
}

impl eframe::App for VrBagApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 檢查背景任務狀態
        self.check_async_updates(ctx);

        // 背景任務處理中時持續請求重繪以更新動畫/狀態
        if self.is_processing {
            ctx.request_repaint();
        }

        egui::SidePanel::left("control_panel")
            .resizable(true)
            .default_width(340.0)
            .show(ctx, |ui| {
                ui.add_space(5.0);
                ui.heading("我的 NOAA VR BAG 轉檔工具");
                ui.separator();

                // 處理中時停用控制控制項
                ui.add_enabled_ui(!self.is_processing, |ui| {
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
                                    self.handle_load_bag(ctx, path_str);
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

                    // 🎯 BNPC 控制選項組
                    ui.group(|ui| {
                        ui.label(
                            egui::RichText::new("🎯 潛艦 BNPC 動態安全等深線")
                                .strong()
                                .color(egui::Color32::GOLD),
                        );
                        ui.checkbox(&mut self.enable_bnpc, "打開 On-the-Fly BNPC 算繪");

                        if self.enable_bnpc {
                            ui.add(
                                egui::Slider::new(&mut self.bnpc_depth, -50.0..=1000.0)
                                    .text("安全水深 (m)"),
                            );
                            ui.add(
                                egui::Slider::new(&mut self.bnpc_smoothing, 0..=3)
                                    .text("曲線平滑度"),
                            );
                            ui.add(
                                egui::Slider::new(&mut self.bnpc_stroke_width, 0.5..=10.0)
                                    .text("等深線粗細"),
                            );
                            ui.horizontal(|ui| {
                                ui.label("等深線顏色:");
                                ui.color_edit_button_srgba(&mut self.bnpc_color);
                            });
                        }
                    });

                    ui.add_space(8.0);

                    ui.group(|ui| {
                        ui.label(egui::RichText::new("⚙ 轉檔設定").strong());
                        ui.add_space(3.0);

                        ui.label("輸出解析度 (m):");
                        if self.selected_res_idx >= self.resolutions.len() {
                            self.selected_res_idx = 0;
                        }
                        egui::ComboBox::from_id_source("res_cb")
                            .selected_text(&self.resolutions[self.selected_res_idx])
                            .show_ui(ui, |ui| {
                                for (i, r) in self.resolutions.iter().enumerate() {
                                    ui.selectable_value(&mut self.selected_res_idx, i, r);
                                }
                            });

                        ui.add_space(5.0);
                        ui.checkbox(&mut self.auto_align_depth, "自動對齊水深方向 (正值向下)");
                    });

                    if !self.preview_data.crs_name.is_empty() {
                        ui.add_space(8.0);
                        ui.group(|ui| {
                            ui.label(egui::RichText::new("來源 VR BAG 數據資訊").strong());
                            ui.add_space(3.0);
                            ui.label(format!(
                                "母網格尺寸  : {} × {} 像素",
                                self.preview_data.width, self.preview_data.height
                            ));
                            ui.label(format!("坐標系統    : {}", self.preview_data.crs_name));
                            ui.label(format!("水平 EPSG   : {}", self.preview_data.epsg_code));
                            ui.label(format!(
                                "垂直基準碼  : {} (S-100標準)",
                                self.preview_data.vertical_datum_code
                            ));
                            ui.label(format!(
                                "原點 X (Easting) : {:.2}",
                                self.preview_data.geo_transform[0]
                            ));
                            ui.label(format!(
                                "原點 Y (Northing): {:.2}",
                                self.preview_data.geo_transform[3]
                            ));

                            ui.separator();

                            ui.label(
                                egui::RichText::new("📐 可變網格 (VR) 解析度")
                                    .strong()
                                    .color(egui::Color32::KHAKI),
                            );
                            ui.label(format!(
                                "母網格解析度  : {:.2} m",
                                self.preview_data.base_res
                            ));
                            ui.label(format!(
                                "最細網格解析度: {:.2} m",
                                self.preview_data.min_res
                            ));
                            ui.label(format!(
                                "最粗網格解析度: {:.2} m",
                                self.preview_data.max_res
                            ));
                        });
                    }

                    ui.add_space(15.0);

                    let run_btn = ui.add_sized(
                        [ui.available_width(), 38.0],
                        egui::Button::new(
                            egui::RichText::new("開始轉格式至 S-102")
                                .size(16.0)
                                .strong(),
                        ),
                    );

                    if run_btn.clicked() {
                        self.handle_export(ctx);
                    }
                });

                ui.add_space(8.0);
                if self.is_processing {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new(&self.status_message)
                                .color(egui::Color32::GOLD)
                                .strong(),
                        );
                    });
                } else {
                    ui.label(egui::RichText::new(&self.status_message).color(egui::Color32::KHAKI));
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("水深資料視覺化檢視");
                ui.separator();
                ui.selectable_value(&mut self.view_mode, ViewMode::Depth, "🌊 水深");
                ui.selectable_value(&mut self.view_mode, ViewMode::Uncertainty, "⚠ 不確定性");

                ui.separator();

                if ui.button("➕ 放大").clicked() {
                    self.zoom = (self.zoom * 1.25).min(15.0);
                }
                if ui.button("➖ 縮小").clicked() {
                    self.zoom = (self.zoom / 1.25).max(0.2);
                }
                if ui.button("🔄 重置視角").clicked() {
                    self.zoom = 1.0;
                    self.pan = egui::Vec2::ZERO;
                }
                ui.label(egui::RichText::new(format!("{:.0}%", self.zoom * 100.0)).weak());

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
                let (rect, response) =
                    ui.allocate_exact_size(avail_size, egui::Sense::click_and_drag());

                if response.hovered() {
                    let scroll_delta = ui.input(|i| i.smooth_scroll_delta.y);
                    if scroll_delta != 0.0 {
                        let zoom_factor = if scroll_delta > 0.0 { 1.15 } else { 0.85 };
                        self.zoom = (self.zoom * zoom_factor).clamp(0.2, 15.0);
                    }
                }

                if response.dragged() {
                    self.pan += response.drag_delta();
                }

                let tex_size = texture.size_vec2();
                let base_scale = (avail_size.x / tex_size.x).min(avail_size.y / tex_size.y);
                let base_display_size = tex_size * base_scale;
                let scaled_size = base_display_size * self.zoom;

                let center = rect.center() + self.pan;
                let image_rect = egui::Rect::from_center_size(center, scaled_size);

                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 0.0, egui::Color32::from_gray(25));

                let mut child_ui = ui.child_ui(rect, *ui.layout(), None);
                child_ui.set_clip_rect(rect);
                child_ui.painter().image(
                    texture.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );

                // ---------------- ⚡ BNPC 等深線繪製 (含快取機制) ----------------
                if self.enable_bnpc && !self.preview_data.depth_grid.is_empty() {
                    // 1. 僅在「參數變動」或「尚無快取」時才計算等深線，避免拉圖時重算拖慢 CPU
                    if (self.bnpc_depth - self.last_computed_depth).abs() > 0.001
                        || self.bnpc_smoothing != self.last_computed_smooth
                        || self.cached_contours.is_empty()
                    {
                        self.cached_contours = crate::bnpc::generate_bnpc_contours(
                            &self.preview_data.depth_grid,
                            self.preview_data.width,
                            self.preview_data.height,
                            self.bnpc_depth,
                            self.bnpc_smoothing,
                        );
                        self.last_computed_depth = self.bnpc_depth;
                        self.last_computed_smooth = self.bnpc_smoothing;
                    }

                    // 2. 超高速 GPU 畫布繪製 (使用 Shape::line 繪製連貫多折線)
                    let stroke = egui::Stroke::new(self.bnpc_stroke_width, self.bnpc_color);
                    let grid_w = self.preview_data.width as f32;
                    let grid_h = self.preview_data.height as f32;

                    for line in &self.cached_contours {
                        let screen_points: Vec<egui::Pos2> = line
                            .iter()
                            .map(|pt| {
                                let norm_x = pt.x / grid_w;
                                let norm_y = pt.y / grid_h;
                                egui::pos2(
                                    image_rect.min.x + norm_x * image_rect.width(),
                                    image_rect.min.y + norm_y * image_rect.height(),
                                )
                            })
                            .collect();

                        if screen_points.len() >= 2 {
                            child_ui
                                .painter()
                                .add(egui::Shape::line(screen_points, stroke));
                        }
                    }
                }

                // 滑鼠懸浮視窗資訊展示
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

                        let gt = &self.preview_data.geo_transform;
                        let proj_x = gt[0] + (col as f64 * gt[1]) + (row as f64 * gt[2]);
                        let proj_y = gt[3] + (col as f64 * gt[4]) + (row as f64 * gt[5]);

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
                                        lat = xs[0];
                                        lon = ys[0];
                                        has_valid_coords = true;
                                    }
                                }
                            }
                        }

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
                                ui.label("經緯度: (轉碼失敗或缺失空間參考)");
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
                        egui::RichText::new("請點擊左側「瀏覽」載入 VR NOAA .bag 檔案預覽")
                            .size(17.0)
                            .color(egui::Color32::GRAY),
                    );
                });
            }
        });
    }
}