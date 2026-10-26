//! A stand-in for Fossa Direct's window: a host strip on top and VectorCraft's desk in the rest
//! (`cargo run --release -p vectorcraft-ui-egui --example fossa_host [picture.png]`). "Add to
//! shot" grabs the artboard as a PNG; files the desk saves are listed in the strip.
// An example: unwrapping is fine here, unlike in shipped code (AGENTS.md > Robustness).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use vectorcraft_ui_egui::fossa_native::Desk;

pub struct Host {
    pub desk: Desk,
    pub note: String,
}

impl Host {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Host { desk: Desk::new(&cc.egui_ctx, cc.wgpu_render_state.as_ref()), note: "Fossa host strip".into() }
    }
}

impl eframe::App for Host {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.desk.raw_input_hook(raw);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let full = ui.max_rect();
        let strip = egui::Rect::from_min_max(full.min, egui::pos2(full.max.x, full.min.y + 36.0));
        ui.painter().rect_filled(strip, 0.0, egui::Color32::from_rgb(24, 22, 30));
        ui.scope_builder(egui::UiBuilder::new().max_rect(strip.shrink2(egui::vec2(12.0, 6.0))).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
            ui.label(egui::RichText::new("Fossa").strong());
            if ui.button("Add to shot").clicked() {
                self.note = match self.desk.grab_png() {
                    Ok(png) => format!("grabbed {} bytes", png.len()),
                    Err(e) => e,
                };
            }
            for (name, bytes) in self.desk.take_shot_requests() {
                self.note = format!("added {name} to the shot ({} bytes)", bytes.len());
            }
            for (name, bytes) in self.desk.take_saved() {
                self.note = format!("saved {name} ({} bytes)", bytes.len());
            }
            ui.label(&self.note);
        });
        let rest = egui::Rect::from_min_max(egui::pos2(full.min.x, strip.max.y), full.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(rest).id_salt("desk_vector"), |ui| self.desk.ui(ui, frame));
    }
}

fn main() -> eframe::Result {
    let picture = std::env::args().nth(1);
    let options = eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_inner_size([1600.0, 1000.0]), ..Default::default() };
    eframe::run_native(
        "Fossa host",
        options,
        Box::new(move |cc| {
            let mut host = Host::new(cc);
            if let Some(p) = picture {
                let name = std::path::Path::new(&p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                host.desk.open(&name, std::fs::read(&p)?);
            }
            Ok(Box::new(host))
        }),
    )
}
