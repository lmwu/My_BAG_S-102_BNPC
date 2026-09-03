use eframe::egui;
use gdal::spatial_ref::{CoordTransform, SpatialRef};
use rfd::FileDialog;

use crate::io::bag::{export_to_s102, load_real_bag};
use crate::models::{PreviewData, ViewMode};

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
}

impl Default for VrBagApp {
    fn default() -> Self {
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
        }
    }
}

impl VrBagApp {
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

    fn handle_load_bag(&mut self, ctx: &egui::Context, path: &str) {
        match load_real_bag(ctx, path, self.auto_align_depth) {
            Ok((preview, resolutions)) => {
                let base_res = preview.base_res;
                let epsg_code = preview.epsg_code;
                let vert_datum_code = preview.vertical_datum_code;

                self.preview_data = preview;
                self.resolutions = resolutions;
                self.selected_res_idx = 0;
                self.zoom = 1.0;
                self.pan = egui::Vec2::ZERO;

                self.status_message = format!(
                    "成功載入 VR BAG！母網格: {:.2}m, 水平 EPSG: {}, 垂直基準碼: {}",
                    base_res, epsg_code, vert_datum_code
                );
            }
            Err(err_msg) => {
                self.status_message = err_msg;
            }
        }
    }

    fn handle_export(&mut self) {
        if self.input_path.is_empty() {
            self.status_message = "錯誤: 請先選擇來源 VR BAG 檔案！".to_string();
            return;
        }
        if self.output_path.is_empty() {
            self.status_message = "錯誤: 請先指定輸出 S-102 檔案路徑！".to_string();
            return;
        }

        let target_res = self.get_selected_resolution_val();
        match export_to_s102(
            &self.input_path,
            &self.output_path,
            target_res,
            self.auto_align_depth,
            self.preview_data.epsg_code,
            self.preview_data.vertical_datum_code,
        ) {
            Ok(msg) => self.status_message = msg,
            Err(msg) => self.status_message = msg,
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
                ui.heading("我的 NOAA VR BAG 轉檔工具");
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
                                self.handle_load_bag(ctx, &path_str);
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
                    self.handle_export();
                }

                ui.add_space(5.0);
                ui.label(egui::RichText::new(&self.status_message).color(egui::Color32::KHAKI));
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("水深資料視覺化檢視");
                ui.separator();
                ui.selectable_value(&mut self.view_mode, ViewMode::Depth, "🌊 水深");
                ui.selectable_value(&mut self.view_mode, ViewMode::Uncertainty, "⚠ 不確定性");

                ui.separator();

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