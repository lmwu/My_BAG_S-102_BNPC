use eframe::egui;
use gdal::raster::RasterBand;
use gdal::spatial_ref::{CoordTransform, SpatialRef};
use gdal::{Dataset, DatasetOptions, GdalOpenFlags, Metadata};
use hdf5::H5Type;
use ndarray;
use rfd::FileDialog;

const BUNDLED_FONT: &[u8] = include_bytes!("../assets/FZYTK.ttf");

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1600.0, 900.0])
            .with_min_inner_size([1280.0, 720.0])
            .with_maximized(true),
        ..Default::default()
    };

    eframe::run_native(
        "我的 NOAA VR BAG 與 S-102 工具 (Sejima Kyuzo 製作)",
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

#[repr(C)]
#[derive(H5Type, Clone, Copy, Debug)]
pub struct S102Point {
    pub depth: f32,
    pub uncertainty: f32,
}

#[derive(PartialEq, Clone, Copy)]
enum ViewMode {
    Depth,
    Uncertainty,
}

#[allow(dead_code)]
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
    pub epsg_code: i32,
    pub base_res: f64,
    pub min_res: f64,
    pub max_res: f64,
    pub available_res: Vec<String>,
    pub vertical_datum_code: i32,
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
            epsg_code: 0,
            base_res: 0.0,
            min_res: 0.0,
            max_res: 0.0,
            available_res: vec![],
            vertical_datum_code: 12,
        }
    }
}

struct VrBagApp {
    input_path: String,
    output_path: String,
    resolutions: Vec<String>,
    selected_res_idx: usize,
    auto_align_depth: bool,
    status_message: String,
    view_mode: ViewMode,
    preview_data: PreviewData,
    zoom: f32,
    pan: egui::Vec2,
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
    fn load_real_bag(&mut self, ctx: &egui::Context, bag_path: &str) {
        let mut base_res = 0.0;
        let mut min_res = 0.0;
        let mut max_res = 0.0;
        let mut vert_datum_code = 12;

        if let Ok(low_res_ds) = Dataset::open(bag_path) {
            if let Ok(gt) = low_res_ds.geo_transform() {
                base_res = gt[1].abs();
            }

            for domain in &["", "xml:BAG"] {
                if let Some(metadata) = low_res_ds.metadata_domain(*domain) {
                    for item in metadata {
                        if let Some((key, val)) = item.split_once('=') {
                            let k = key.trim();
                            let v = val.trim();
                            if k.starts_with("MIN_RESOLUTION") {
                                if let Ok(parsed) = v.parse::<f64>() {
                                    if min_res == 0.0 || parsed < min_res {
                                        min_res = parsed;
                                    }
                                }
                            } else if k.starts_with("MAX_RESOLUTION") {
                                if let Ok(parsed) = v.parse::<f64>() {
                                    if parsed > max_res {
                                        max_res = parsed;
                                    }
                                }
                            }
                        }

                        let parsed_code = parse_vertical_datum_code(&item);
                        if parsed_code != 12 || vert_datum_code == 12 {
                            vert_datum_code = parsed_code;
                        }
                    }
                }
            }
        }

        if base_res == 0.0 {
            base_res = 1.0;
        }
        if min_res == 0.0 {
            min_res = base_res;
        }
        if max_res == 0.0 {
            max_res = base_res;
        }

        let open_opts = DatasetOptions {
            open_flags: GdalOpenFlags::GDAL_OF_RASTER,
            open_options: Some(&[
                "MODE=RESAMPLED_GRID",
                "VALUE_POPULATION=MEAN",
                "RES_STRATEGY=AUTO",
            ]),
            ..Default::default()
        };

        match Dataset::open_ex(bag_path, open_opts) {
            Ok(dataset) => {
                let geo_transform = match dataset.geo_transform() {
                    Ok(gt) => gt,
                    Err(e) => {
                        self.status_message = format!("無法讀取 GeoTransform: {}", e);
                        return;
                    }
                };

                let (orig_width, orig_height) = dataset.raster_size();
                let spatial_wkt = dataset.projection();

                // 解析純水平 EPSG 代碼
                let epsg_code = extract_horizontal_epsg(&spatial_wkt);

                let crs_name = if !spatial_wkt.is_empty() {
                    if let Ok(srs) = SpatialRef::from_wkt(&spatial_wkt) {
                        srs.name().unwrap_or("未知投影座標系".to_string())
                    } else {
                        "無法解析 WKT 坐標系".to_string()
                    }
                } else {
                    "未設定投影坐標系".to_string()
                };

                // 🛡 計算該 BAG 檔案的實體地理涵蓋範圍（公尺）並預先過濾暴記憶體的選項
                let extent_x = geo_transform[1].abs() * orig_width as f64;
                let extent_y = geo_transform[5].abs() * orig_height as f64;

                let mut res_options = vec!["自動 (最佳原生地形解析度)".to_string()];
                let candidate_steps = [
                    0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0,
                ];

                // 安全記憶體上限 (3000 MB ≈ 3 GB)
                let max_safe_mem_mb = 3000.0;

                for &step in &candidate_steps {
                    if step >= min_res * 0.9 && step <= base_res * 1.1 {
                        let export_w = ((extent_x / step).round() as usize).max(1);
                        let export_h = ((extent_y / step).round() as usize).max(1);
                        let total_points = export_w.saturating_mul(export_h);
                        let est_memory_mb = (total_points as f64 * 16.0) / (1024.0 * 1024.0);

                        if est_memory_mb <= max_safe_mem_mb {
                            res_options.push(format!("{:.2} m (原生網格階層)", step));
                        }
                    }
                }

                if res_options.len() == 1 {
                    res_options.push(format!("{:.2} m (母網格)", base_res));
                }

                self.resolutions = res_options.clone();
                self.selected_res_idx = 0;

                let max_preview_dim = 2048;
                let scale = (max_preview_dim as f64 / orig_width.max(orig_height) as f64).min(1.0);
                let width = ((orig_width as f64 * scale).round() as usize).max(1);
                let height = ((orig_height as f64 * scale).round() as usize).max(1);

                let mut preview_gt = geo_transform;
                preview_gt[1] *= orig_width as f64 / width as f64;
                preview_gt[5] *= orig_height as f64 / height as f64;

                let band_depth: RasterBand = match dataset.rasterband(1) {
                    Ok(b) => b,
                    Err(e) => {
                        self.status_message = format!("讀取水深Band失敗: {}", e);
                        return;
                    }
                };
                let band_unc: RasterBand = match dataset.rasterband(2) {
                    Ok(b) => b,
                    Err(e) => {
                        self.status_message = format!("讀取不確定性Band失敗: {}", e);
                        return;
                    }
                };

                let nodata_depth = band_depth.no_data_value().unwrap_or(-1000000.0) as f32;
                let nodata_unc = band_unc.no_data_value().unwrap_or(-1000000.0) as f32;

                let mut depth_buffer = match band_depth.read_as::<f32>(
                    (0, 0),
                    (orig_width, orig_height),
                    (width, height),
                    None,
                ) {
                    Ok(b) => b.data().to_vec(),
                    Err(e) => {
                        self.status_message = format!("讀取水深數據失敗: {}", e);
                        return;
                    }
                };
                let unc_buffer = match band_unc.read_as::<f32>(
                    (0, 0),
                    (orig_width, orig_height),
                    (width, height),
                    None,
                ) {
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
                    geo_transform: preview_gt,
                    spatial_wkt,
                    crs_name,
                    epsg_code,
                    base_res,
                    min_res,
                    max_res,
                    available_res: res_options,
                    vertical_datum_code: vert_datum_code,
                };

                self.zoom = 1.0;
                self.pan = egui::Vec2::ZERO;

                self.status_message = format!(
                    "成功載入 VR BAG！母網格: {:.2}m, 水平 EPSG: {}, 垂直基準碼: {}",
                    base_res, epsg_code, vert_datum_code
                );
            }
            Err(e) => {
                self.status_message = format!("載入 VR BAG 精細網格失敗: {}", e);
            }
        }
    }

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

    fn export_to_s102(&mut self) {
        if self.input_path.is_empty() {
            self.status_message = "錯誤: 請先選擇來源 VR BAG 檔案！".to_string();
            return;
        }
        if self.output_path.is_empty() {
            self.status_message = "錯誤: 請先指定輸出 S-102 檔案路徑！".to_string();
            return;
        }

        let target_res = self.get_selected_resolution_val();

        let open_opts = DatasetOptions {
            open_flags: GdalOpenFlags::GDAL_OF_RASTER,
            open_options: Some(&["MODE=RESAMPLED_GRID", "VALUE_POPULATION=MEAN"]),
            ..Default::default()
        };

        match Dataset::open_ex(&self.input_path, open_opts) {
            Ok(dataset) => {
                let geo_transform = match dataset.geo_transform() {
                    Ok(gt) => gt,
                    Err(e) => {
                        self.status_message = format!("無法讀取 GeoTransform: {}", e);
                        return;
                    }
                };

                let (orig_w, orig_h) = dataset.raster_size();

                let extent_x = geo_transform[1].abs() * orig_w as f64;
                let extent_y = geo_transform[5].abs() * orig_h as f64;
                let export_w = ((extent_x / target_res).round() as usize).max(1);
                let export_h = ((extent_y / target_res).round() as usize).max(1);

                // 🛡 記憶體安全檢查機制 ----------------------------------------------------
                let total_points = export_w.saturating_mul(export_h);
                let est_memory_mb = (total_points as f64 * 16.0) / (1024.0 * 1024.0);

                if est_memory_mb > 4096.0 {
                    self.status_message = format!(
                        "⚠️ 輸出限制：選取 {:.2}m 解析度將產生 {}×{} (約 {:.1} 億點) 網格，需 {:.1} MB 記憶體！請改選較粗解析度。",
                        target_res,
                        export_w,
                        export_h,
                        total_points as f64 / 1e8,
                        est_memory_mb
                    );
                    return;
                }
                // -------------------------------------------------------------------------

                self.status_message =
                    format!("正在處理 {}×{} 網格數據，請稍候...", export_w, export_h);

                let band_depth = match dataset.rasterband(1) {
                    Ok(b) => b,
                    Err(e) => {
                        self.status_message = format!("讀取水深 Band 失敗: {}", e);
                        return;
                    }
                };
                let band_unc = match dataset.rasterband(2) {
                    Ok(b) => b,
                    Err(e) => {
                        self.status_message = format!("讀取不確定性 Band 失敗: {}", e);
                        return;
                    }
                };

                let nodata_d = band_depth.no_data_value().unwrap_or(-1000000.0) as f32;
                let nodata_u = band_unc.no_data_value().unwrap_or(-1000000.0) as f32;

                let depth_data = match band_depth.read_as::<f32>(
                    (0, 0),
                    (orig_w, orig_h),
                    (export_w, export_h),
                    None,
                ) {
                    Ok(b) => b.data().to_vec(),
                    Err(e) => {
                        self.status_message = format!("降採樣水深數據失敗: {}", e);
                        return;
                    }
                };

                let unc_data = match band_unc.read_as::<f32>(
                    (0, 0),
                    (orig_w, orig_h),
                    (export_w, export_h),
                    None,
                ) {
                    Ok(b) => b.data().to_vec(),
                    Err(e) => {
                        self.status_message = format!("降採樣不確定性數據失敗: {}", e);
                        return;
                    }
                };

                // 安全分配記憶體，避免 Panic Crash
                let mut points = Vec::new();
                if points.try_reserve(total_points).is_err() {
                    self.status_message = "❌ 系統記憶體不足，無法分配向量空間！".to_string();
                    return;
                }

                // 修正原點對應：S-102 原點在西南角 (Y-軸由南向北倒序，X-軸由西向東正序)
                for row in (0..export_h).rev() {
                    for col in 0..export_w {
                        let i = row * export_w + col;
                        let mut d = depth_data[i];
                        let u = unc_data[i];

                        let is_d_invalid = d == nodata_d || d.is_nan();
                        let is_u_invalid = u == nodata_u || u.is_nan();

                        if !is_d_invalid && self.auto_align_depth {
                            d = -d;
                        }

                        points.push(S102Point {
                            depth: if is_d_invalid { 1000000.0 } else { d },
                            uncertainty: if is_u_invalid { 1000000.0 } else { u },
                        });
                    }
                }

                let min_x = geo_transform[0];
                let max_x = min_x + (export_w as f64 * target_res);
                let min_y = geo_transform[3] - (export_h as f64 * target_res);
                let max_y = geo_transform[3];
                let dx = target_res;
                let dy = target_res;

                let epsg = extract_horizontal_epsg(&dataset.projection());
                let final_epsg = if epsg > 0 {
                    epsg
                } else {
                    self.preview_data.epsg_code
                };

                if let Err(e) = self.write_s102_hdf5_file(
                    &self.output_path,
                    &points,
                    export_w,
                    export_h,
                    min_x,
                    max_x,
                    min_y,
                    max_y,
                    dx,
                    dy,
                    final_epsg,
                    self.preview_data.vertical_datum_code,
                ) {
                    self.status_message = format!("寫入 S-102 HDF5 失敗: {}", e);
                } else {
                    self.status_message = format!(
                        "轉檔成功！目標解析度: {:.2}m, 維度: {}x{}, 水平 EPSG: {} (QGIS請選擇此CRS)",
                        target_res, export_w, export_h, final_epsg
                    );
                }
            }
            Err(e) => {
                self.status_message = format!("開啟 BAG 檔案失敗: {}", e);
            }
        }
    }

    fn write_s102_hdf5_file(
        &self,
        output_path: &str,
        points: &[S102Point],
        width: usize,
        height: usize,
        min_x: f64,
        max_x: f64,
        min_y: f64,
        max_y: f64,
        dx: f64,
        dy: f64,
        epsg: i32,
        vertical_datum: i32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let file = hdf5::File::create(output_path)?;

        file.new_attr::<hdf5::types::VarLenUnicode>()
            .create("productSpecification")?
            .write_scalar(&"INT.IHO.S-102.2.1".parse::<hdf5::types::VarLenUnicode>()?)?;
        file.new_attr::<hdf5::types::VarLenUnicode>()
            .create("issueDate")?
            .write_scalar(&"2026-09-03".parse::<hdf5::types::VarLenUnicode>()?)?;

        file.new_attr::<f64>()
            .create("westBoundLongitude")?
            .write_scalar(&min_x)?;
        file.new_attr::<f64>()
            .create("eastBoundLongitude")?
            .write_scalar(&max_x)?;
        file.new_attr::<f64>()
            .create("southBoundLatitude")?
            .write_scalar(&min_y)?;
        file.new_attr::<f64>()
            .create("northBoundLatitude")?
            .write_scalar(&max_y)?;

        file.new_attr::<hdf5::types::VarLenUnicode>()
            .create("horizontalDatumReference")?
            .write_scalar(&"EPSG".parse::<hdf5::types::VarLenUnicode>()?)?;
        file.new_attr::<i32>()
            .create("epsgCode")?
            .write_scalar(&epsg)?;
        file.new_attr::<i32>()
            .create("horizontalCRS")?
            .write_scalar(&epsg)?;

        file.new_attr::<i32>()
            .create("verticalDatum")?
            .write_scalar(&vertical_datum)?;

        // 2. 垂直坐標方向 (S-100 規範: 1 代表深度向下正值, Depth)
        file.new_attr::<i32>()
            .create("verticalCS")?
            .write_scalar(&1)?;

        // 3. 垂直坐標基準 (S-100 規範: 1 代表水面/海圖基準面 Surface)
        file.new_attr::<i32>()
            .create("verticalCoordinateBase")?
            .write_scalar(&1)?;

        let group_cov = file.create_group("BathymetryCoverage")?;
        let group_sub = group_cov.create_group("BathymetryCoverage.01")?;

        group_sub
            .new_attr::<f64>()
            .create("gridOriginLongitude")?
            .write_scalar(&min_x)?;
        group_sub
            .new_attr::<f64>()
            .create("gridOriginLatitude")?
            .write_scalar(&min_y)?;
        group_sub
            .new_attr::<f64>()
            .create("gridSpacingLongitudinal")?
            .write_scalar(&dx)?;
        group_sub
            .new_attr::<f64>()
            .create("gridSpacingLatitudinal")?
            .write_scalar(&dy)?;
        group_sub
            .new_attr::<u32>()
            .create("numPointsLongitudinal")?
            .write_scalar(&(width as u32))?;
        group_sub
            .new_attr::<u32>()
            .create("numPointsLatitudinal")?
            .write_scalar(&(height as u32))?;
        group_sub
            .new_attr::<hdf5::types::VarLenUnicode>()
            .create("startSequence")?
            .write_scalar(&"0,0".parse::<hdf5::types::VarLenUnicode>()?)?;

        let group_g001 = group_sub.create_group("Group_001")?;
        let points_2d = ndarray::ArrayView2::from_shape((height, width), points)?;
        let dataset = group_g001
            .new_dataset_builder()
            .empty::<S102Point>()
            .shape([height, width])
            .create("values")?;

        dataset.write(points_2d)?;

        Ok(())
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
                    self.export_to_s102();
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

// -----------------------------------------------------------------------------
// 獨立輔助函式 (Helper Functions)
// -----------------------------------------------------------------------------

/// 從 BAG WKT 中精確剝離並只提取「水平 CRS」的 EPSG 代碼
pub fn extract_horizontal_epsg(spatial_wkt: &str) -> i32 {
    if spatial_wkt.is_empty() {
        return 0;
    }

    // 1. 優先使用 GDAL 原生 API 辨識
    if let Ok(mut srs) = SpatialRef::from_wkt(spatial_wkt) {
        let _ = srs.auto_identify_epsg();
        if let Ok(code) = srs.auth_code() {
            if code > 0 {
                return code as i32;
            }
        }
    }

    // 2. 後備機制 (Fallback)：先裁切掉垂直坐標系 (VERT_CS) 區塊
    let horiz_wkt = if let Some(v_pos) = spatial_wkt.find("VERT_CS[") {
        &spatial_wkt[..v_pos]
    } else {
        spatial_wkt
    };

    // 3. 使用 rfind 由後往前搜尋：
    // WKT 結構中，最外層 PROJCS/GEOGCS 的 EPSG 代碼 (如 8693) 必在區塊末端；
    // 裡層的 SPHEROID (7019) 與 DATUM (1116) 則在前段。
    if let Some(pos) = horiz_wkt.rfind(r#"AUTHORITY["EPSG""#) {
        let sub = &horiz_wkt[pos..];
        let digits: String = sub
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        return digits.parse::<i32>().unwrap_or(0);
    }

    0
}

/// 將 BAG 檔案中的垂直基準面字串映射為 S-102 (S-100) 標準整數代碼
pub fn parse_vertical_datum_code(datum_str: &str) -> i32 {
    let upper = datum_str.to_uppercase();
    if upper.contains("MLLW") || upper.contains("MEAN LOWER LOW WATER") {
        12 // S-100: Mean Lower Low Water
    } else if upper.contains("LAT") || upper.contains("LOWEST ASTRONOMICAL TIDE") {
        10 // S-100: Lowest Astronomical Tide
    } else if upper.contains("MLW") || upper.contains("MEAN LOW WATER") {
        11 // S-100: Mean Low Water
    } else if upper.contains("MSL") || upper.contains("MEAN SEA LEVEL") {
        1 // S-100: Mean Sea Level
    } else {
        12 // 預設 MLLW
    }
}
