//! The Fossa Direct desk (`fossa_native::Desk`) offscreen, as Fossa hosts it: a 1280x720 PNG
//! opened to trace over, one 1600x1000 screenshot of the host window, and the artboard grabbed
//! back as a PNG of the document's size. Ignored by default:
//! `FOSSA_DESK_SHOT=shot.png cargo test --release -p vectorcraft-ui-egui --test fossa_desk -- --ignored`.
// Integration tests: unwrapping and panicking on failure is fine here, unlike in shipped code (AGENTS.md > Robustness).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use vectorcraft_ui_egui::fossa_native::Desk;

struct Host {
    desk: Desk,
}

impl eframe::App for Host {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let full = ui.max_rect();
        let strip = egui::Rect::from_min_max(full.min, egui::pos2(full.max.x, full.min.y + 36.0));
        ui.painter().rect_filled(strip, 0.0, egui::Color32::from_rgb(24, 22, 30));
        ui.scope_builder(egui::UiBuilder::new().max_rect(strip.shrink2(egui::vec2(12.0, 6.0))), |ui| ui.label("Fossa host strip"));
        let rest = egui::Rect::from_min_max(egui::pos2(full.min.x, strip.max.y), full.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(rest).id_salt("desk_vector"), |ui| self.desk.ui(ui, frame));
    }
}

/// A 1280x720 frame: a sky gradient over a dark ground with a sun, something to trace.
fn frame_png() -> Vec<u8> {
    let img = image::RgbaImage::from_fn(1280, 720, |x, y| {
        let (dx, dy) = (x as f32 - 900.0, y as f32 - 250.0);
        if dx * dx + dy * dy < 90.0 * 90.0 {
            image::Rgba([250, 200, 80, 255])
        } else if y > 480 {
            image::Rgba([40, 50, 40, 255])
        } else {
            image::Rgba([(60 + y / 6) as u8, (110 + y / 8) as u8, 200, 255])
        }
    });
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

#[test]
#[ignore]
fn desk_traces_a_frame_and_hands_it_back() {
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_accessibility_check(false).wgpu().build_eframe(|cc| {
        let mut desk = Desk::new(&cc.egui_ctx, cc.wgpu_render_state.as_ref());
        desk.open("frame.png", frame_png());
        Host { desk }
    });
    h.run_steps(6);
    if let Ok(path) = std::env::var("FOSSA_DESK_SHOT") {
        let img = h.render().unwrap();
        img.save(&path).unwrap();
    }
    let png = h.state_mut().desk.grab_png().unwrap();
    let back = image::load_from_memory(&png).unwrap();
    assert_eq!((back.width(), back.height()), (1280, 720));
    // The template layer is locked: the picture is there to trace, not to move.
    let app = h.state_mut().desk.app();
    let layers = &app.session.active().unwrap().doc.layers;
    assert!(layers.iter().any(|l| l.is_template() && l.locked), "no locked template layer");
}
