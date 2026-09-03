mod app;
mod io;
mod models;
mod utils;

use app::VrBagApp;
use eframe::egui;

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