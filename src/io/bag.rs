use eframe::egui;
use gdal::raster::RasterBand;
use gdal::spatial_ref::SpatialRef;
use gdal::{Dataset, DatasetOptions, GdalOpenFlags, Metadata};
use rayon::prelude::*;

use crate::io::s102::write_s102_hdf5_file;
use crate::models::{PreviewData, S102Point};
use crate::utils::{extract_horizontal_epsg, parse_vertical_datum_code};

pub fn load_real_bag(
    ctx: &egui::Context,
    bag_path: &str,
    auto_align_depth: bool,
) -> Result<(PreviewData, Vec<String>), String> {
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

    if base_res == 0.0 { base_res = 1.0; }
    if min_res == 0.0 { min_res = base_res; }
    if max_res == 0.0 { max_res = base_res; }

    let open_opts = DatasetOptions {
        open_flags: GdalOpenFlags::GDAL_OF_RASTER,
        open_options: Some(&[
            "MODE=RESAMPLED_GRID",
            "VALUE_POPULATION=MEAN",
            "RES_STRATEGY=AUTO",
        ]),
        ..Default::default()
    };

    let dataset = Dataset::open_ex(bag_path, open_opts)
        .map_err(|e| format!("載入 VR BAG 精細網格失敗: {}", e))?;

    let geo_transform = dataset
        .geo_transform()
        .map_err(|e| format!("無法讀取 GeoTransform: {}", e))?;

    let (orig_width, orig_height) = dataset.raster_size();
    let spatial_wkt = dataset.projection();
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

    let extent_x = geo_transform[1].abs() * orig_width as f64;
    let extent_y = geo_transform[5].abs() * orig_height as f64;

    let mut res_options = vec!["自動 (最佳原生地形解析度)".to_string()];
    let candidate_steps = [
        0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0,
    ];
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

    let max_preview_dim = 2048;
    let scale = (max_preview_dim as f64 / orig_width.max(orig_height) as f64).min(1.0);
    let width = ((orig_width as f64 * scale).round() as usize).max(1);
    let height = ((orig_height as f64 * scale).round() as usize).max(1);

    let mut preview_gt = geo_transform;
    preview_gt[1] *= orig_width as f64 / width as f64;
    preview_gt[5] *= orig_height as f64 / height as f64;

    let band_depth: RasterBand = dataset
        .rasterband(1)
        .map_err(|e| format!("讀取水深Band失敗: {}", e))?;
    let band_unc: RasterBand = dataset
        .rasterband(2)
        .map_err(|e| format!("讀取不確定性Band失敗: {}", e))?;

    let nodata_depth = band_depth.no_data_value().unwrap_or(-1000000.0) as f32;
    let nodata_unc = band_unc.no_data_value().unwrap_or(-1000000.0) as f32;

    let mut depth_buffer = band_depth
        .read_as::<f32>((0, 0), (orig_width, orig_height), (width, height), None)
        .map_err(|e| format!("讀取水深數據失敗: {}", e))?
        .data()
        .to_vec();

    let unc_buffer = band_unc
        .read_as::<f32>((0, 0), (orig_width, orig_height), (width, height), None)
        .map_err(|e| format!("讀取不確定性數據失敗: {}", e))?
        .data()
        .to_vec();

    if auto_align_depth {
        depth_buffer.par_iter_mut().for_each(|val| {
            if *val != nodata_depth && !val.is_nan() {
                *val = -(*val);
            }
        });
    }

    // --- Rayon 平行計算極值 (Min / Max) ---
    let (min_d, max_d) = depth_buffer
        .par_iter()
        .copied()
        .filter(|&v| v != nodata_depth && !v.is_nan())
        .fold(
            || (f32::INFINITY, f32::NEG_INFINITY),
            |(min, max), v| (min.min(v), max.max(v)),
        )
        .reduce(
            || (f32::INFINITY, f32::NEG_INFINITY),
            |(a_min, a_max), (b_min, b_max)| (a_min.min(b_min), a_max.max(b_max)),
        );

    let (min_d, max_d) = if min_d < max_d { (min_d, max_d) } else { (0.0, 100.0) };

    // --- Rayon 平行化像素 RGBA 轉換矩陣 ---
    let total_pixels = width * height;
    let mut depth_pixels = vec![0u8; total_pixels * 4];
    let mut unc_pixels = vec![0u8; total_pixels * 4];

    depth_pixels
        .par_chunks_exact_mut(4)
        .zip(depth_buffer.par_iter())
        .for_each(|(pixel, &d)| {
            if d == nodata_depth || d.is_nan() {
                pixel.copy_from_slice(&[0, 0, 0, 0]);
            } else {
                let norm = ((d - min_d) / (max_d - min_d + 0.0001)).clamp(0.0, 1.0);
                let r = ((1.0 - norm) * 255.0) as u8;
                let g = ((1.0 - (norm - 0.5).abs() * 2.0) * 255.0).max(0.0) as u8;
                let b = (norm * 255.0) as u8;
                pixel.copy_from_slice(&[r, g, b, 255]);
            }
        });

    unc_pixels
        .par_chunks_exact_mut(4)
        .zip(unc_buffer.par_iter())
        .for_each(|(pixel, &u)| {
            if u == nodata_unc || u.is_nan() {
                pixel.copy_from_slice(&[0, 0, 0, 0]);
            } else {
                let u_norm = (u / 2.0).clamp(0.0, 1.0);
                let ur = (u_norm * 255.0) as u8;
                let ug = ((1.0 - u_norm) * 255.0) as u8;
                pixel.copy_from_slice(&[ur, ug, 50, 255]);
            }
        });

    let depth_img = egui::ColorImage::from_rgba_unmultiplied([width, height], &depth_pixels);
    let unc_img = egui::ColorImage::from_rgba_unmultiplied([width, height], &unc_pixels);

    let depth_tex = ctx.load_texture("real_depth", depth_img, egui::TextureOptions::LINEAR);
    let unc_tex = ctx.load_texture("real_unc", unc_img, egui::TextureOptions::LINEAR);

    let preview = PreviewData {
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
        vertical_datum_code: vert_datum_code,
    };

    Ok((preview, res_options))
}

pub fn export_to_s102(
    input_path: &str,
    output_path: &str,
    target_res: f64,
    auto_align_depth: bool,
    fallback_epsg: i32,
    vertical_datum_code: i32,
) -> Result<String, String> {
    let open_opts = DatasetOptions {
        open_flags: GdalOpenFlags::GDAL_OF_RASTER,
        open_options: Some(&["MODE=RESAMPLED_GRID", "VALUE_POPULATION=MEAN"]),
        ..Default::default()
    };

    let dataset = Dataset::open_ex(input_path, open_opts)
        .map_err(|e| format!("開啟 BAG 檔案失敗: {}", e))?;

    let geo_transform = dataset
        .geo_transform()
        .map_err(|e| format!("無法讀取 GeoTransform: {}", e))?;

    let (orig_w, orig_h) = dataset.raster_size();

    let extent_x = geo_transform[1].abs() * orig_w as f64;
    let extent_y = geo_transform[5].abs() * orig_h as f64;
    let export_w = ((extent_x / target_res).round() as usize).max(1);
    let export_h = ((extent_y / target_res).round() as usize).max(1);

    let total_points = export_w.saturating_mul(export_h);
    let est_memory_mb = (total_points as f64 * 16.0) / (1024.0 * 1024.0);

    if est_memory_mb > 4096.0 {
        return Err(format!(
            "⚠️ 輸出限制：選取 {:.2}m 解析度將產生 {}×{} (約 {:.1} 億點) 網格，需 {:.1} MB 記憶體！請改選較粗解析度。",
            target_res, export_w, export_h, total_points as f64 / 1e8, est_memory_mb
        ));
    }

    let band_depth = dataset
        .rasterband(1)
        .map_err(|e| format!("讀取水深 Band 失敗: {}", e))?;
    let band_unc = dataset
        .rasterband(2)
        .map_err(|e| format!("讀取不確定性 Band 失敗: {}", e))?;

    let nodata_d = band_depth.no_data_value().unwrap_or(-1000000.0) as f32;
    let nodata_u = band_unc.no_data_value().unwrap_or(-1000000.0) as f32;

    let depth_data = band_depth
        .read_as::<f32>((0, 0), (orig_w, orig_h), (export_w, export_h), None)
        .map_err(|e| format!("降採樣水深數據失敗: {}", e))?
        .data()
        .to_vec();

    let unc_data = band_unc
        .read_as::<f32>((0, 0), (orig_w, orig_h), (export_w, export_h), None)
        .map_err(|e| format!("降採樣不確定性數據失敗: {}", e))?
        .data()
        .to_vec();

    // --- Rayon 平行建構 S102Point 矩陣並完成 Y 軸轉置 (西南角原點對齊) ---
    let mut points = vec![
        S102Point {
            depth: 1000000.0,
            uncertainty: 1000000.0
        };
        total_points
    ];

    points
        .par_chunks_exact_mut(export_w)
        .enumerate()
        .for_each(|(out_row_idx, row_slice)| {
            // S-102 原點在西南角 (Bottom-Up)，GDAL 讀取為西北角 (Top-Down)
            let source_row = export_h - 1 - out_row_idx;
            let row_offset = source_row * export_w;

            for col in 0..export_w {
                let i = row_offset + col;
                let mut d = depth_data[i];
                let u = unc_data[i];

                let is_d_invalid = d == nodata_d || d.is_nan();
                let is_u_invalid = u == nodata_u || u.is_nan();

                if !is_d_invalid && auto_align_depth {
                    d = -d;
                }

                row_slice[col] = S102Point {
                    depth: if is_d_invalid { 1000000.0 } else { d },
                    uncertainty: if is_u_invalid { 1000000.0 } else { u },
                };
            }
        });

    let min_x = geo_transform[0];
    let max_x = min_x + (export_w as f64 * target_res);
    let min_y = geo_transform[3] - (export_h as f64 * target_res);
    let max_y = geo_transform[3];
    let dx = target_res;
    let dy = target_res;

    let epsg = extract_horizontal_epsg(&dataset.projection());
    let final_epsg = if epsg > 0 { epsg } else { fallback_epsg };

    write_s102_hdf5_file(
        output_path,
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
        vertical_datum_code,
    )
    .map_err(|e| format!("寫入 S-102 HDF5 失敗: {}", e))?;

    Ok(format!(
        "🎉 轉檔成功！目標解析度: {:.2}m, 維度: {}x{}, 水平 EPSG: {} (QGIS請選擇此CRS)",
        target_res, export_w, export_h, final_epsg
    ))
}