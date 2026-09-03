use eframe::egui::TextureHandle;
use hdf5::H5Type;

#[repr(C)]
#[derive(H5Type, Clone, Copy, Debug)]
pub struct S102Point {
    pub depth: f32,
    pub uncertainty: f32,
}

#[derive(PartialEq, Clone, Copy)]
pub enum ViewMode {
    Depth,
    Uncertainty,
}

pub struct PreviewData {
    pub depth_texture: Option<TextureHandle>,
    pub uncertainty_texture: Option<TextureHandle>,
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
            
            vertical_datum_code: 12,
        }
    }
}