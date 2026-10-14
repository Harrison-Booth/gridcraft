use serde_json::{Value as Json, json};

use crate::Session;

/// Office add-ins are process-wide: these tests take turns.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// A minimal add-in manifest (written for these tests from the public schema) with `permissions`.
fn add_addin(guid: &str, permissions: &str) -> String {
    let xml = format!(
        r#"<OfficeApp xmlns="http://schemas.microsoft.com/office/appforoffice/1.1" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="TaskPaneApp">
  <Id>{guid}</Id><Version>1.0</Version><ProviderName>Tests</ProviderName><DefaultLocale>en-US</DefaultLocale>
  <DisplayName DefaultValue="Sync Test"/><Hosts><Host Name="Workbook"/></Hosts>
  <DefaultSettings><SourceLocation DefaultValue="https://tests.example/pane.html"/></DefaultSettings>
  <Permissions>{permissions}</Permissions></OfficeApp>"#
    );
    let m = gridcraft_plugin::office::parse_manifest(&xml, "gridcraft").unwrap();
    let id = m.id.clone();
    gridcraft_plugin::install(m, std::env::temp_dir()).unwrap();
    id
}

fn session() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s
}

fn sync(s: &mut Session, addin: &str, actions: Json) -> Json {
    s.execute("office.sync", json!({"addin": addin, "actions": actions})).unwrap()
}

fn range(sheet: &str, addr: &str) -> Json {
    json!([{"p": "workbook"}, {"p": "worksheets"}, {"m": "getItem", "a": [sheet]}, {"m": "getRange", "a": [addr]}])
}

#[test]
fn a_sync_writes_and_reads_ranges_as_one_undo_step() {
    let _g = serial();
    let id = add_addin("aaaaaaaa-0000-4000-8000-000000000001", "ReadWriteDocument");
    let mut s = session();
    s.execute("cell.set", json!({"cell": "A1", "input": "keep"})).unwrap();
    let undo = s.active().unwrap().undo.len();
    let out = sync(
        &mut s,
        &id,
        json!([
            {"op": "set", "path": range("Sheet1", "B2:C3"), "prop": "values", "value": [[1, 2], [3, "=B2+C2"]]},
            {"op": "set", "path": [{"p": "workbook"}, {"p": "worksheets"}, {"m": "getItem", "a": ["sheet1"]}, {"m": "getRange", "a": ["B2:C3"]}, {"p": "format"}, {"p": "font"}], "prop": "bold", "value": true},
            {"op": "set", "path": range("Sheet1", "D1"), "prop": "values", "value": "filled"},
            {"op": "set", "path": range("Sheet1", "B2"), "prop": "numberFormat", "value": "0.00"},
            {"op": "load", "path": range("Sheet1", "B2:C3"), "props": ["values", "formulas", "text", "address", "rowCount"]},
            {"op": "load", "path": [{"p": "workbook"}, {"p": "worksheets"}, {"m": "getItem", "a": ["Sheet1"]}, {"m": "getRange", "a": ["B2"]}, {"p": "format"}, {"p": "font"}], "props": ["bold", "name"]},
            {"op": "load", "path": [{"p": "workbook"}, {"p": "worksheets"}], "props": ["items"]},
        ]),
    );
    let r = &out["results"];
    assert_eq!(r[4]["values"], json!([[1.0, 2.0], [3.0, 3.0]]), "{out}");
    assert_eq!(r[4]["formulas"][1][1], "=B2+C2");
    assert_eq!(r[4]["text"][0][0], "1.00");
    assert_eq!(r[4]["address"], "Sheet1!B2:C3");
    assert_eq!(r[4]["rowCount"], 2);
    assert_eq!(r[5]["bold"], true);
    assert_eq!(r[6]["items"][0]["name"], "Sheet1");
    assert_eq!(s.active().unwrap().undo.len(), undo + 1, "the whole sync is one undo step");
    s.execute("edit.undo", json!({})).unwrap();
    let back = sync(&mut s, &id, json!([{"op": "load", "path": range("Sheet1", "A1:B2"), "props": ["values"]}]));
    assert_eq!(back["results"][0]["values"], json!([["keep", null], [null, null]]));
    assert!(gridcraft_plugin::unload(&id));
}

#[test]
fn a_failing_action_changes_nothing_and_names_itself() {
    let _g = serial();
    let id = add_addin("aaaaaaaa-0000-4000-8000-000000000002", "ReadWriteDocument");
    let mut s = session();
    let out = sync(
        &mut s,
        &id,
        json!([
            {"op": "set", "path": range("Sheet1", "A1"), "prop": "values", "value": [[5]]},
            {"op": "set", "path": range("Nowhere", "A1"), "prop": "values", "value": [[6]]},
        ]),
    );
    assert_eq!(out["error"]["code"], "ItemNotFound");
    assert_eq!(out["error"]["action"], 1);
    let a1 = sync(&mut s, &id, json!([{"op": "load", "path": range("Sheet1", "A1"), "props": ["values"]}]));
    assert_eq!(a1["results"][0]["values"], json!([[null]]), "the first action was taken back");
    // Sizes must match, as in Excel; a single value fills.
    let wrong = sync(&mut s, &id, json!([{"op": "set", "path": range("Sheet1", "A1:B2"), "prop": "values", "value": [[1]]}]));
    assert_eq!(wrong["error"]["code"], "InvalidArgument");
    let unknown = sync(&mut s, &id, json!([{"op": "call", "path": range("Sheet1", "A1"), "method": "explode"}]));
    assert_eq!(unknown["error"]["code"], "NotImplemented");
    assert!(gridcraft_plugin::unload(&id));
}

#[test]
fn permissions_bound_what_an_add_in_does() {
    let _g = serial();
    let reader = add_addin("aaaaaaaa-0000-4000-8000-000000000003", "ReadDocument");
    let restricted = add_addin("aaaaaaaa-0000-4000-8000-000000000004", "Restricted");
    let mut s = session();
    let w = sync(&mut s, &reader, json!([{"op": "set", "path": range("Sheet1", "A1"), "prop": "values", "value": 1}]));
    assert_eq!(w["error"]["code"], "AccessDenied");
    assert!(sync(&mut s, &reader, json!([{"op": "load", "path": range("Sheet1", "A1"), "props": ["values"]}]))["results"].is_array());
    let r = sync(&mut s, &restricted, json!([{"op": "load", "path": range("Sheet1", "A1"), "props": ["values"]}]));
    assert_eq!(r["error"]["code"], "AccessDenied");
    // Settings are always the add-in's own.
    let saved = sync(
        &mut s,
        &restricted,
        json!([{"op": "call", "path": [{"p": "settings"}], "method": "save", "args": [{"theme": "dark"}]}, {"op": "load", "path": [{"p": "settings"}]}]),
    );
    assert_eq!(saved["results"][1], json!({"theme": "dark"}));
    let other = sync(&mut s, &reader, json!([{"op": "load", "path": [{"p": "settings"}]}]));
    assert_eq!(other["results"][0], json!({}), "one add-in doesn't see another's settings");
    assert!(gridcraft_plugin::unload(&reader) && gridcraft_plugin::unload(&restricted));
}

#[test]
fn the_selection_sheets_and_common_api() {
    let _g = serial();
    let id = add_addin("aaaaaaaa-0000-4000-8000-000000000005", "ReadWriteDocument");
    let mut s = session();
    s.execute("range.setValues", json!({"range": "A1", "values": [["a", 1], ["b", 2]]})).unwrap();
    s.execute("selection.set", json!({"range": "A1:B2"})).unwrap();
    let out = sync(
        &mut s,
        &id,
        json!([
            {"op": "load", "path": [{"p": "workbook"}, {"m": "getSelectedRange"}], "props": ["address"]},
            {"op": "load", "path": [{"p": "workbook"}, {"m": "getSelectedRange"}, {"m": "getCell", "a": [1, 1]}], "props": ["values", "address"]},
            {"op": "call", "path": [{"p": "document"}], "method": "getSelectedData", "args": ["text"]},
            {"op": "call", "path": [{"p": "document"}], "method": "getSelectedData", "args": ["matrix"]},
            {"op": "call", "path": [{"p": "workbook"}, {"p": "worksheets"}], "method": "add", "args": ["Prices"]},
            {"op": "set", "path": range("Prices", "A1"), "prop": "values", "value": "here"},
            {"op": "call", "path": [{"p": "workbook"}, {"p": "worksheets"}, {"m": "getItem", "a": ["Prices"]}], "method": "activate"},
            {"op": "load", "path": [{"p": "workbook"}, {"p": "worksheets"}, {"m": "getActiveWorksheet"}], "props": ["name", "position"]},
        ]),
    );
    let r = &out["results"];
    assert_eq!(r[0]["address"], "Sheet1!A1:B2", "{out}");
    assert_eq!(r[1]["address"], "Sheet1!B2");
    assert_eq!(r[1]["values"], json!([[2.0]]));
    assert_eq!(r[2], "a\t1\nb\t2");
    assert_eq!(r[3], json!([["a", 1.0], ["b", 2.0]]));
    assert_eq!(r[4]["name"], "Prices");
    assert_eq!(r[7], json!({"name": "Prices", "position": 1}));
    assert!(gridcraft_plugin::unload(&id));
}

#[test]
fn office_commands_open_their_pane() {
    let _g = serial();
    let id = add_addin("aaaaaaaa-0000-4000-8000-000000000006", "ReadDocument");
    let mut s = session();
    let out = s.execute("plugins.run", json!({"plugin": id, "command": "taskpane"})).unwrap();
    assert_eq!(out["office"]["kind"], "showTaskpane");
    assert!(
        s.take_ui_requests().iter().any(
            |q| matches!(q, crate::UiRequest::OfficePane { url, title, .. } if url == "https://tests.example/pane.html" && title == "Sync Test")
        )
    );
    assert!(s.execute("office.sync", json!({"addin": "nope", "actions": []})).is_err());
    assert!(gridcraft_plugin::unload(&id));
}
