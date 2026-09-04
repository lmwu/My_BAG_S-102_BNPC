use eframe::egui::Pos2;

pub type BnpcContour = Vec<Pos2>;

#[inline]
fn is_invalid(v: f32) -> bool {
    v.is_nan() || v.abs() > 8000.0
}

/// 高速度 Marching Squares 算繪
pub fn generate_bnpc_contours(
    grid: &[f32],
    width: usize,
    height: usize,
    target_depth: f32,
    smoothing_iterations: usize,
) -> Vec<BnpcContour> {
    if width < 2 || height < 2 || grid.len() < width * height {
        return vec![];
    }

    let mut segments = Vec::new();

    // 1. 快速 Marching Squares 線段抽取
    for row in 0..height - 1 {
        for col in 0..width - 1 {
            let i0 = row * width + col;
            let i1 = row * width + (col + 1);
            let i2 = (row + 1) * width + (col + 1);
            let i3 = (row + 1) * width + col;

            let v0 = grid[i0];
            let v1 = grid[i1];
            let v2 = grid[i2];
            let v3 = grid[i3];

            if is_invalid(v0) || is_invalid(v1) || is_invalid(v2) || is_invalid(v3) {
                continue;
            }

            let case_key = ((v0 >= target_depth) as u8)
                | (((v1 >= target_depth) as u8) << 1)
                | (((v2 >= target_depth) as u8) << 2)
                | (((v3 >= target_depth) as u8) << 3);

            if case_key == 0 || case_key == 15 {
                continue;
            }

            let p_top = interpolate(col as f32, row as f32, col as f32 + 1.0, row as f32, v0, v1, target_depth);
            let p_right = interpolate(col as f32 + 1.0, row as f32, col as f32 + 1.0, row as f32 + 1.0, v1, v2, target_depth);
            let p_bottom = interpolate(col as f32, row as f32 + 1.0, col as f32 + 1.0, row as f32 + 1.0, v3, v2, target_depth);
            let p_left = interpolate(col as f32, row as f32, col as f32, row as f32 + 1.0, v0, v3, target_depth);

            match case_key {
                1 | 14 => segments.push((p_left, p_top)),
                2 | 13 => segments.push((p_top, p_right)),
                3 | 12 => segments.push((p_left, p_right)),
                4 | 11 => segments.push((p_right, p_bottom)),
                5 => { segments.push((p_left, p_top)); segments.push((p_right, p_bottom)); }
                6 | 9 => segments.push((p_top, p_bottom)),
                7 | 8 => segments.push((p_left, p_bottom)),
                10 => { segments.push((p_top, p_right)); segments.push((p_left, p_bottom)); }
                _ => {}
            }
        }
    }

    // 2. 線段快速首尾串聯
    let polylines = stitch_fast(segments);

    // 3. 切角平滑化
    polylines
        .into_iter()
        .map(|line| {
            let mut smoothed = line;
            for _ in 0..smoothing_iterations {
                smoothed = chaikin_smooth(smoothed);
            }
            smoothed
        })
        .collect()
}

#[inline]
fn interpolate(x1: f32, y1: f32, x2: f32, y2: f32, v1: f32, v2: f32, target: f32) -> Pos2 {
    let t = if (v2 - v1).abs() < 1e-5 { 0.5 } else { (target - v1) / (v2 - v1) }.clamp(0.0, 1.0);
    Pos2::new(x1 + t * (x2 - x1), y1 + t * (y2 - y1))
}

fn stitch_fast(mut segments: Vec<(Pos2, Pos2)>) -> Vec<BnpcContour> {
    let mut polylines = Vec::new();
    while let Some((p1, p2)) = segments.pop() {
        let mut line = vec![p1, p2];
        let mut expanded = true;

        while expanded {
            expanded = false;
            let tail = *line.last().unwrap();
            
            // 尋找可連接的尾端
            if let Some(idx) = segments.iter().position(|(s1, s2)| {
                (s1.x - tail.x).abs() < 1e-3 && (s1.y - tail.y).abs() < 1e-3
                    || (s2.x - tail.x).abs() < 1e-3 && (s2.y - tail.y).abs() < 1e-3
            }) {
                let (s1, s2) = segments.swap_remove(idx);
                if (s1.x - tail.x).abs() < 1e-3 && (s1.y - tail.y).abs() < 1e-3 {
                    line.push(s2);
                } else {
                    line.push(s1);
                }
                expanded = true;
            }
        }
        polylines.push(line);
    }
    polylines
}

fn chaikin_smooth(points: BnpcContour) -> BnpcContour {
    if points.len() < 3 {
        return points;
    }
    let mut smoothed = Vec::with_capacity(points.len() * 2);
    smoothed.push(points[0]);
    for window in points.windows(2) {
        let p0 = window[0];
        let p1 = window[1];
        smoothed.push(Pos2::new(0.75 * p0.x + 0.25 * p1.x, 0.75 * p0.y + 0.25 * p1.y));
        smoothed.push(Pos2::new(0.25 * p0.x + 0.75 * p1.x, 0.25 * p0.y + 0.75 * p1.y));
    }
    smoothed.push(*points.last().unwrap());
    smoothed
}