//! What plugins add to the app: ribbon groups (on a built-in tab or a tab of their own) and
//! right-click menu items, declared in their manifest's `ui` section. Each runs one of the
//! plugin's commands through `plugins.run`.

use egui::Ui;
use gridcraft_engine::plugin::{self, ButtonSize, MenuKind};
use serde_json::{Value as Json, json};

use crate::SheetApp;
use crate::icons::Icon;
use crate::widgets::{big_button, small_button};

/// One button or menu item.
pub struct Item {
    pub plugin: String,
    pub command: String,
    pub label: String,
    pub tooltip: String,
    pub icon: Icon,
    pub small: bool,
    pub params: Json,
}

/// The tabs plugins add (names not among the built-in tabs), in load order.
pub fn extra_tabs() -> Vec<String> {
    let mut tabs: Vec<String> = Vec::new();
    for p in plugin::list().into_iter().filter(|p| p.enabled()) {
        for g in &p.manifest.ui.ribbon {
            let built_in = plugin::RIBBON_TABS.contains(&g.tab.as_str()) || (g.tab == "Developer" && crate::developer_ui::visible());
            if !built_in && !tabs.contains(&g.tab) {
                tabs.push(g.tab.clone());
            }
        }
    }
    tabs
}

/// The plugin groups on `tab`: (group name, buttons).
pub fn groups(tab: &str) -> Vec<(String, Vec<Item>)> {
    let mut out = Vec::new();
    for p in plugin::list().into_iter().filter(|p| p.enabled()) {
        let m = &p.manifest;
        for g in m.ui.ribbon.iter().filter(|g| g.tab == tab) {
            let buttons = g
                .buttons
                .iter()
                .map(|b| {
                    let decl = m.command(&b.command);
                    Item {
                        plugin: m.id.clone(),
                        command: b.command.clone(),
                        label: b.label.clone().or_else(|| decl.map(|c| c.label.clone())).unwrap_or_else(|| b.command.clone()),
                        tooltip: b.tooltip.clone().or_else(|| decl.map(|c| c.description.clone())).unwrap_or_default(),
                        icon: b.icon.as_deref().and_then(icon_named).unwrap_or(Icon::Script),
                        small: b.size == ButtonSize::Small,
                        params: b.params.clone(),
                    }
                })
                .collect();
            out.push((g.group.clone(), buttons));
        }
    }
    out
}

/// The plugin items for a right-click menu.
pub fn menu_items(kind: MenuKind) -> Vec<Item> {
    let mut out = Vec::new();
    for p in plugin::list().into_iter().filter(|p| p.enabled()) {
        let m = &p.manifest;
        for item in m.ui.menus.iter().filter(|i| i.menu == kind) {
            let decl = m.command(&item.command);
            out.push(Item {
                plugin: m.id.clone(),
                command: item.command.clone(),
                label: item.label.clone().or_else(|| decl.map(|c| c.label.clone())).unwrap_or_else(|| item.command.clone()),
                tooltip: decl.map(|c| c.description.clone()).unwrap_or_default(),
                icon: Icon::Script,
                small: true,
                params: item.params.clone(),
            });
        }
    }
    out
}

/// Runs an item's command; `extra` fields (e.g. the right-clicked sheet) are added to its params.
pub fn run(app: &mut SheetApp, item: &Item, extra: Json) {
    let mut params = if item.params.is_object() { item.params.clone() } else { json!({}) };
    if let (Some(p), Json::Object(e)) = (params.as_object_mut(), extra) {
        for (k, v) in e {
            p.entry(k).or_insert(v);
        }
    }
    app.run_or_alert("plugins.run", json!({"plugin": item.plugin, "command": item.command, "params": params}));
}

/// Draws the plugin groups on `tab`, each after a separator; small buttons stack three high.
pub fn ribbon_groups(app: &mut SheetApp, ui: &mut Ui, tab: &str, separate_first: bool) {
    for (i, (_group, buttons)) in groups(tab).into_iter().enumerate() {
        if separate_first || i > 0 {
            crate::ribbon::sep(ui);
        }
        let mut k = 0;
        while let Some(b) = buttons.get(k) {
            if !b.small {
                if labelled(big_button(ui, b.icon, &b.label, &b.tooltip, false), &b.label).clicked() {
                    run(app, b, json!({}));
                }
                k += 1;
                continue;
            }
            let column: Vec<&Item> = buttons.iter().skip(k).take_while(|b| b.small).take(3).collect();
            k += column.len();
            ui.vertical(|ui| {
                for b in column {
                    if labelled(small_button(ui, b.icon, &b.label, &b.tooltip, false), &b.label).clicked() {
                        run(app, b, json!({}));
                    }
                }
            });
        }
    }
}

/// Names the button for screen readers (and tests).
fn labelled(resp: egui::Response, label: &str) -> egui::Response {
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    resp
}

/// The icon a manifest names (`"icon": "chart"`); case and `-`/`_` don't matter.
pub fn icon_named(name: &str) -> Option<Icon> {
    let key: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    Some(match key.as_str() {
        "script" | "plugin" => Icon::Script,
        "paste" => Icon::Paste,
        "copy" => Icon::Copy,
        "cut" => Icon::Cut,
        "insert" => Icon::Insert,
        "delete" => Icon::Delete,
        "format" => Icon::Format,
        "sum" => Icon::Sum,
        "function" => Icon::Function,
        "fx" => Icon::Fx,
        "calc" | "calculate" | "refresh" => Icon::Calc,
        "table" => Icon::Table,
        "pivottable" => Icon::PivotTable,
        "chart" => Icon::Chart,
        "chartbar" => Icon::ChartBar,
        "chartline" => Icon::ChartLine,
        "chartpie" => Icon::ChartPie,
        "chartscatter" => Icon::ChartScatter,
        "sparkline" => Icon::Sparkline,
        "picture" | "image" => Icon::Picture,
        "link" => Icon::Link,
        "comment" => Icon::Comment,
        "note" => Icon::Note,
        "filter" => Icon::Filter,
        "sort" | "sortasc" => Icon::SortAsc,
        "sortdesc" => Icon::SortDesc,
        "find" => Icon::Find,
        "search" => Icon::Search,
        "lock" => Icon::Lock,
        "settings" => Icon::Settings,
        "share" => Icon::Share,
        "sheet" => Icon::Sheet,
        "book" | "workbook" => Icon::Book,
        "folder" | "open" => Icon::OpenFolder,
        "save" => Icon::Save,
        "print" => Icon::Print,
        "calendar" | "date" => Icon::Calendar,
        "clock" | "time" => Icon::Clock,
        "info" => Icon::Info,
        "check" => Icon::Check,
        "plus" | "add" => Icon::Plus,
        "minus" => Icon::Minus,
        "pen" => Icon::Pen,
        "currency" | "money" => Icon::Currency,
        "percent" => Icon::Percent,
        "validation" => Icon::Validation,
        "name" => Icon::Name,
        "zoom" => Icon::Zoom,
        "more" => Icon::More,
        _ => return None,
    })
}

// ---------------------------------------------------------------- managing plugins

/// Installing a plugin from GitHub: the address typed, the confirmation step, and the download,
/// which runs on its own thread so the window keeps painting.
#[derive(Default)]
pub struct Install {
    pub url: String,
    /// The repository waiting for the user's go-ahead (`owner/name`).
    pub confirming: Option<String>,
    pending: Option<std::sync::mpsc::Receiver<Result<std::path::PathBuf, String>>>,
    /// The last install's outcome: `Ok(message)` or `Err(error)`.
    pub outcome: Option<Result<String, String>>,
}

impl Install {
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
}

/// Starts downloading the plugin at `url` (after the user confirmed).
pub fn start_install(app: &mut SheetApp, url: &str) {
    let url = url.trim().to_string();
    let st = &mut app.plugin_install;
    st.confirming = None;
    let Some(root) = plugin::install::install_dir() else {
        st.outcome = Some(Err("There's no plugins folder to install into (preferences are turned off).".into()));
        return;
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("plugin-install".into()).spawn(move || {
            let _ = tx.send(plugin::install::from_github(&url, &root));
        });
        match spawned {
            Ok(_) => {
                st.pending = Some(rx);
                st.outcome = None;
            }
            Err(e) => st.outcome = Some(Err(format!("Couldn't start the download: {e}"))),
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (url, root);
        st.outcome = Some(Err("Plugins need the desktop app.".into()));
    }
}

/// Picks up a finished download and loads the plugin (every frame; nothing to do most of them).
pub fn poll_install(app: &mut SheetApp, ctx: &egui::Context) {
    let Some(rx) = &app.plugin_install.pending else { return };
    let result = match rx.try_recv() {
        Ok(r) => r,
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
            return;
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("the download stopped unexpectedly".into()),
    };
    app.plugin_install.pending = None;
    let outcome = result.and_then(|dir| app.run("plugins.load", json!({"path": dir}))).map(|info| {
        let name = info.get("name").and_then(Json::as_str).unwrap_or("The plugin");
        let version = info.get("version").and_then(Json::as_str).unwrap_or("");
        format!("Installed {name} {version}").trim_end().to_string()
    });
    if outcome.is_ok() {
        app.plugin_install.url.clear();
    }
    app.plugin_install.outcome = Some(outcome);
}

/// "Install from GitHub": an address field, a confirmation naming the repository, progress and
/// the outcome.
pub fn install_row(app: &mut SheetApp, ui: &mut Ui) {
    let lang = app.ui.language;
    ui.horizontal(|ui| {
        ui.label(lang.tr("Install from GitHub:"));
        let busy = app.plugin_install.busy();
        let field = egui::TextEdit::singleline(&mut app.plugin_install.url).hint_text("https://github.com/owner/repository").desired_width(340.0);
        let resp = ui.add_enabled(!busy, field);
        let go = ui.add_enabled(!busy && !app.plugin_install.url.trim().is_empty(), egui::Button::new(lang.tr("Install"))).clicked()
            || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !app.plugin_install.url.trim().is_empty());
        if go {
            match plugin::install::parse_github_url(&app.plugin_install.url) {
                Ok(r) => app.plugin_install.confirming = Some(format!("{}/{}", r.owner, r.name)),
                Err(e) => app.plugin_install.outcome = Some(Err(e)),
            }
        }
        if busy {
            ui.spinner();
            ui.label(lang.tr("Installing…"));
        }
    });
    if let Some(repo) = app.plugin_install.confirming.clone() {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(format!(
                "{} github.com/{repo}",
                lang.tr("A plugin is a program that runs with your permissions. Install it only if you trust its author. Install and run")
            ));
            ui.horizontal(|ui| {
                if ui.button(lang.tr("Install")).clicked() {
                    let url = app.plugin_install.url.clone();
                    start_install(app, &url);
                }
                if ui.button(lang.tr("Cancel")).clicked() {
                    app.plugin_install.confirming = None;
                }
            });
        });
    }
    match &app.plugin_install.outcome {
        Some(Ok(m)) => {
            ui.label(egui::RichText::new(m).color(crate::theme::Tokens::get(ui.ctx()).accent));
        }
        Some(Err(e)) => {
            ui.label(egui::RichText::new(e).color(ui.visuals().error_fg_color));
        }
        None => {}
    }
}

/// One plugin as a full-width card: name and version, then status, author, id and where it came
/// from, the description across the width, and Documentation / Activate or Deactivate / Unload on
/// the right.
pub fn plugin_card(app: &mut SheetApp, ui: &mut Ui, p: &plugin::Plugin, unload: bool) {
    let lang = app.ui.language;
    let m = &p.manifest;
    let on = p.enabled();
    egui::Frame::group(ui.style()).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(format!("{} {}", m.name, m.version).trim_end().to_string()).strong());
                let status = p.status();
                let mut about = vec![match status.as_str() {
                    "off" => lang.tr("Inactive").to_string(),
                    s if s.starts_with("failed") => s.to_string(),
                    _ => lang.tr("Active").to_string(),
                }];
                if !m.author.is_empty() {
                    about.push(m.author.clone());
                }
                about.push(m.id.clone());
                if let Some(repo) = p.source.as_ref().and_then(|s| s.get("repository")).and_then(Json::as_str) {
                    about.push(repo.trim_start_matches("https://").to_string());
                }
                ui.weak(about.join(" · "));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if unload && ui.button(lang.tr("Unload")).on_hover_text(lang.tr("Until the next start")).clicked() {
                    app.run_or_alert("plugins.unload", json!({"id": m.id}));
                }
                if ui.button(lang.tr(if on { "Deactivate" } else { "Activate" })).clicked() {
                    app.run_or_alert("plugins.setEnabled", json!({"id": m.id, "enabled": !on}));
                }
                if !m.homepage.trim().is_empty() && ui.link(lang.tr("Documentation")).on_hover_text(m.homepage.trim()).clicked() {
                    app.run_or_alert("plugins.openHomepage", json!({"id": m.id}));
                }
            });
        });
        if !m.description.is_empty() {
            ui.add(egui::Label::new(&m.description).wrap());
        }
    });
}
