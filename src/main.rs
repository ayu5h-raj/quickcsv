//! QuickCSV Native Entry Point

use eframe::egui;
use quickcsv::FastCsvApp;

fn load_icon() -> egui::IconData {
    let (icon_rgba, icon_width, icon_height) = {
        let icon_bytes = include_bytes!("../icons/icon-128.png");
        let image = image::load_from_memory(icon_bytes)
            .expect("Failed to load icon")
            .into_rgba8();
        let (width, height) = image.dimensions();
        (image.into_vec(), width, height)
    };

    egui::IconData {
        rgba: icon_rgba,
        width: icon_width,
        height: icon_height,
    }
}

fn main() -> eframe::Result<()> {
    let icon = load_icon();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([600.0, 400.0])
            .with_title("QuickCSV")
            .with_icon(icon)
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "QuickCSV",
        options,
        Box::new(|cc| {
            // Add Phosphor icons to fonts
            let mut fonts = egui::FontDefinitions::default();
            egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
            cc.egui_ctx.set_fonts(fonts);

            // Follow system theme
            cc.egui_ctx.set_visuals(egui::Visuals::dark());

            Ok(Box::new(FastCsvApp::default()))
        }),
    )
}
