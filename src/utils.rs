use gdal::spatial_ref::SpatialRef;

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

    // 2. 後備機制 (Fallback)：裁切掉 VERT_CS 區塊
    let horiz_wkt = if let Some(v_pos) = spatial_wkt.find("VERT_CS[") {
        &spatial_wkt[..v_pos]
    } else {
        spatial_wkt
    };

    // 3. 由後往前搜尋最外層 PROJCS/GEOGCS 的 EPSG 代碼
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