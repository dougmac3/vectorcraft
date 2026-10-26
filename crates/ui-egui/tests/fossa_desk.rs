//! The Fossa Direct desk (`fossa_native::Desk`) offscreen, as Fossa hosts it: a 1280x720 PNG
//! opened to trace over, the editor driven through its control channel in process, one panel
//! drawn alone, a dialog and a context menu kept inside the desk, and the grabs handed back.
//! Ignored by default:
//! `FOSSA_DESK_SHOT=shot.png FOSSA_PANEL_SHOT=panel.png cargo test --release -p vectorcraft-ui-egui --test fossa_desk -- --ignored`.
// Integration tests: unwrapping and panicking on failure is fine here, unlike in shipped code (AGENTS.md > Robustness).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use egui_kittest::kittest::Queryable as _;
use serde_json::json;
use vectorcraft_ui_egui::fossa_native::Desk;

/// The desk's rect in the 1600x1000 window: a host strip above it and one below.
const DESK: egui::Rect = egui::Rect { min: egui::pos2(0.0, 36.0), max: egui::pos2(1600.0, 900.0) };
/// Where a panel alone is drawn: 900x320 under the top strip.
const PANEL: egui::Rect = egui::Rect { min: egui::pos2(0.0, 36.0), max: egui::pos2(900.0, 356.0) };

struct Host {
    desk: Desk,
    /// Draw this panel alone instead of the whole desk.
    panel: Option<String>,
}

impl eframe::App for Host {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.desk.raw_input_hook(raw);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let full = ui.max_rect();
        for strip in [egui::Rect::from_min_max(full.min, egui::pos2(full.max.x, DESK.min.y)), egui::Rect::from_min_max(egui::pos2(full.min.x, DESK.max.y), full.max)] {
            ui.painter().rect_filled(strip, 0.0, egui::Color32::from_rgb(24, 22, 30));
            ui.scope_builder(egui::UiBuilder::new().max_rect(strip.shrink2(egui::vec2(12.0, 6.0))), |ui| ui.label("Fossa host strip"));
        }
        match self.panel.clone() {
            Some(panel) => ui.scope_builder(egui::UiBuilder::new().max_rect(PANEL).id_salt("desk_vector"), |ui| self.desk.panel_ui(&panel, ui, frame)),
            None => ui.scope_builder(egui::UiBuilder::new().max_rect(DESK).id_salt("desk_vector"), |ui| self.desk.ui(ui, frame)),
        };
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

/// The rects of the floating layers (dialogs, menus, popups) shown now.
fn floating(ctx: &egui::Context) -> Vec<egui::Rect> {
    ctx.memory(|m| {
        m.layer_ids()
            .filter(|l| matches!(l.order, egui::Order::Foreground | egui::Order::Tooltip) && m.areas().is_visible(l))
            .filter_map(|l| m.area_rect(l.id))
            .collect()
    })
}

fn decodes(png: &[u8]) -> (u32, u32) {
    let img = image::load_from_memory(png).unwrap();
    (img.width(), img.height())
}

#[test]
#[ignore]
fn desk_traces_a_frame_and_hands_it_back() {
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_accessibility_check(false).wgpu().build_eframe(|cc| {
        let mut desk = Desk::new(&cc.egui_ctx, cc.wgpu_render_state.as_ref());
        desk.open("frame.png", frame_png());
        Host { desk, panel: None }
    });
    h.run_steps(6);
    if let Ok(path) = std::env::var("FOSSA_DESK_SHOT") {
        h.render().unwrap().save(&path).unwrap();
    }
    let desk = &mut h.state_mut().desk;
    assert_eq!(decodes(&desk.grab_png().unwrap()), (1280, 720));
    // The template layer is locked: the picture is there to trace, not to move.
    let layers = &desk.app().session.active().unwrap().doc.layers;
    assert!(layers.iter().any(|l| l.is_template() && l.locked), "no locked template layer");
    let template = layers.iter().find(|l| l.is_template()).unwrap().id.0;

    // The control channel in process: a rectangle with a drop shadow, as an agent would add them.
    let made = desk.control("engine.execute", json!({"command": "shape.rectangle", "params": {"x": 200, "y": 380, "width": 420, "height": 220}})).unwrap();
    let rect = made["id"].as_u64().unwrap();
    desk.control("engine.execute", json!({"command": "select.set", "params": {"ids": [rect]}})).unwrap();
    desk.control("engine.execute", json!({"command": "effect.apply", "params": {"effect": "stylize.dropShadow"}})).unwrap();
    let doc = desk.control("document.inspect", json!({})).unwrap();
    assert!(doc["history"].to_string().contains("Drop Shadow"), "no shadow: {doc}");
    assert!(desk.control("engine.execute", json!({"command": "no.such"})).is_err());

    // The commands, typed where their descriptions say.
    let commands = desk.commands();
    let params: Vec<&serde_json::Value> = commands.as_array().unwrap().iter().flat_map(|c| c["typedParams"].as_array().unwrap()).collect();
    let typed = params.iter().filter(|p| !p["kind"].is_null()).count();
    println!("commands: {}, params: {} typed, {} untyped", commands.as_array().unwrap().len(), typed, params.len() - typed);
    println!("panels: {}", desk.panels().join(", "));
    assert!(typed > params.len() / 2, "{typed} of {}", params.len());

    // Grabs: the selection, a layer, the art over its frame.
    assert_eq!(decodes(&desk.grab_selection_png().unwrap()), (1280, 720));
    assert_eq!(decodes(&desk.grab_layer_png(template).unwrap()), (1280, 720));
    assert_eq!(decodes(&desk.grab_png_with_frame().unwrap()), (1280, 720));
    desk.control("engine.execute", json!({"command": "fossa.addToShot", "params": {"id": rect}})).unwrap();
    let shots = desk.take_shot_requests();
    assert_eq!(shots.len(), 1);
    assert_eq!(decodes(&shots[0].1), (1280, 720));
    desk.control("engine.execute", json!({"command": "select.none"})).unwrap();
    assert!(desk.grab_selection_png().is_err(), "nothing selected, nothing to grab");
    desk.control("engine.execute", json!({"command": "select.set", "params": {"ids": [rect]}})).unwrap();

    // A dialog: it opens inside the desk, takes a click, and closes.
    desk.control("engine.execute", json!({"command": "file.documentSetup"})).unwrap();
    h.run_steps(4);
    assert!(h.state_mut().desk.app().ui.dialog.is_some(), "no dialog");
    let shown = floating(&h.ctx);
    assert!(!shown.is_empty(), "the dialog drew no window");
    for r in &shown {
        assert!(DESK.expand(0.5).contains_rect(*r), "dialog {r:?} outside the desk {DESK:?}");
    }
    h.get_by_label("Cancel").click();
    h.run_steps(4);
    assert!(h.state_mut().desk.app().ui.dialog.is_none(), "Cancel didn't close the dialog");

    // A context menu opened near the desk's bottom edge stays inside it.
    let at = egui::pos2(800.0, DESK.max.y - 120.0);
    let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed, modifiers: Default::default() };
    h.hover_at(at);
    h.run_steps(1);
    h.event(button(true));
    h.run_steps(1);
    h.event(button(false));
    h.run_steps(6);
    let menus = floating(&h.ctx);
    assert!(!menus.is_empty(), "no context menu");
    for r in &menus {
        assert!(DESK.expand(0.5).contains_rect(*r), "menu {r:?} outside the desk {DESK:?}");
    }
    if let Ok(path) = std::env::var("FOSSA_MENU_SHOT") {
        h.render().unwrap().save(&path).unwrap();
    }
    h.key_press(egui::Key::Escape);
    h.run_steps(3);

    // One panel alone, 900x320: the Appearance of the rectangle, its shadow in it.
    h.state_mut().desk.focus(json!({"select": [rect]})).unwrap();
    h.state_mut().panel = Some("appearance".into());
    h.run_steps(6);
    if let Ok(path) = std::env::var("FOSSA_PANEL_SHOT") {
        let img = h.render().unwrap();
        let s = h.ctx.pixels_per_point();
        let cut = image::imageops::crop_imm(&img, (PANEL.min.x * s) as u32, (PANEL.min.y * s) as u32, (PANEL.width() * s) as u32, (PANEL.height() * s) as u32);
        cut.to_image().save(&path).unwrap();
    }
    assert!(h.state_mut().desk.panels().iter().any(|p| p == "appearance"));
}
