//! The Developer tab: macros run by a macro add-in (VBA). It shows once an add-in that runs
//! macros is loaded (`"macros": {"language": "VBA"}` in its manifest), with Visual Basic (the
//! workbook's code modules in an editor), Macros (list, run, edit, create, delete), Record Macro,
//! Macro Security and Add-ins. Everything goes through the `developer.*` commands; the add-in
//! owns the language, GridCraft the interface. A workbook with macros opens with them disabled
//! and a message bar to enable them (Macro Security: prompt).

use egui::{RichText, Ui, vec2};
use serde_json::{Value as Json, json};

use crate::SheetApp;
use crate::icons::Icon;
use crate::widgets::{big_button, small_button};

/// The Developer tab's windows.
#[derive(Default)]
pub struct DevState {
    pub macros: Option<MacrosWindow>,
    pub editor: Option<Editor>,
    pub security: bool,
    /// Workbooks whose "macros disabled" bar was shown (once per workbook).
    notice_seen: std::collections::HashSet<u64>,
}

#[derive(Default)]
pub struct MacrosWindow {
    pub list: Vec<Json>,
    pub selected: usize,
    pub new_name: String,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Editor {
    /// (name, kind, source)
    pub modules: Vec<(String, String, String)>,
    pub current: usize,
    pub text: String,
    pub dirty: bool,
    pub error: Option<String>,
    pub new_module: String,
}

/// Whether the Developer tab shows: an add-in that runs macros is loaded.
pub fn visible() -> bool {
    !gridcraft_engine::plugin::macro_plugins().is_empty()
}

fn language() -> String {
    gridcraft_engine::plugin::macro_plugins().into_iter().next().map(|(_, l)| l).unwrap_or_else(|| "VBA".into())
}

/// Names a painted button for screen readers (and tests).
fn named(resp: egui::Response, label: &str) -> egui::Response {
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    resp
}

pub fn ribbon(app: &mut SheetApp, ui: &mut Ui) {
    if named(big_button(ui, Icon::Script, "Visual\nBasic", "The workbook's code modules", false), "Visual Basic").clicked() {
        open_editor(app, None);
    }
    if named(big_button(ui, Icon::Script, "Macros", "Run, edit or delete a macro", false), "Macros").clicked() {
        open_macros(app);
    }
    ui.vertical(|ui| {
        let recording = app.session.recording.is_some();
        let label = if recording { "Stop Recording" } else { "Record Macro" };
        if named(small_button(ui, if recording { Icon::Close } else { Icon::Plus }, label, "Record what you do as a macro in Module1", false), label)
            .clicked()
        {
            match app.session.run("developer.recordMacro", json!({"stop": recording})) {
                Ok(r) if recording => {
                    let module = r.get("module").and_then(Json::as_str).unwrap_or("Module1").to_string();
                    app.after_engine();
                    open_editor(app, Some(&module));
                }
                Ok(_) => app.after_engine(),
                Err(e) => app.message = Some(("Record Macro".into(), e)),
            }
        }
        if named(small_button(ui, Icon::Check, "Macro Security", "Which workbooks' macros may run", false), "Macro Security").clicked() {
            app.dev.security = true;
        }
    });
    crate::ribbon::sep(ui);
    if named(big_button(ui, Icon::Script, "Add-ins", "Plugins and Office add-ins", false), "Add-ins").clicked() {
        app.open_dialog("addIns", json!({}));
    }
}

pub fn open_macros(app: &mut SheetApp) {
    let mut w = MacrosWindow::default();
    match app.session.run("developer.macros", json!({})) {
        Ok(r) => w.list = r.get("macros").and_then(Json::as_array).cloned().unwrap_or_default(),
        Err(e) => w.error = Some(e),
    }
    app.after_engine();
    app.dev.macros = Some(w);
}

pub fn open_editor(app: &mut SheetApp, module: Option<&str>) {
    let mut e = Editor::default();
    match app.session.run("developer.modules", json!({})) {
        Ok(r) => {
            e.modules = r
                .get("modules")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .map(|m| {
                    let s = |k: &str| m.get(k).and_then(Json::as_str).unwrap_or("").to_string();
                    (s("name"), s("kind"), s("source"))
                })
                .collect();
        }
        Err(err) => e.error = Some(err),
    }
    app.after_engine();
    if let Some(m) = module {
        e.current = e.modules.iter().position(|(n, _, _)| n == m).unwrap_or(0);
    }
    e.text = e.modules.get(e.current).map(|(_, _, s)| s.clone()).unwrap_or_default();
    app.dev.editor = Some(e);
}

/// The message bar for a workbook whose macros are disabled (once per workbook).
pub fn notice(app: &mut SheetApp) {
    if app.notice.is_some() || !visible() || app.session.prefs.macro_security != "prompt" {
        return;
    }
    let Some(d) = app.session.active() else { return };
    if d.wb.vba_project.is_none() || d.macros_enabled || app.dev.notice_seen.contains(&d.uid) {
        return;
    }
    app.dev.notice_seen.insert(d.uid);
    app.notice = Some(crate::Notice {
        text: "SECURITY WARNING  Macros have been disabled.".into(),
        kind: crate::NoticeKind::Warning,
        action: Some(("Enable Content".into(), "enableMacros".into())),
    });
}

/// Draws the open Developer windows.
pub fn windows(app: &mut SheetApp, ctx: &egui::Context) {
    macros_window(app, ctx);
    editor_window(app, ctx);
    security_window(app, ctx);
}

fn macros_window(app: &mut SheetApp, ctx: &egui::Context) {
    let Some(mut w) = app.dev.macros.take() else { return };
    let mut open = true;
    let mut close = false;
    let mut edit: Option<String> = None;
    let enabled = gridcraft_engine::cmd::developer::macros_allowed(&app.session);
    egui::Window::new(app.ui.language.tr("Macros"))
        .id(egui::Id::new("dev_macros"))
        .collapsible(false)
        .resizable(false)
        .default_width(460.0)
        .open(&mut open)
        .show(ctx, |ui| {
            if !enabled {
                ui.label(RichText::new("Macros are disabled for this workbook. Enable Content in the message bar, or change Macro Security.").weak());
            }
            ui.horizontal(|ui| {
                ui.label("Macro name:");
                ui.add(egui::TextEdit::singleline(&mut w.new_name).desired_width(260.0));
            });
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_min_size(vec2(420.0, 180.0));
                egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                    if w.list.is_empty() {
                        ui.weak("This workbook has no macros yet: type a name and Create.");
                    }
                    for (i, m) in w.list.iter().enumerate() {
                        let name = m.get("name").and_then(Json::as_str).unwrap_or("");
                        if ui.selectable_label(i == w.selected, name).clicked() {
                            w.selected = i;
                            w.new_name = name.to_string();
                        }
                    }
                });
            });
            let chosen = w.list.get(w.selected).and_then(|m| m.get("name")).and_then(Json::as_str).map(str::to_string);
            ui.horizontal(|ui| {
                if ui.add_enabled(enabled && chosen.is_some(), egui::Button::new("Run")).clicked()
                    && let Some(name) = &chosen
                {
                    match app.session.run("developer.runMacro", json!({"name": name})) {
                        Ok(_) => close = true,
                        Err(e) => w.error = Some(e),
                    }
                }
                if ui.add_enabled(chosen.is_some(), egui::Button::new("Edit")).clicked()
                    && let Some(m) = w.list.get(w.selected)
                {
                    edit = m.get("module").and_then(Json::as_str).map(str::to_string);
                }
                let name = w.new_name.trim().to_string();
                let valid = !name.is_empty()
                    && name.chars().all(|c| c.is_alphanumeric() || c == '_')
                    && !name.chars().next().is_some_and(|c| c.is_ascii_digit());
                if ui
                    .add_enabled(
                        valid && !w.list.iter().any(|m| m.get("procedure").and_then(Json::as_str) == Some(name.as_str())),
                        egui::Button::new("Create"),
                    )
                    .clicked()
                {
                    // A new Sub at the end of Module1, opened in the editor.
                    edit = Some(create_macro(app, &name, &mut w.error));
                }
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
            if let Some(e) = &w.error {
                ui.label(RichText::new(e).color(ui.visuals().error_fg_color));
            }
        });
    app.after_engine();
    if let Some(module) = edit {
        open_editor(app, Some(&module));
        return;
    }
    if open && !close {
        app.dev.macros = Some(w);
    }
}

/// Adds `Sub name()` to Module1 (made when missing); the module's name.
fn create_macro(app: &mut SheetApp, name: &str, error: &mut Option<String>) -> String {
    let module = "Module1".to_string();
    let existing = app
        .session
        .run("developer.modules", json!({}))
        .ok()
        .and_then(|r| r.get("modules").and_then(Json::as_array).cloned())
        .and_then(|m| m.into_iter().find(|x| x.get("name").and_then(Json::as_str) == Some("Module1")))
        .and_then(|m| m.get("source").and_then(Json::as_str).map(str::to_string))
        .map(|t| format!("{}\r\n\r\n", t.trim_end()))
        .unwrap_or_default();
    let source = format!("{existing}Sub {name}()\r\n\r\nEnd Sub\r\n");
    if let Err(e) = app.session.run("developer.saveModule", json!({"name": module, "source": source})) {
        *error = Some(e);
    }
    module
}

fn editor_window(app: &mut SheetApp, ctx: &egui::Context) {
    let Some(mut e) = app.dev.editor.take() else { return };
    let mut open = true;
    let title = format!("{} — {}", app.ui.language.tr("Visual Basic"), app.session.active().map(|d| d.display_title()).unwrap_or_default());
    egui::Window::new(title)
        .id(egui::Id::new("dev_editor"))
        .collapsible(false)
        .resizable(true)
        .default_size(vec2(820.0, 520.0))
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(e.dirty, egui::Button::new("Save"))
                    .on_hover_text("Saves the module into the workbook (save the workbook as .xlsm to keep it)")
                    .clicked()
                {
                    let name = e.modules.get(e.current).map(|(n, _, _)| n.clone()).unwrap_or_else(|| "Module1".into());
                    match app.session.run("developer.saveModule", json!({"name": name, "source": e.text})) {
                        Ok(_) => {
                            if let Some(m) = e.modules.get_mut(e.current) {
                                m.2 = e.text.clone();
                            }
                            e.dirty = false;
                            e.error = None;
                        }
                        Err(err) => e.error = Some(err),
                    }
                }
                if ui.button("Run Macro…").clicked() {
                    open_macros(app);
                }
                ui.separator();
                ui.add(egui::TextEdit::singleline(&mut e.new_module).hint_text("Module2").desired_width(120.0));
                let valid = !e.new_module.trim().is_empty() && !e.modules.iter().any(|(n, _, _)| n.eq_ignore_ascii_case(e.new_module.trim()));
                if ui.add_enabled(valid, egui::Button::new("Insert Module")).clicked() {
                    let name = e.new_module.trim().to_string();
                    let source = format!("Attribute VB_Name = \"{name}\"\r\n");
                    match app.session.run("developer.saveModule", json!({"name": name, "source": source})) {
                        Ok(_) => {
                            e.modules.push((name, "standard".into(), source.clone()));
                            e.current = e.modules.len() - 1;
                            e.text = source;
                            e.dirty = false;
                            e.new_module.clear();
                        }
                        Err(err) => e.error = Some(err),
                    }
                }
                let standard = e.modules.get(e.current).is_some_and(|(_, k, _)| k == "standard");
                if ui.add_enabled(standard, egui::Button::new("Remove Module")).clicked()
                    && let Some((name, _, _)) = e.modules.get(e.current).cloned()
                {
                    match app.session.run("developer.deleteModule", json!({"name": name})) {
                        Ok(_) => {
                            e.modules.remove(e.current);
                            e.current = 0;
                            e.text = e.modules.first().map(|(_, _, s)| s.clone()).unwrap_or_default();
                            e.dirty = false;
                        }
                        Err(err) => e.error = Some(err),
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(language());
                });
            });
            if let Some(err) = &e.error {
                ui.label(RichText::new(err).color(ui.visuals().error_fg_color));
            }
            ui.separator();
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(170.0);
                    ui.weak("Modules");
                    if e.modules.is_empty() {
                        ui.weak("No code yet: Insert Module.");
                    }
                    let mut pick = None;
                    for (i, (name, kind, _)) in e.modules.iter().enumerate() {
                        let label = if kind == "standard" { name.clone() } else { format!("{name} ({kind})") };
                        if ui.selectable_label(i == e.current, label).clicked() {
                            pick = Some(i);
                        }
                    }
                    if let Some(i) = pick
                        && i != e.current
                    {
                        if let Some(m) = e.modules.get_mut(e.current)
                            && e.dirty
                        {
                            // Unsaved text stays with its module until saved or closed.
                            m.2 = e.text.clone();
                        }
                        e.current = i;
                        e.text = e.modules.get(i).map(|(_, _, s)| s.clone()).unwrap_or_default();
                    }
                });
                ui.separator();
                egui::ScrollArea::both().id_salt("dev_code").show(ui, |ui| {
                    let resp = ui.add(
                        egui::TextEdit::multiline(&mut e.text)
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .desired_rows(24)
                            .lock_focus(true)
                            .interactive(!e.modules.is_empty()),
                    );
                    if resp.changed() {
                        e.dirty = true;
                    }
                });
            });
        });
    if open {
        app.dev.editor = Some(e);
    }
}

fn security_window(app: &mut SheetApp, ctx: &egui::Context) {
    if !app.dev.security {
        return;
    }
    let mut open = true;
    let level = app.session.prefs.macro_security.clone();
    egui::Window::new(app.ui.language.tr("Macro Security"))
        .id(egui::Id::new("dev_security"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            for (value, text) in [
                ("disable", "Disable all macros"),
                ("prompt", "Disable macros with notification (enable them per workbook)"),
                ("enable", "Enable all macros (not recommended: a macro is a program)"),
            ] {
                if ui.radio(level == value, text).clicked() {
                    app.run_or_alert("developer.macroSecurity", json!({"level": value}));
                }
            }
            ui.add_space(6.0);
            if ui.button("Close").clicked() {
                app.dev.security = false;
            }
        });
    if !open {
        app.dev.security = false;
    }
}
