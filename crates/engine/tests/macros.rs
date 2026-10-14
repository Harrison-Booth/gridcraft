//! Macros through a macro add-in (protocol 1.1): the add-in reads and writes the workbook while a
//! macro runs (`host/sync`), keeps its code in the workbook's `vbaProject.bin`, and a run is one
//! undo step. The add-in is this test binary started again with `--serve-macros` (no libtest
//! harness, so standard output is the protocol's alone).

use std::io::{BufRead, Write};

use gridcraft_engine::Session;
use gridcraft_engine::core::{CellRef, Value};
use serde_json::{Value as Json, json};

const PLUGIN: &str = "org.gridcraft.test-macros";

fn main() {
    if std::env::args().any(|a| a == "--serve-macros") {
        serve();
        return;
    }
    let tests: [(&str, fn()); 4] = [
        ("macros_run_as_one_undo_step_and_call_the_host", macros_run_as_one_undo_step_and_call_the_host),
        ("a_failing_macro_changes_nothing", a_failing_macro_changes_nothing),
        ("macro_security_gates_running", macro_security_gates_running),
        ("modules_live_in_the_workbooks_vba_project", modules_live_in_the_workbooks_vba_project),
    ];
    for (name, t) in tests {
        t();
        println!("test {name} ... ok");
    }
}

/// The add-in: `Fill` writes B1 = A1 * 2 (reading A1 through the host) and says so; `Broken`
/// writes then fails; modules are the vbaProject's bytes taken as UTF-8 text.
fn serve() {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut out = std::io::stdout();
    let mut next = 0u32;
    // Sends a host request and waits for its answer.
    let mut host = |out: &mut std::io::Stdout, lines: &mut std::io::Lines<std::io::StdinLock<'_>>, method: &str, params: Json| -> Json {
        next += 1;
        let id = format!("h{next}");
        writeln!(out, "{}", json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).unwrap();
        out.flush().unwrap();
        for line in lines.by_ref() {
            let msg: Json = serde_json::from_str(&line.unwrap()).unwrap();
            if msg["id"] == json!(id) {
                return msg;
            }
        }
        Json::Null
    };
    let a1 = json!([{"p": "workbook"}, {"p": "worksheets"}, {"m": "getActiveWorksheet"}, {"m": "getRange", "a": ["A1"]}]);
    let b1 = json!([{"p": "workbook"}, {"p": "worksheets"}, {"m": "getActiveWorksheet"}, {"m": "getRange", "a": ["B1"]}]);
    while let Some(Ok(line)) = lines.next() {
        let Ok(msg) = serde_json::from_str::<Json>(&line) else { continue };
        let Some(id) = msg.get("id").cloned() else { break };
        let p = msg["params"].clone();
        let answer = match msg["method"].as_str() {
            Some("macros/list") => {
                json!({"result": {"macros": [{"name": "Module1.Fill", "module": "Module1", "procedure": "Fill"}, {"name": "Module1.Broken", "module": "Module1", "procedure": "Broken"}]}})
            }
            Some("macros/run") if p["name"] == "Module1.Fill" || p["name"] == "Fill" => {
                let r = host(&mut out, &mut lines, "host/sync", json!({"actions": [{"op": "load", "path": a1, "props": ["values"]}]}));
                let x = r["result"]["results"][0]["values"][0][0].as_f64().unwrap_or(0.0);
                host(&mut out, &mut lines, "host/sync", json!({"actions": [{"op": "set", "path": b1, "prop": "values", "value": x * 2.0}]}));
                host(&mut out, &mut lines, "host/message", json!({"text": format!("doubled {x}")}));
                json!({"result": {}})
            }
            Some("macros/run") if p["name"] == "Module1.Broken" => {
                host(&mut out, &mut lines, "host/sync", json!({"actions": [{"op": "set", "path": b1, "prop": "values", "value": 99}]}));
                let bad = host(
                    &mut out,
                    &mut lines,
                    "host/sync",
                    json!({"actions": [{"op": "load", "path": [{"p": "workbook"}, {"p": "worksheets"}, {"m": "getItem", "a": ["Nope"]}], "props": ["name"]}]}),
                );
                json!({"error": {"code": 1, "message": format!("Run-time error '9': Subscript out of range ({})", bad["error"]["data"]["code"].as_str().unwrap_or(""))}})
            }
            Some("macros/modules") => {
                let r = host(&mut out, &mut lines, "host/vbaProject/get", json!({}));
                let text = r["result"]["base64"]
                    .as_str()
                    .map(|b| String::from_utf8(gridcraft_engine::io::base64_decode(b).unwrap()).unwrap())
                    .unwrap_or_default();
                json!({"result": {"modules": if text.is_empty() { json!([]) } else { json!([{"name": "Module1", "kind": "standard", "source": text}]) }}})
            }
            Some("macros/saveModule") => {
                let b64 = gridcraft_engine::io::base64_encode(p["source"].as_str().unwrap_or("").as_bytes());
                host(&mut out, &mut lines, "host/vbaProject/set", json!({"base64": b64}));
                json!({"result": {}})
            }
            Some("macros/record") => {
                json!({"result": {"source": format!("Sub Macro1()\r\n' {} steps\r\nEnd Sub", p["steps"].as_array().map(Vec::len).unwrap_or(0))}})
            }
            _ => json!({"result": {}}),
        };
        let mut reply = json!({"jsonrpc": "2.0", "id": id});
        if let (Some(r), Json::Object(a)) = (reply.as_object_mut(), answer) {
            r.extend(a);
        }
        if writeln!(out, "{reply}").and_then(|_| out.flush()).is_err() {
            break;
        }
    }
}

fn session() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    let dir = std::env::temp_dir().join(format!("gridcraft-engine-macros-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = std::env::current_exe().unwrap();
    let manifest =
        json!({"id": PLUGIN, "name": "Test macros", "macros": {"language": "TestBasic"}, "run": {"command": exe, "args": ["--serve-macros"]}});
    std::fs::write(dir.join("plugin.json"), manifest.to_string()).unwrap();
    s.execute("plugins.load", json!({"path": dir})).unwrap();
    s
}

fn value(s: &Session, a: &str) -> Value {
    s.active().unwrap().wb.active().unwrap().value(CellRef::parse(a).unwrap())
}

fn macros_run_as_one_undo_step_and_call_the_host() {
    let mut s = session();
    s.execute("cell.set", json!({"cell": "A1", "input": "21"})).unwrap();
    let list = s.execute("developer.macros", json!({})).unwrap();
    assert_eq!(list["macros"][0]["name"], "Module1.Fill");
    assert_eq!(list["macros"][0]["language"], "TestBasic");
    assert_eq!(list["enabled"], false, "a workbook's macros are off until enabled");
    s.execute("developer.enableMacros", json!({})).unwrap();
    let undo = s.active().unwrap().undo.len();
    s.execute("developer.runMacro", json!({"name": "Module1.Fill"})).unwrap();
    assert_eq!(value(&s, "B1"), Value::Number(42.0));
    assert_eq!(s.active().unwrap().undo.len(), undo + 1, "one undo step for the whole macro");
    assert!(s.take_ui_requests().iter().any(|q| matches!(q, gridcraft_engine::UiRequest::Message(m) if m == "doubled 21")));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(value(&s, "B1"), Value::Empty);
    assert!(gridcraft_engine::plugin::unload(PLUGIN));
}

fn a_failing_macro_changes_nothing() {
    let mut s = session();
    s.execute("developer.enableMacros", json!({})).unwrap();
    let undo = s.active().unwrap().undo.len();
    let e = s.execute("developer.runMacro", json!({"name": "Module1.Broken"})).unwrap_err().to_string();
    assert!(e.contains("Subscript out of range") && e.contains("ItemNotFound"), "{e}");
    assert_eq!(value(&s, "B1"), Value::Empty, "the write before the failure was taken back");
    assert_eq!(s.active().unwrap().undo.len(), undo);
    assert!(gridcraft_engine::plugin::unload(PLUGIN));
}

fn macro_security_gates_running() {
    let mut s = session();
    assert!(s.execute("developer.runMacro", json!({"name": "Fill"})).unwrap_err().to_string().contains("disabled"));
    s.execute("developer.macroSecurity", json!({"level": "enable"})).unwrap();
    assert!(s.execute("developer.runMacro", json!({"name": "Fill"})).is_ok());
    s.execute("developer.macroSecurity", json!({"level": "disable"})).unwrap();
    s.execute("developer.enableMacros", json!({})).unwrap();
    assert!(s.execute("developer.runMacro", json!({"name": "Fill"})).is_err(), "Disable wins over a workbook's Enable Content");
    assert!(s.execute("developer.macroSecurity", json!({"level": "maybe"})).is_err());
    assert!(gridcraft_engine::plugin::unload(PLUGIN));
    assert!(s.execute("developer.macros", json!({})).unwrap()["macros"].as_array().unwrap().is_empty());
    assert!(s.execute("developer.runMacro", json!({"name": "Fill"})).is_err());
}

fn modules_live_in_the_workbooks_vba_project() {
    let mut s = session();
    assert_eq!(s.execute("developer.modules", json!({})).unwrap()["modules"], json!([]));
    s.execute("developer.saveModule", json!({"name": "Module1", "source": "Sub Hello()\r\nEnd Sub"})).unwrap();
    assert_eq!(s.active().unwrap().wb.vba_project.as_deref(), Some(&b"Sub Hello()\r\nEnd Sub"[..]));
    assert_eq!(s.execute("developer.modules", json!({})).unwrap()["modules"][0]["source"], "Sub Hello()\r\nEnd Sub");
    // Saving a module is undoable, like any change to the workbook.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.active().unwrap().wb.vba_project.is_none());
    let code = s.execute("developer.recordToCode", json!({"steps": [{"command": "cell.set", "params": {"cell": "A1", "input": "5"}}]})).unwrap();
    assert!(code["source"].as_str().unwrap().contains("1 steps"));
    assert!(gridcraft_engine::plugin::unload(PLUGIN));
}
