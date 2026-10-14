//! Developer tab: macros. GridCraft doesn't run macros itself; a plugin that declares
//! `"macros": {"language": "VBA"}` does (protocol 1.1, `macros/*`), and reaches the workbook
//! back through the host requests answered here ([`host_call`]: `host/sync`, the workbook's
//! `vbaProject.bin`, messages). Running a macro is one undoable step.
//!
//! Macro security (`prefs.macro_security`): `disable` runs nothing; `prompt` (the default) runs
//! a workbook's macros once the user enabled them for it (`developer.enableMacros`, the message
//! bar's Enable Content); `enable` runs all.

use gridcraft_plugin::office::{Action, Permission};
use gridcraft_plugin::{HostError, call_macros, macro_plugins};
use serde_json::{Value as Json, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::{EngineError, Result, Session, UiRequest};

/// Largest `vbaProject.bin` a plugin may write.
const MAX_PROJECT: usize = 64 << 20;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "developer.macros", "Macros", ["Developer", "Code"], None, "{} → {macros: [{name, module, procedure, plugin, language}], enabled, security}. The workbook's macros", always, macros),
        cmd!(
            "developer.runMacro",
            "Run Macro",
            ["Developer", "Code"],
            None,
            "{name, plugin?} → {ran}. Runs a macro (Module1.Hello) as one undoable step; needs macros enabled for the workbook",
            always,
            run_macro
        ),
        cmd!(query "developer.modules", "Visual Basic", ["Developer", "Code"], None, "{plugin?} → {modules: [{name, kind, source}]}. The workbook's code modules", always, modules),
        cmd!(
            "developer.saveModule",
            "Save Module",
            ["Developer", "Code"],
            None,
            "{name, source, plugin?} → {}. Adds or replaces a code module (saved with the workbook as .xlsm)",
            always,
            save_module
        ),
        cmd!("developer.deleteModule", "Remove Module", ["Developer", "Code"], None, "{name, plugin?} → {}", always, delete_module),
        cmd!(query "developer.recordToCode", "Macro from Steps", ["Developer", "Code"], None, "{steps: [{command, params}], plugin?} → {source}. The steps as a macro's code", always, record_to_code),
        cmd!(
            noundo "developer.recordMacro",
            "Record Macro",
            ["Developer", "Code"],
            None,
            "{stop?: bool, module?: \"Module1\"} → {recording} or, when stopping, {module, source}. Records what you do as a macro in the module",
            always,
            record_macro
        ),
        cmd!(noundo "developer.enableMacros", "Enable Content", ["Developer", "Code"], None, "{on?: bool} → {enabled}. Lets this workbook's macros run (until it closes)", always, enable_macros),
        cmd!(
            noundo "developer.macroSecurity",
            "Macro Security",
            ["Developer", "Code"],
            None,
            "{level?: disable|prompt|enable} → {level}. Which workbooks' macros may run",
            always,
            macro_security
        ),
    ]
}

/// The plugin that runs macros: `plugin` when given, else the first one loaded.
fn macro_plugin(id: &str, p: &Json) -> Result<(String, String)> {
    let all = macro_plugins();
    match str_param(p, "plugin") {
        Some(want) => all.into_iter().find(|(pid, _)| pid == want).ok_or_else(|| bad(id, format!("no macro add-in `{want}` is loaded"))),
        None => all.into_iter().next().ok_or_else(|| bad(id, "no add-in runs macros: install one (VBA) in File › Options › Add-ins")),
    }
}

/// Whether the active workbook's macros may run.
pub fn macros_allowed(s: &Session) -> bool {
    match s.prefs.macro_security.as_str() {
        "enable" => true,
        "disable" => false,
        _ => s.active().is_some_and(|d| d.macros_enabled),
    }
}

/// Answers a plugin's request to the host (protocol 1.1) for the active workbook.
pub fn host_call(s: &mut Session, plugin: &str, method: &str, params: &Json) -> std::result::Result<Json, HostError> {
    let fail = |message: String| HostError { code: -32000, message, data: Json::Null };
    match method {
        "host/sync" => {
            let actions: Vec<Action> = serde_json::from_value(params.get("actions").cloned().unwrap_or(Json::Array(Vec::new())))
                .map_err(|e| fail(format!("`actions`: {e}")))?;
            if actions.len() > gridcraft_plugin::office::MAX_ACTIONS {
                return Err(fail("too many actions".into()));
            }
            // A plugin is a program the user installed: the workbook is open to it.
            match super::office::run_actions(s, plugin, Permission::ReadWriteDocument, &actions) {
                Ok(results) => Ok(json!({"results": results})),
                Err(error) => {
                    let message = error.get("message").and_then(Json::as_str).unwrap_or("failed").to_string();
                    Err(HostError { code: -32000, message, data: error })
                }
            }
        }
        "host/vbaProject/get" => {
            let bytes = s.active().and_then(|d| d.wb.vba_project.clone());
            Ok(json!({"base64": bytes.map(|b| crate::io::base64_encode(&b))}))
        }
        "host/vbaProject/set" => {
            let b64 = params.get("base64").and_then(Json::as_str).ok_or_else(|| fail("`base64` is required".into()))?;
            if b64.len() > MAX_PROJECT / 3 * 4 + 8 {
                return Err(fail("the project is larger than 64 MiB".into()));
            }
            let bytes = crate::io::base64_decode(b64).ok_or_else(|| fail("invalid base64".into()))?;
            let d = s.active_mut().ok_or_else(|| fail("no workbook is open".into()))?;
            std::sync::Arc::make_mut(&mut d.wb).vba_project = Some(bytes);
            Ok(json!({}))
        }
        "host/message" => {
            let text: String = params.get("text").and_then(Json::as_str).unwrap_or("").chars().take(4000).collect();
            s.ui_requests.push(UiRequest::Message(text));
            // vbOK: the message doesn't wait for the user yet.
            Ok(json!({"result": 1}))
        }
        "host/log" => {
            let text: String = params.get("text").and_then(Json::as_str).unwrap_or("").chars().take(4000).collect();
            log::info!("plugin {plugin}: {text}");
            Ok(json!({}))
        }
        _ => Err(HostError::unknown()),
    }
}

/// Sends `method` to the macro plugin, answering its host requests; the commands its host
/// requests run are one step of `id` (their own undo and journal entries are dropped), and a
/// failure takes them back.
fn call(s: &mut Session, _id: &str, plugin: &str, method: &str, params: Json) -> Result<Json> {
    let before = s.active().map(|d| (d.uid, d.wb.clone(), d.selection.clone(), d.undo.len()));
    let journal_len = s.journal.len();
    let r = call_macros(plugin, method, params, &mut |m, p| host_call(s, plugin, m, p));
    s.journal.truncate(journal_len);
    if let Some((uid, wb, sel, undo_len)) = before
        && let Some(d) = s.docs.iter_mut().find(|d| d.uid == uid)
    {
        d.undo.truncate(undo_len);
        if r.is_err() {
            d.wb = wb;
            d.selection = sel;
            d.calc.rebuild(&d.wb);
        }
    }
    // The add-in's own words (a macro's `Run-time error '9': Subscript out of range`).
    r.map_err(EngineError::Other)
}

fn macros(s: &mut Session, p: &Json) -> Result<Json> {
    let mut out = Vec::new();
    let plugins: Vec<(String, String)> = match str_param(p, "plugin") {
        Some(_) => vec![macro_plugin("developer.macros", p)?],
        None => macro_plugins(),
    };
    for (plugin, language) in plugins {
        let r = call(s, "developer.macros", &plugin, "macros/list", json!({}))?;
        for m in r.get("macros").and_then(Json::as_array).into_iter().flatten().take(10_000) {
            let mut m = m.clone();
            m["plugin"] = json!(plugin);
            m["language"] = json!(language);
            out.push(m);
        }
    }
    Ok(json!({"macros": out, "enabled": macros_allowed(s), "security": s.prefs.macro_security}))
}

fn run_macro(s: &mut Session, p: &Json) -> Result<Json> {
    const ID: &str = "developer.runMacro";
    let name = str_param(p, "name").filter(|n| !n.trim().is_empty()).ok_or_else(|| bad(ID, "missing `name`"))?.to_string();
    if !macros_allowed(s) {
        return Err(EngineError::Disabled(
            ID.into(),
            "macros are disabled for this workbook: Enable Content, or change Developer › Macro Security".into(),
        ));
    }
    let (plugin, _) = macro_plugin(ID, p)?;
    call(s, ID, &plugin, "macros/run", json!({"name": name}))?;
    Ok(json!({"ran": name}))
}

fn modules(s: &mut Session, p: &Json) -> Result<Json> {
    let (plugin, _) = macro_plugin("developer.modules", p)?;
    let mut r = call(s, "developer.modules", &plugin, "macros/modules", json!({}))?;
    r["plugin"] = json!(plugin);
    Ok(r)
}

fn save_module(s: &mut Session, p: &Json) -> Result<Json> {
    const ID: &str = "developer.saveModule";
    let name = str_param(p, "name").filter(|n| !n.trim().is_empty()).ok_or_else(|| bad(ID, "missing `name`"))?;
    let source = str_param(p, "source").ok_or_else(|| bad(ID, "missing `source`"))?;
    if source.len() > 16 << 20 {
        return Err(bad(ID, "a module is limited to 16 MiB"));
    }
    let (plugin, _) = macro_plugin(ID, p)?;
    call(s, ID, &plugin, "macros/saveModule", json!({"name": name, "source": source}))
}

fn delete_module(s: &mut Session, p: &Json) -> Result<Json> {
    const ID: &str = "developer.deleteModule";
    let name = str_param(p, "name").filter(|n| !n.trim().is_empty()).ok_or_else(|| bad(ID, "missing `name`"))?;
    let (plugin, _) = macro_plugin(ID, p)?;
    call(s, ID, &plugin, "macros/deleteModule", json!({"name": name}))
}

fn record_to_code(s: &mut Session, p: &Json) -> Result<Json> {
    const ID: &str = "developer.recordToCode";
    let steps = p.get("steps").cloned().filter(Json::is_array).ok_or_else(|| bad(ID, "missing `steps` (a list of {command, params})"))?;
    let (plugin, _) = macro_plugin(ID, p)?;
    call(s, ID, &plugin, "macros/record", json!({"steps": steps}))
}

fn record_macro(s: &mut Session, p: &Json) -> Result<Json> {
    const ID: &str = "developer.recordMacro";
    let stop = p.get("stop").and_then(Json::as_bool).unwrap_or(s.recording.is_some());
    if !stop {
        macro_plugin(ID, p)?;
        s.recording = Some(s.journal.len());
        return Ok(json!({"recording": true}));
    }
    let Some(start) = s.recording.take() else { return Ok(json!({"recording": false})) };
    let steps: Vec<Json> = s
        .journal
        .iter()
        .skip(start)
        .filter(|(id, _)| !id.starts_with("developer.") && !id.starts_with("automate."))
        .map(|(id, p)| json!({"command": id, "params": p}))
        .collect();
    let (plugin, _) = macro_plugin(ID, p)?;
    let code = call(s, ID, &plugin, "macros/record", json!({"steps": steps}))?;
    let source = code.get("source").and_then(Json::as_str).unwrap_or("").to_string();
    // The new macro goes at the end of the module (Module1 by default), as Excel records it.
    let module = str_param(p, "module").unwrap_or("Module1").to_string();
    let existing = call(s, ID, &plugin, "macros/modules", json!({}))?;
    let before = existing
        .get("modules")
        .and_then(Json::as_array)
        .and_then(|m| m.iter().find(|x| x.get("name").and_then(Json::as_str) == Some(module.as_str())))
        .and_then(|m| m.get("source").and_then(Json::as_str))
        .map(|t| format!("{}\r\n\r\n", t.trim_end()))
        .unwrap_or_default();
    call(s, ID, &plugin, "macros/saveModule", json!({"name": module, "source": format!("{before}{source}")}))?;
    Ok(json!({"recording": false, "module": module, "source": source, "steps": steps.len()}))
}

fn enable_macros(s: &mut Session, p: &Json) -> Result<Json> {
    let on = p.get("on").and_then(Json::as_bool).unwrap_or(true);
    let d = s.active_mut().ok_or_else(|| bad("developer.enableMacros", "no workbook is open"))?;
    d.macros_enabled = on;
    Ok(json!({"enabled": on}))
}

fn macro_security(s: &mut Session, p: &Json) -> Result<Json> {
    if let Some(level) = str_param(p, "level") {
        if !matches!(level, "disable" | "prompt" | "enable") {
            return Err(bad("developer.macroSecurity", "`level` is disable, prompt or enable"));
        }
        s.prefs.macro_security = level.to_string();
    }
    Ok(json!({"level": s.prefs.macro_security}))
}
