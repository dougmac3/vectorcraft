//! Help → About VectorCraft.

use crate::VectorcraftApp;
use crate::theme;

/// The About window, while `UiState::about` is set.
pub(super) fn show(app: &mut VectorcraftApp, ctx: &egui::Context) {
    if !app.ui.about {
        return;
    }
    let mut open = true;
    egui::Window::new(tl!("About VectorCraft"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .frame(egui::Frame::window(&ctx.global_style()).inner_margin(egui::Margin::same(18)))
        .show(ctx, |ui| {
            // Tabs: About · Contributors · Models (the credits are compiled in, see `credits`).
            let tab_id = egui::Id::new("about_tab");
            let mut tab = ui.data_mut(|d| d.get_temp::<u8>(tab_id)).unwrap_or(0);
            // Inside Fossa: the About tab alone (the credits link to GitHub profiles).
            let hosted = crate::fossa_hosted();
            if hosted {
                tab = 0;
            } else {
                ui.horizontal(|ui| {
                for (i, label) in [tl!("About"), tl!("Contributors"), tl!("Models")].into_iter().enumerate() {
                    let i = i as u8;
                    if ui.selectable_label(tab == i, label).clicked() {
                        tab = i;
                    }
                }
                });
                ui.separator();
            }
            ui.data_mut(|d| d.insert_temp(tab_id, tab));
            match tab {
                1 => {
                    ui.set_width(660.0);
                    crate::credits::contributors_ui(app, ui);
                }
                2 => {
                    ui.set_width(660.0);
                    crate::credits::models_ui(ui);
                }
                _ => about_tab(app, ui),
            }
        });
    app.ui.about = open;
}

/// The About tab: mark, version, community links, licences and the GPU in use.
fn about_tab(app: &mut VectorcraftApp, ui: &mut egui::Ui) {
    ui.set_width(380.0);
    let hosted = crate::fossa_hosted();
    ui.horizontal(|ui| {
        if !hosted {
            let (r, _) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
            crate::brand::paint_mark(ui, r);
        }
        ui.vertical(|ui| {
            ui.label(egui::RichText::new("VectorCraft").font(theme::semibold(22.0)));
            ui.label(crate::i18n::fmt(
                tl!("Version {version} — open-source vector illustration in pure Rust."),
                &[("version", env!("CARGO_PKG_VERSION"))],
            ));
        });
    });
    ui.add_space(12.0);
    if hosted {
        // No ArtCraft name or community links inside Fossa; the licences stay.
        ui.label(
            egui::RichText::new("MIT OR Apache-2.0. Fonts: Source Sans 3, Inter, JetBrains Mono (OFL). Icons: Lucide (ISC) + VectorCraft.").size(11.0),
        );
    } else {
        crate::community::links(app, ui);
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new(tl!(
                "Part of ArtCraft. MIT OR Apache-2.0. Fonts: Source Sans 3, Inter, JetBrains Mono (OFL). Icons: Lucide (ISC) + VectorCraft."
            ))
            .size(11.0),
        );
    }
    // The GPU the window renders with, for bug reports (Preferences › Performance picks it).
    if let Some(adapter) = &app.graphics_adapter {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Graphics: {adapter}"), &[("adapter", adapter.as_str())])).size(11.0));
    }
}
