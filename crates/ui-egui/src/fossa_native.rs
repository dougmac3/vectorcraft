//! VectorCraft as a desk inside Fossa Direct's own window (`Desk`): in-process, drawn in a rect
//! the host gives it, never a window of its own. It mirrors the web bridge (`FOSSA_HOSTED`, the
//! short File menu): Fossa hands files in as a drop would, and Save and Export hand their files
//! back through [`Desk::take_saved`] instead of writing them to disk. Open, Place and the folder
//! pickers show the system's file dialogs on a thread of their own, so Fossa's window keeps running.
//!
//! The host drives the editor the way an agent does through the control channel ([`Desk::control`],
//! the methods of [`crate::control`], in process), and can draw one of its panels alone
//! ([`Desk::panel_ui`]) narrowed to what it is about ([`Desk::focus`]). While the desk draws, egui's
//! screen is the desk's rect: dialogs, menus, tooltips and their backdrops stay inside it.

use std::sync::Arc;

use serde_json::{Value, json};
use vectorcraft_doc::{Document, Node, NodeId};
use vectorcraft_engine::Session;
use vectorcraft_engine::file_access;

use crate::control::{self, ControlRequest, Outcome};
use crate::picks::PickRequest;
use crate::{FilePick, Inbox, Services, VectorcraftApp};

/// The salt every id the desk makes is under, so none meets the host's or another desk's.
const SALT: &str = "vectorcraft";

/// VectorCraft embedded in a host window.
pub struct Desk {
    app: VectorcraftApp,
    /// The host window's context, for control requests between frames.
    ctx: egui::Context,
    /// Files Save and Export wrote, waiting for the host.
    saved: Inbox,
    /// The editor's own style, put on while it runs and taken off after.
    style: Option<Arc<egui::Style>>,
    /// The focused widget is one of the desk's.
    focus_inside: bool,
    /// The focused widget when the desk last ran.
    focus_seen: Option<egui::Id>,
    /// The pass the editor's per-frame work last ran in: a frame that draws the desk and a panel
    /// of it runs that work once.
    logic_pass: Option<u64>,
}

impl Desk {
    /// `render_state` is the host window's wgpu state. VectorCraft paints on the CPU, so it is
    /// not needed yet.
    pub fn new(ctx: &egui::Context, render_state: Option<&eframe::egui_wgpu::RenderState>) -> Self {
        let _ = render_state;
        crate::FOSSA_HOSTED.store(true, std::sync::atomic::Ordering::Relaxed);
        let saved = Inbox::default();
        let out = saved.clone();
        let wake = ctx.clone();
        let services = Services {
            // Saves and exports go back to Fossa, as the web bridge's downloads did: no dialogs, no folders.
            download: Some(Box::new(move |name: &str, bytes: &[u8]| {
                let name = vectorcraft_engine::cmd::fileio::file_name(name);
                out.lock().unwrap_or_else(|e| e.into_inner()).push((name, bytes.to_vec()));
                Ok(())
            })),
            inbox: Some(Inbox::default()),
            // Open, Place and the folder pickers read the files the person picks.
            read: Some(Box::new(|p: &str| std::fs::read(p).map_err(|e| e.to_string()))),
            exists: Some(Box::new(|p: &str| std::path::Path::new(p).exists())),
            // The dialogs show off the UI thread only ([`start_pick`]): one asked for in line
            // would hold the host's window, so in line they pick nothing.
            pick_open: Some(Box::new(|_: &FilePick| None)),
            pick_open_multi: Some(Box::new(Vec::new)),
            pick_folder: Some(Box::new(|| None)),
            start_pick: Some(Box::new(move |request: PickRequest| start_pick(request, &wake))),
            ..Default::default()
        };
        // Preferences, recovery folders, updates and the control server are the desktop app's;
        // the desk starts with none of them.
        let app = VectorcraftApp::new(Session::new(), services);
        Desk { app, ctx: ctx.clone(), saved, style: None, focus_inside: false, focus_seen: None, logic_pass: None }
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

    /// Pictures the person asked to add to the shot since the last call (the Layers panel's Add
    /// to shot), (name, PNG).
    pub fn take_shot_requests(&mut self) -> Vec<(String, Vec<u8>)> {
        std::mem::take(&mut self.app.shot_requests)
    }

    /// The active artboard as a PNG at the document's size (one pixel a point), without the
    /// template layers (the picture traced over).
    pub fn grab_png(&mut self) -> Result<Vec<u8>, String> {
        export_png(&mut self.app, json!({"format": "png", "scale": 1}))
    }

    /// [`Self::grab_png`] with the template layers drawn too: the art over the frame it was traced on.
    pub fn grab_png_with_frame(&mut self) -> Result<Vec<u8>, String> {
        let doc = self.app.session.active().map(|st| st.doc.clone()).ok_or("no document open")?;
        render_png(&mut self.app, &doc)
    }

    /// The selected objects alone, where they are on the artboard, as a PNG of the artboard's
    /// size. An error when nothing is selected.
    pub fn grab_selection_png(&mut self) -> Result<Vec<u8>, String> {
        if crate::panels::selection_len(&self.app) == 0 {
            return Err("nothing is selected".into());
        }
        export_png(&mut self.app, json!({"format": "png", "scale": 1, "selectedOnly": true}))
    }

    /// Layer (or sublayer, group, object) `layer_id` alone, shown even when it is hidden, as a PNG
    /// of the artboard's size.
    pub fn grab_layer_png(&mut self, layer_id: u64) -> Result<Vec<u8>, String> {
        layer_png(&mut self.app, layer_id)
    }

    /// Run control method `method` (those of [`crate::control`]: `engine.execute`,
    /// `engine.commands`, `ui.set`, `ui.dialog.set`, `ui.click`...) on the desk's editor, as the
    /// control channel does, in process -> its result, or its error. Also `desk.panels` and
    /// `desk.focus {target}` ([`Self::panels`], [`Self::focus`]). Between frames or inside one.
    pub fn control(&mut self, method: &str, params: Value) -> Result<Value, String> {
        match method {
            "engine.commands" => return Ok(self.commands()),
            "desk.panels" => return Ok(json!(self.panels())),
            "desk.focus" => return self.focus(params.get("target").cloned().unwrap_or(params)).map(|()| Value::Null),
            _ => {}
        }
        let (req, _reply) = ControlRequest::new(method, params);
        let (app, ctx) = (&mut self.app, &self.ctx);
        app.ui.answering_control = true;
        let r = file_access::unconfined(|| vectorcraft_engine::guard::catch_panic(|| control::handle(app, ctx, &req)));
        app.ui.answering_control = false;
        ctx.request_repaint();
        match r {
            Err(msg) => {
                app.picks.recover();
                Err(format!("internal error: {msg}"))
            }
            Ok(Outcome::Done(mut v)) if v["ok"] == true => Ok(v["result"].take()),
            Ok(Outcome::Done(v)) => Err(v["error"].as_str().unwrap_or("failed").to_string()),
            Ok(Outcome::Screenshot { .. }) => Err("ui.screenshot captures a window of its own: use ui.render or grab_png".into()),
        }
    }

    /// `engine.commands`: every engine and UI command with whether it is enabled, its params
    /// description (`params`), and `typedParams`, each parameter that description names as
    /// `{id, optional, kind, default?, min?, max?, choices?, note}`. `kind` is bool, number,
    /// string, id, choice, array or object as far as the description says, else null (untyped:
    /// `note` is all there is).
    pub fn commands(&self) -> Value {
        let mut list = control::all_commands(&self.app);
        for c in list.as_array_mut().into_iter().flatten() {
            let typed = typed_params(c["params"].as_str().unwrap_or_default());
            c["typedParams"] = Value::Array(typed);
        }
        list
    }

    /// The panels [`Self::panel_ui`] draws: the ids `window.panel` takes.
    pub fn panels(&self) -> Vec<String> {
        crate::state::all_panels().map(|(id, _)| id.to_string()).collect()
    }

    /// Narrow what the panels show to `target`, by selection and view, each key optional:
    /// `select: [ids]` (those objects selected), `layer: id` (that layer current, its row shown in
    /// the Layers panel), `item: index` (that fill, stroke or effect row of the selection's
    /// appearance targeted: the Appearance, Stroke and effect controls edit it), `artboard: index`,
    /// `zoom: percent` and `center: [x, y]` or `rect: [x, y, width, height]` (the view fitted to
    /// it), `tool: id`. Applied in that order; the first that fails stops it.
    pub fn focus(&mut self, target: Value) -> Result<(), String> {
        let app = &mut self.app;
        if let Some(ids) = target.get("select") {
            app.run("select.set", json!({ "ids": ids }))?;
        }
        if let Some(id) = target.get("layer") {
            app.run("layer.setCurrent", json!({ "id": id }))?;
            app.run("layer.locate", json!({ "id": id }))?;
        }
        if let Some(index) = target.get("item") {
            app.run("appearance.setActiveItem", json!({ "index": index }))?;
        }
        if let Some(index) = target.get("artboard") {
            app.run("view.goToArtboard", json!({ "index": index }))?;
        }
        if let Some(r) = target.get("rect").and_then(Value::as_array) {
            let n = |i: usize| r.get(i).and_then(Value::as_f64).unwrap_or(0.0);
            let (w, h) = (n(2).max(1.0), n(3).max(1.0));
            // The canvas as last drawn (the desk's whole rect before it ever drew).
            let room = app.canvas_rect.map_or((800.0, 600.0), |c| (f64::from(c.width()), f64::from(c.height())));
            let zoom = (room.0 / w).min(room.1 / h) * 90.0;
            app.run("view.setZoom", json!({ "zoom": zoom, "center": [n(0) + w / 2.0, n(1) + h / 2.0] }))?;
        } else if let Some(zoom) = target.get("zoom") {
            let mut p = json!({ "zoom": zoom });
            if let Some(c) = target.get("center") {
                p["center"] = c.clone();
            }
            app.run("view.setZoom", p)?;
        }
        if let Some(tool) = target.get("tool") {
            app.run("tool.select", json!({ "tool": tool }))?;
        }
        Ok(())
    }

    /// Run and draw the editor inside `ui`'s rect, one frame.
    pub fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame(ui, |app, ui| app.ui(ui));
    }

    /// Run the editor and draw panel `panel` alone (one of [`Self::panels`]) inside `ui`'s rect,
    /// one frame, with the dialogs it opens kept inside that rect.
    pub fn panel_ui(&mut self, panel: &str, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let Some(id) = crate::menus::normalize_panel(panel) else {
            log::warn!("no panel {panel}");
            return;
        };
        self.frame(ui, |app, ui| app.panel_ui(ui, id));
    }

    /// One frame of the editor in `ui`'s rect, drawn by `draw`, under the desk's style, ids and
    /// keyboard rules, with egui's screen the rect.
    fn frame(&mut self, ui: &mut egui::Ui, draw: impl FnOnce(&mut VectorcraftApp, &mut egui::Ui)) {
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
        let host_screen = confine_screen(&ctx, rect);

        let pass = ctx.cumulative_pass_nr();
        if self.logic_pass != Some(pass) {
            self.logic_pass = Some(pass);
            file_access::unconfined(|| self.app.logic(&ctx));
        }
        let style = ctx.global_style();
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect).id_salt(SALT), |ui| {
            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
            ui.set_style(style.clone());
            file_access::unconfined(|| draw(&mut self.app, ui));
        });

        restore_screen(&ctx, host_screen);
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

    /// Call from the host's `eframe::App::raw_input_hook`: the input control requests make
    /// (`ui.click`, `ui.drag`, `ui.key`, `ui.text`, `ui.wheel`) goes into the host's next pass, a
    /// step a frame, as real input does.
    pub fn raw_input_hook(&mut self, raw: &mut egui::RawInput) {
        self.app.raw_input_hook(raw);
    }

    /// The editor, for hosts and tests that need more than this API.
    pub fn app(&mut self) -> &mut VectorcraftApp {
        &mut self.app
    }
}

/// The Layers panel's Add to shot (`fossa.addToShot {id}`): the row's art queued for
/// [`Desk::take_shot_requests`] -> `{name}`.
pub(crate) fn add_to_shot(app: &mut VectorcraftApp, p: &Value) -> Result<Value, String> {
    let id = p.get("id").and_then(Value::as_u64).ok_or("missing `id`")?;
    let png = layer_png(app, id)?;
    let label = app.session.active().and_then(|st| st.doc.node(NodeId(id))).and_then(|n| n.name.clone()).unwrap_or_default();
    let stem: String = label.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    let name = format!("{}.png", if stem.trim_matches('_').is_empty() { "layer" } else { stem.trim_matches('_') });
    app.shot_requests.push((name.clone(), png));
    app.status("Added to the shot");
    Ok(json!({ "name": name }))
}

/// `document.export`'s PNG with `params`.
fn export_png(app: &mut VectorcraftApp, params: Value) -> Result<Vec<u8>, String> {
    let r = app.run("document.export", params)?;
    r["dataBase64"].as_str().and_then(vectorcraft_format::base64_decode).ok_or_else(|| "the export returned no picture".into())
}

/// Node `id` of the active document alone (its layers around it, shown), as a PNG of the first
/// artboard.
fn layer_png(app: &mut VectorcraftApp, id: u64) -> Result<Vec<u8>, String> {
    let st = app.session.active().ok_or("no document open")?;
    let id = NodeId(id);
    st.doc.node(id).ok_or("no such layer")?;
    let mut doc = Document::clone(&st.doc);
    doc.layers = st.doc.layers.iter().filter_map(|l| only(l, id)).map(Arc::new).collect();
    render_png(app, &doc)
}

/// `n` with only node `id` in it (whole), shown, `None` when it isn't in `n`.
fn only(n: &Node, id: NodeId) -> Option<Node> {
    let mut out = if n.id == id {
        n.clone()
    } else {
        let kept: Vec<Arc<Node>> = n.children()?.iter().filter_map(|c| only(c, id).map(Arc::new)).collect();
        if kept.is_empty() {
            return None;
        }
        let mut out = n.clone();
        *out.children_mut()? = kept;
        out
    };
    out.visible = true;
    Some(out)
}

/// `doc`'s first artboard as a PNG, one pixel a point, template layers drawn, on transparency.
fn render_png(app: &mut VectorcraftApp, doc: &Document) -> Result<Vec<u8>, String> {
    let rect = doc.artboards.first().map(|a| a.rect).ok_or("the document has no artboards")?;
    vectorcraft_render::raster_size(rect, 1.0)?;
    let opts = vectorcraft_render::RenderOptions { skip_templates: false, ..Default::default() };
    app.canvas.renderer.render_region_with(doc, rect, 1.0, &opts).to_png()
}

/// Show `request`'s file dialog on a thread of its own, answering on the receiver and waking the
/// host's window. macOS shows its panels on the main thread only: none there.
fn start_pick(request: PickRequest, wake: &egui::Context) -> Option<std::sync::mpsc::Receiver<Vec<String>>> {
    if cfg!(target_os = "macos") {
        return None;
    }
    // Set up here: the filter names are translated in the UI thread's language.
    let dialog = file_dialog(&request);
    let (tx, rx) = std::sync::mpsc::channel();
    let wake = wake.clone();
    std::thread::Builder::new()
        .name("vectorcraft-file-dialog".into())
        .spawn(move || {
            let picked: Vec<std::path::PathBuf> = match request {
                PickRequest::Open(_) => dialog.pick_file().into_iter().collect(),
                PickRequest::OpenMany => dialog.pick_files().unwrap_or_default(),
                PickRequest::Save(_) => dialog.save_file().into_iter().collect(),
                PickRequest::Folder => dialog.pick_folder().into_iter().collect(),
            };
            // The desk gone meanwhile has no use for the answer.
            let _ = tx.send(picked.into_iter().map(|p| p.to_string_lossy().into_owned()).collect());
            wake.request_repaint();
        })
        .ok()?;
    Some(rx)
}

/// The dialog for `request`: its file types, folder and suggested name, as the desktop app's.
fn file_dialog(request: &PickRequest) -> rfd::FileDialog {
    let lang = crate::i18n::current();
    let name = |n: &str| crate::i18n::tr_ctx(lang, "file-filter", n).to_string();
    let pick = match request {
        PickRequest::Open(pick) | PickRequest::Save(pick) => pick,
        PickRequest::OpenMany => {
            return vectorcraft_engine::cmd::fileio::place_filters()
                .fold(rfd::FileDialog::new().set_title(crate::i18n::t("Place")), |d, (n, exts)| d.add_filter(name(n), exts));
        }
        PickRequest::Folder => return rfd::FileDialog::new(),
    };
    let mut d = pick.filters.iter().fold(rfd::FileDialog::new(), |d, (n, exts)| d.add_filter(name(n), exts));
    if let Some(folder) = &pick.folder {
        d = d.set_directory(folder);
    }
    if !pick.name.is_empty() {
        d = d.set_file_name(&pick.name);
    }
    d
}

/// Make egui's screen `rect` for what draws next (Areas, Windows, popups, menus and tooltips keep
/// to it, modal backdrops cover it, dialogs centre in it) -> the host's input state, for
/// [`restore_screen`]. egui reads its screen from the pass's input; this input is the pass's in
/// all else.
fn confine_screen(ctx: &egui::Context, rect: egui::Rect) -> egui::InputState {
    let options = ctx.options(|o| o.input_options.clone());
    ctx.input_mut(|i| {
        let host = i.clone();
        let raw = egui::RawInput {
            screen_rect: Some(rect),
            safe_area_insets: Some(egui::SafeAreaInsets::default()),
            time: Some(host.time),
            predicted_dt: host.predicted_dt,
            max_texture_side: Some(host.max_texture_side),
            focused: host.focused,
            // This pass's pinch and rotation, which the new state would otherwise lose.
            events: vec![egui::Event::Zoom(host.zoom_delta()), egui::Event::Rotate(host.rotation_delta())],
            ..Default::default()
        };
        let mut desk = host.clone().begin_pass(raw, true, host.pixels_per_point, options);
        desk.raw = host.raw.clone();
        desk.pointer = host.pointer.clone();
        desk.events = host.events.clone();
        desk.keys_down = host.keys_down.clone();
        desk.modifiers = host.modifiers;
        desk.smooth_scroll_delta = host.smooth_scroll_delta;
        desk.unstable_dt = host.unstable_dt;
        desk.stable_dt = host.stable_dt;
        *i = desk;
        host
    })
}

/// Back to the host's screen ([`confine_screen`]), with what the desk took of this pass's input
/// (its keys, its scrolling) gone from it.
fn restore_screen(ctx: &egui::Context, mut host: egui::InputState) {
    ctx.input_mut(|i| {
        host.events = std::mem::take(&mut i.events);
        host.keys_down = std::mem::take(&mut i.keys_down);
        host.smooth_scroll_delta = i.smooth_scroll_delta;
        *i = host;
    });
}

/// The parameters params description `doc` names, typed as far as its text says (see
/// [`Desk::commands`]).
fn typed_params(doc: &str) -> Vec<Value> {
    use vectorcraft_engine::cmd::params::{object_body, split_top};
    let Ok((body, mut rest)) = object_body(doc.trim_start()) else { return Vec::new() };
    let mut entries = split_top(body, ',');
    // `{...} or {...}`: the objects the command takes instead.
    while let Some(next) = rest.trim_start().strip_prefix("or ").or_else(|| rest.trim_start().strip_prefix("| ")).filter(|r| r.starts_with('{')) {
        let Ok((b, r)) = object_body(next) else { break };
        entries.extend(split_top(b, ','));
        rest = r;
    }
    entries.into_iter().map(str::trim).filter(|e| !e.is_empty() && !e.starts_with('\u{2026}')).filter_map(param).collect()
}

/// One entry of a params description (`name?: note`, `name=default (note)`) typed.
fn param(entry: &str) -> Option<Value> {
    let head_end = entry.find([':', '=', '(']).unwrap_or(entry.len());
    let head = entry.get(..head_end)?.trim();
    // `a?|b?` names two keys: the first stands for both here.
    let first = head.split('|').next()?.trim();
    let optional = first.ends_with('?');
    let name = first.trim_end_matches('?').trim();
    let mut chars = name.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let rest = entry.get(head_end..)?.trim();
    let (mut default, note) = match rest.strip_prefix('=') {
        Some(r) => {
            let r = r.trim_start();
            let end = r.find([' ', '(']).unwrap_or(r.len());
            (Some(literal(r.get(..end)?)), r.get(end..)?.trim())
        }
        None => (None, rest.strip_prefix(':').unwrap_or(rest).trim()),
    };
    let mut p = json!({ "id": name, "optional": optional, "kind": Value::Null, "note": note });
    let lower = note.to_ascii_lowercase();
    let word = lower.split([' ', '(', ',']).next().unwrap_or_default();
    let choices = choices(note);
    let kind = if word == "bool" || word == "boolean" || word == "true|false" || word == "false|true" || word == "true" || word == "false" {
        "bool"
    } else if let Some((list, marked)) = choices {
        if default.is_none() {
            default = marked.map(Value::from);
        }
        p["choices"] = json!(list);
        "choice"
    } else if note.starts_with('[') {
        "array"
    } else if note.starts_with('{') {
        "object"
    } else if word.parse::<f64>().is_ok()
        || ["number", "int", "integer", "float", "deg", "degrees", "percent", "pt", "points", "px", "pixels", "%", "0-based", "1-based", "seconds", "ms"]
            .contains(&word)
    {
        // `quality?: 90 (...)`: a number first is the default.
        if default.is_none() && word.parse::<f64>().is_ok() {
            default = Some(literal(word));
        }
        "number"
    } else if ["string", "text", "name", "path", "url", "hex", "\"#rrggbb\""].contains(&word) {
        "string"
    } else if word == "id" {
        "id"
    } else {
        by_name(name, default.as_ref())
    };
    if let Some((lo, hi)) = range(note) {
        p["min"] = json!(lo);
        p["max"] = json!(hi);
    }
    if default.is_none()
        && let Some(i) = lower.find("default")
    {
        let after = note.get(i + "default".len()..).unwrap_or_default().trim_start_matches([':', ' ']);
        let end = after.find([' ', ')', ',', ';']).unwrap_or(after.len());
        default = after.get(..end).filter(|d| !d.is_empty()).map(literal);
    }
    if let Some(d) = default {
        p["default"] = d;
    }
    if kind != "untyped" {
        p["kind"] = json!(kind);
    }
    Some(p)
}

/// A default written in a description: a number, true, false, else the text.
fn literal(s: &str) -> Value {
    let s = s.trim().trim_matches(|c| c == '"' || c == '`');
    serde_json::from_str::<Value>(s).ok().filter(|v| v.is_number() || v.is_boolean()).unwrap_or_else(|| json!(s))
}

/// `none|art (default)|type`: the values a note lists, and the one it marks the default, when it
/// starts with such a list.
fn choices(note: &str) -> Option<(Vec<String>, Option<String>)> {
    let parts = vectorcraft_engine::cmd::params::split_top(note, '|');
    if parts.len() < 2 {
        return None;
    }
    let (mut list, mut marked) = (Vec::new(), None);
    for (k, part) in parts.iter().enumerate() {
        let part = part.trim();
        let (value, after) = part.split_once([' ', '(']).map_or((part, ""), |(v, a)| (v, a));
        let value = value.trim_matches('"');
        let after = after.trim_start().trim_start_matches('(');
        if value.is_empty() || !value.chars().all(|c| c.is_ascii_alphanumeric() || "_-#.".contains(c)) {
            return None;
        }
        // Only the last value may run on into prose; the others are a word, perhaps marked.
        if k + 1 < parts.len() && !(after.is_empty() || after.starts_with("default)")) {
            return None;
        }
        if after.starts_with("default)") {
            marked = Some(value.to_string());
        }
        list.push(value.to_string());
    }
    Some((list, marked))
}

/// `0..1`, `2-256`, `0\u{2013}100` in a note: its range.
fn range(note: &str) -> Option<(f64, f64)> {
    let bytes: Vec<char> = note.chars().collect();
    let num = |s: &str| s.parse::<f64>().ok();
    for (i, c) in bytes.iter().enumerate() {
        let sep = match c {
            '\u{2013}' => 1,
            '.' if bytes.get(i + 1) == Some(&'.') => 2,
            _ => continue,
        };
        let left: String = bytes.get(..i)?.iter().rev().take_while(|c| c.is_ascii_digit() || **c == '.' || **c == '-').collect::<Vec<_>>().into_iter().rev().collect();
        let right: String = bytes.get(i + sep..)?.iter().take_while(|c| c.is_ascii_digit() || **c == '.' || **c == '-').collect();
        if let (Some(lo), Some(hi)) = (num(left.trim_end_matches('.')), num(&right)) {
            return Some((lo, hi));
        }
    }
    None
}

/// A parameter's kind from its name and default, when its note says none.
fn by_name(name: &str, default: Option<&Value>) -> &'static str {
    const NUMBERS: &[&str] = &[
        "x", "y", "x1", "y1", "x2", "y2", "cx", "cy", "dx", "dy", "width", "height", "w", "h", "radius", "radius1", "radius2", "angle", "rotation",
        "opacity", "scale", "size", "zoom", "index", "count", "sides", "points", "segments", "amount", "spacing", "rows", "columns", "weight",
    ];
    match default {
        Some(Value::Bool(_)) => return "bool",
        Some(Value::Number(_)) => return "number",
        _ => {}
    }
    if NUMBERS.contains(&name) {
        "number"
    } else if name == "id" {
        "id"
    } else if name == "ids" {
        "array"
    } else if ["name", "path", "text", "title", "label", "format", "font"].contains(&name) {
        "string"
    } else {
        "untyped"
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::typed_params;

    #[test]
    fn params_descriptions_are_typed_where_they_say() {
        let p = typed_params("{x, y, width, height, rotation?: deg (about centre)} \u{2192} {id}");
        assert_eq!(p.iter().map(|p| p["kind"].clone()).collect::<Vec<_>>(), vec![json!("number"); 5]);
        assert_eq!(p[4]["optional"], true);
        let p = typed_params("{cx, cy, radius, sides=6, clockwise?: bool}");
        assert_eq!((p[3]["kind"].clone(), p[3]["default"].clone()), (json!("number"), json!(6)));
        assert_eq!(p[4]["kind"], "bool");
        let p = typed_params("{antiAlias?: none|art (default)|type (text snapped to pixels), quality?: 90 (jpg: 0\u{2013}100), what?: something}");
        assert_eq!(p[0]["choices"], json!(["none", "art", "type"]));
        assert_eq!(p[0]["default"], "art");
        assert_eq!((p[1]["kind"].clone(), p[1]["min"].clone(), p[1]["max"].clone()), (json!("number"), json!(0.0), json!(100.0)));
        assert!(p[2]["kind"].is_null(), "untyped: {}", p[2]);
    }
}
