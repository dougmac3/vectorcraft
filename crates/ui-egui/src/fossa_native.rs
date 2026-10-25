//! VectorCraft as a desk inside Fossa Direct's own window (`Desk`): in-process, drawn in a rect
//! the host gives it, never a window of its own. It mirrors the web bridge (`FOSSA_HOSTED`, the
//! short File menu): Fossa hands files in as a drop would, and Save and Export hand their files
//! back through [`Desk::take_saved`] instead of writing them to disk.

use std::sync::Arc;

use serde_json::json;
use vectorcraft_engine::Session;
use vectorcraft_engine::file_access;

use crate::{Inbox, Services, VectorcraftApp};

/// The salt every id the desk makes is under, so none meets the host's or another desk's.
const SALT: &str = "vectorcraft";

/// VectorCraft embedded in a host window.
pub struct Desk {
    app: VectorcraftApp,
    /// Files Save and Export wrote, waiting for the host.
    saved: Inbox,
    /// The editor's own style, put on while it runs and taken off after.
    style: Option<Arc<egui::Style>>,
    /// The focused widget is one of the desk's.
    focus_inside: bool,
    /// The focused widget when the desk last ran.
    focus_seen: Option<egui::Id>,
}

impl Desk {
    /// `render_state` is the host window's wgpu state. VectorCraft paints on the CPU, so it is
    /// not needed yet.
    pub fn new(ctx: &egui::Context, render_state: Option<&eframe::egui_wgpu::RenderState>) -> Self {
        let _ = (ctx, render_state);
        crate::FOSSA_HOSTED.store(true, std::sync::atomic::Ordering::Relaxed);
        let saved = Inbox::default();
        let out = saved.clone();
        let services = Services {
            // Saves and exports go back to Fossa, as the web bridge's downloads did: no dialogs, no folders.
            download: Some(Box::new(move |name: &str, bytes: &[u8]| {
                let name = vectorcraft_engine::cmd::fileio::file_name(name);
                out.lock().unwrap_or_else(|e| e.into_inner()).push((name, bytes.to_vec()));
                Ok(())
            })),
            inbox: Some(Inbox::default()),
            ..Default::default()
        };
        // Preferences, recovery folders, updates and the control server are the desktop app's;
        // the desk starts with none of them.
        let app = VectorcraftApp::new(Session::new(), services);
        Desk { app, saved, style: None, focus_inside: false, focus_seen: None }
    }

    /// Open a file Fossa hands in, as a drop would: a PNG or JPEG becomes a new document of its
    /// size with the picture on a locked template layer to trace over; anything else (SVG, PDF,
    /// a VectorCraft document) opens as artwork.
    pub fn open(&mut self, name: &str, bytes: Vec<u8>) {
        let raster = matches!(vectorcraft_engine::cmd::fileio::extension(name).as_str(), "png" | "jpg" | "jpeg");
        let r = if raster { self.trace_over(name, &bytes) } else { crate::io::open_bytes(&mut self.app, name, &bytes, None).map(|_| ()) };
        if let Err(e) = r {
            crate::io::report_open_error(&mut self.app, name, &e);
        }
    }

    fn trace_over(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let (w, h) = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| e.to_string())?
            .into_dimensions()
            .map_err(|e| e.to_string())?;
        let title = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
        self.app.run("file.new", json!({"name": title, "width": w, "height": h, "units": "Pixels"}))?;
        self.app.run(
            "file.place",
            json!({"name": name, "dataBase64": vectorcraft_format::base64_encode(bytes), "template": true, "rect": [0, 0, w, h]}),
        )?;
        Ok(())
    }

    /// Files saved or exported since the last call, (file name, bytes).
    pub fn take_saved(&mut self) -> Vec<(String, Vec<u8>)> {
        std::mem::take(&mut *self.saved.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// The active artboard as a PNG at the document's size (one pixel a point).
    pub fn grab_png(&mut self) -> Result<Vec<u8>, String> {
        let r = self.app.run("document.export", json!({"format": "png", "scale": 1}))?;
        r["dataBase64"].as_str().and_then(vectorcraft_format::base64_decode).ok_or_else(|| "the export returned no picture".into())
    }

    /// Run and draw the editor inside `ui`'s rect, one frame.
    pub fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let rect = ui.max_rect();
        let host_styles = (ctx.style_of(egui::Theme::Dark), ctx.style_of(egui::Theme::Light));
        if let Some(s) = &self.style {
            ctx.set_global_style(s.clone());
        }
        // Keys reach the desk only while the pointer is over it, focus is in it, or one of its
        // dialogs is open; otherwise they are set aside for the host.
        let focused = ctx.memory(|m| m.focused());
        if focused != self.focus_seen {
            self.focus_inside = false;
        }
        let over = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| rect.contains(p));
        let keys = !(over || self.focus_inside || self.app.ui.dialog.is_some());
        let held = keys.then(|| ctx.input_mut(take_keys));

        file_access::unconfined(|| self.app.logic(&ctx));
        let style = ctx.global_style();
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect).id_salt(SALT), |ui| {
            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
            ui.set_style(style.clone());
            file_access::unconfined(|| self.app.ui(ui));
        });

        if let Some(held) = held {
            ctx.input_mut(|i| give_keys(i, held));
        }
        let after = ctx.memory(|m| m.focused());
        if after != focused {
            self.focus_inside = after.is_some();
        }
        self.focus_seen = after;
        self.style = Some(ctx.global_style());
        ctx.set_style_of(egui::Theme::Dark, host_styles.0);
        ctx.set_style_of(egui::Theme::Light, host_styles.1);
    }

    /// The editor, for hosts and tests that need more than this API.
    pub fn app(&mut self) -> &mut VectorcraftApp {
        &mut self.app
    }
}

/// The keyboard's input this frame, set aside: (events, keys held).
type Keys = (Vec<egui::Event>, std::collections::HashSet<egui::Key>);

fn is_key(e: &egui::Event) -> bool {
    matches!(
        e,
        egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_) | egui::Event::Ime(_)
    )
}

fn take_keys(i: &mut egui::InputState) -> Keys {
    let (keys, rest) = std::mem::take(&mut i.events).into_iter().partition(is_key);
    i.events = rest;
    (keys, std::mem::take(&mut i.keys_down))
}

fn give_keys(i: &mut egui::InputState, (events, down): Keys) {
    i.events.extend(events);
    i.keys_down.extend(down);
}
