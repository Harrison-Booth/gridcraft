//! Plugins end to end: the engine loads a plugin, formulas call its function, and its commands
//! run as one undoable step. The plugin is this test binary started again with
//! `--serve-plugin` (no libtest harness, so standard output is the protocol's alone).

use std::io::{BufRead, Write};

use gridcraft_engine::Session;
use gridcraft_engine::core::{CellError, CellRef, Value};
use serde_json::{Value as Json, json};

const PLUGIN: &str = "org.gridcraft.test-engine";

fn main() {
    if std::env::args().any(|a| a == "--serve-plugin") {
        serve();
        return;
    }
    let tests: [(&str, fn()); 6] = [
        ("plugin_commands_see_sheets_and_names", plugin_commands_see_sheets_and_names),
        ("functions_recalculate_when_the_plugin_arrives_and_leaves", functions_recalculate_when_the_plugin_arrives_and_leaves),
        ("plugin_commands_are_one_undo_step", plugin_commands_are_one_undo_step),
        ("plugin_commands_see_the_selections_formulas", plugin_commands_see_the_selections_formulas),
        ("a_failing_plugin_command_changes_nothing", a_failing_plugin_command_changes_nothing),
        ("plugins_turn_off_and_on_and_the_preferences_remember", plugins_turn_off_and_on_and_the_preferences_remember),
    ];
    for (name, t) in tests {
        t();
        println!("test {name} ... ok");
    }
}

/// The plugin side.
fn serve() {
    let mut out = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Json>(&line) else { continue };
        let Some(id) = msg.get("id").cloned() else { break };
        let p = &msg["params"];
        let result = match msg["method"].as_str() {
            Some("functions/call") => json!({"value": p["args"][0].as_f64().map(|x| json!(x * 2.0)).unwrap_or(json!({"error": "#VALUE!"}))}),
            Some("commands/run") => {
                let at = p["context"]["active"].as_str().unwrap_or("A1");
                let mut cmds = vec![
                    json!({"command": "cell.set", "params": {"cell": at, "input": "one"}}),
                    json!({"command": "cell.set", "params": {"cell": "C1", "input": "=TESTPLUG.DOUBLE(21)"}}),
                ];
                if p["command"] == "broken" {
                    cmds.push(json!({"command": "no.suchCommand"}));
                }
                json!({"commands": cmds, "message": "done", "result": {"saw": p["context"]["selection"], "formulas": p["context"]["formulas"], "sheets": p["context"]["sheets"], "names": p["context"]["names"]}})
            }
            _ => json!({}),
        };
        if writeln!(out, "{}", json!({"jsonrpc": "2.0", "id": id, "result": result})).is_err() {
            break;
        }
    }
}

fn load(s: &mut Session) {
    let dir = std::env::temp_dir().join(format!("gridcraft-engine-plugin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = std::env::current_exe().unwrap();
    let manifest = json!({
        "id": PLUGIN, "name": "Engine test", "homepage": "https://example.org/docs", "run": {"command": exe, "args": ["--serve-plugin"]},
        "functions": [{"name": "TESTPLUG.DOUBLE", "minArgs": 1, "maxArgs": 1}],
        "commands": [{"id": "fill", "label": "Fill"}, {"id": "broken", "label": "Broken"}],
    });
    std::fs::write(dir.join("plugin.json"), manifest.to_string()).unwrap();
    let info = s.execute("plugins.load", json!({"path": dir})).unwrap();
    assert_eq!(info["id"], PLUGIN);
}

fn value(s: &Session, a: &str) -> Value {
    s.active().unwrap().wb.active().unwrap().value(CellRef::parse(a).unwrap())
}

fn functions_recalculate_when_the_plugin_arrives_and_leaves() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "A1", "input": "=TESTPLUG.DOUBLE(B1)"})).unwrap();
    s.execute("cell.set", json!({"cell": "B1", "input": "4"})).unwrap();
    assert_eq!(value(&s, "A1"), Value::Error(CellError::Name));
    load(&mut s);
    assert_eq!(value(&s, "A1"), Value::number(8.0));
    s.execute("cell.set", json!({"cell": "B1", "input": "5"})).unwrap();
    assert_eq!(value(&s, "A1"), Value::number(10.0));
    let listed = s.execute("home.addIns", json!({})).unwrap();
    assert!(listed["addIns"].as_array().unwrap().iter().any(|p| p["id"] == PLUGIN));
    s.execute("plugins.unload", json!({"id": PLUGIN})).unwrap();
    assert_eq!(value(&s, "A1"), Value::Error(CellError::Name));
    assert!(s.execute("plugins.unload", json!({"id": PLUGIN})).is_err());
}

fn plugin_commands_are_one_undo_step() {
    let mut s = Session::new();
    s.new_workbook();
    load(&mut s);
    s.execute("selection.set", json!({"range": "B2:B3"})).unwrap();
    let undo = s.active().unwrap().undo.len();
    let journal = s.journal.len();
    let r = s.execute("plugins.run", json!({"plugin": PLUGIN, "command": "fill"})).unwrap();
    assert_eq!(r["ran"], 2);
    assert_eq!(r["result"]["saw"], "B2:B3");
    assert_eq!(value(&s, "B2"), Value::from("one"));
    assert_eq!(value(&s, "C1"), Value::number(42.0));
    assert_eq!(s.active().unwrap().undo.len(), undo + 1);
    assert_eq!(s.journal.len(), journal + 1);
    assert_eq!(s.journal.last().unwrap().0, "plugins.run");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(value(&s, "B2"), Value::Empty);
    assert_eq!(value(&s, "C1"), Value::Empty);
    s.execute("plugins.unload", json!({"id": PLUGIN})).unwrap();
}

fn plugin_commands_see_the_selections_formulas() {
    let mut s = Session::new();
    s.new_workbook();
    load(&mut s);
    s.execute("cell.set", json!({"cell": "B2", "input": "=1+1"})).unwrap();
    s.execute("cell.set", json!({"cell": "B3", "input": "7"})).unwrap();
    s.execute("selection.set", json!({"range": "B2:B4"})).unwrap();
    let r = s.execute("plugins.run", json!({"plugin": PLUGIN, "command": "fill"})).unwrap();
    assert_eq!(r["result"]["formulas"], json!([["=1+1"], [null], [null]]));
    s.execute("plugins.unload", json!({"id": PLUGIN})).unwrap();
}

fn plugin_commands_see_sheets_and_names() {
    let mut s = Session::new();
    s.new_workbook();
    load(&mut s);
    s.execute("home.insertSheet", json!({})).unwrap();
    s.execute("sheet.hide", json!({"sheet": 1})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    s.execute("cell.set", json!({"cell": "C4", "input": "5"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Rate", "refersTo": "=0.05"})).unwrap();
    let r = s.execute("plugins.run", json!({"plugin": PLUGIN, "command": "fill"})).unwrap();
    let sheets = r["result"]["sheets"].as_array().unwrap();
    assert_eq!(sheets.len(), 2);
    assert_eq!(sheets[0]["usedRange"], "C4");
    assert_eq!(sheets[1]["visibility"], "Hidden");
    let names = r["result"]["names"].as_array().unwrap();
    assert!(names.iter().any(|n| n["name"] == "Rate" && n["refersTo"] == "=0.05" && n["hidden"] == false), "{names:?}");
    s.execute("plugins.unload", json!({"id": PLUGIN})).unwrap();
}

fn a_failing_plugin_command_changes_nothing() {
    let mut s = Session::new();
    s.new_workbook();
    load(&mut s);
    let undo = s.active().unwrap().undo.len();
    let e = s.execute("plugins.run", json!({"plugin": PLUGIN, "command": "broken"})).unwrap_err().to_string();
    assert!(e.contains("nothing was changed"), "{e}");
    assert_eq!(value(&s, "A1"), Value::Empty);
    assert_eq!(s.active().unwrap().undo.len(), undo);
    assert!(s.execute("plugins.run", json!({"plugin": PLUGIN, "command": "nope"})).is_err());
    assert!(s.execute("plugins.run", json!({})).is_err());
    s.execute("plugins.unload", json!({"id": PLUGIN})).unwrap();
}

fn plugins_turn_off_and_on_and_the_preferences_remember() {
    let mut s = Session::new();
    s.new_workbook();
    load(&mut s);
    s.execute("cell.set", json!({"cell": "A1", "input": "=TESTPLUG.DOUBLE(3)"})).unwrap();
    assert_eq!(value(&s, "A1"), Value::number(6.0));

    let off = s.execute("plugins.setEnabled", json!({"id": PLUGIN, "enabled": false})).unwrap();
    assert_eq!((off["enabled"].clone(), off["status"].clone()), (json!(false), json!("off")));
    assert_eq!(value(&s, "A1"), Value::Error(CellError::Name), "its functions go");
    assert_eq!(s.prefs.disabled_plugins, vec![PLUGIN.to_string()]);
    let e = s.execute("plugins.run", json!({"plugin": PLUGIN, "command": "fill"})).unwrap_err().to_string();
    assert!(e.contains("turned off"), "{e}");
    assert!(s.execute("plugins.list", json!({})).unwrap().as_array().unwrap().iter().any(|p| p["id"] == PLUGIN), "still listed");

    // Loading it again (as at the next start) keeps it off.
    load(&mut s);
    assert!(!gridcraft_engine::plugin::get(PLUGIN).unwrap().enabled());
    gridcraft_engine::cmd::plugins::apply_prefs(&s.prefs);
    assert!(!gridcraft_engine::plugin::get(PLUGIN).unwrap().enabled());

    let on = s.execute("plugins.setEnabled", json!({"id": PLUGIN, "enabled": true})).unwrap();
    assert_eq!(on["enabled"], true);
    assert_eq!(value(&s, "A1"), Value::number(6.0));
    assert!(s.prefs.disabled_plugins.is_empty());
    assert!(s.execute("plugins.setEnabled", json!({"id": PLUGIN})).is_err());

    s.ui_requests.clear();
    assert_eq!(s.execute("plugins.openHomepage", json!({"id": PLUGIN})).unwrap()["url"], "https://example.org/docs");
    assert!(s.ui_requests.contains(&gridcraft_engine::UiRequest::OpenUrl("https://example.org/docs".into())));
    s.execute("plugins.unload", json!({"id": PLUGIN})).unwrap();
    assert!(s.execute("plugins.openHomepage", json!({"id": PLUGIN})).is_err());
}
