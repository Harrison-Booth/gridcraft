//! The library as an add-in sees it — `Excel.run`, proxies, `context.sync()`, the common API —
//! against GridCraft's real engine, in-process (`office.sync` is the transport). Run with
//! `wasm-bindgen-test-runner` (Node).

use std::cell::RefCell;

use gridcraft_engine::Session;
use js_sys::{Array, Function, Promise, Reflect};
use serde_json::{Value as Json, json};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, future_to_promise};
use wasm_bindgen_test::wasm_bindgen_test;

use super::{from_js, install_with_transport, to_js};

thread_local! {
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
    static ADDIN: RefCell<String> = const { RefCell::new(String::new()) };
}

const MANIFEST: &str = r#"<OfficeApp xmlns="http://schemas.microsoft.com/office/appforoffice/1.1" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="TaskPaneApp">
  <Id>bbbbbbbb-0000-4000-8000-000000000001</Id><Version>1.0</Version><ProviderName>Tests</ProviderName><DefaultLocale>en-US</DefaultLocale>
  <DisplayName DefaultValue="Library Test"/><Hosts><Host Name="Workbook"/></Hosts>
  <DefaultSettings><SourceLocation DefaultValue="https://tests.example/pane.html"/></DefaultSettings>
  <Permissions>ReadWriteDocument</Permissions></OfficeApp>"#;

/// A fresh workbook, the test add-in loaded, and the library installed with the engine as its host.
fn setup() {
    let m = gridcraft_plugin::office::parse_manifest(MANIFEST, "gridcraft").unwrap();
    let id = m.id.clone();
    if gridcraft_plugin::get(&id).is_none() {
        gridcraft_plugin::install(m, std::path::PathBuf::from("/addin")).unwrap();
    }
    ADDIN.with(|a| *a.borrow_mut() = id);
    let mut s = Session::new();
    s.new_workbook();
    SESSION.with(|cell| *cell.borrow_mut() = Some(s));
    install_with_transport(|request| {
        let req: Json = serde_json::from_str(&request).unwrap_or(Json::Null);
        let addin = ADDIN.with(|a| a.borrow().clone());
        let answer = SESSION.with(|cell| {
            let mut guard = cell.borrow_mut();
            let s = guard.as_mut().unwrap();
            s.execute("office.sync", json!({"addin": addin, "actions": req["actions"]}))
        });
        let mut out = answer.unwrap_or_else(|e| json!({"error": {"code": "GeneralException", "message": e.to_string()}}));
        out["id"] = req["id"].clone();
        out.to_string()
    });
}

fn engine(id: &str, params: Json) -> Json {
    SESSION.with(|cell| cell.borrow_mut().as_mut().unwrap().execute(id, params).unwrap())
}

fn global(name: &str) -> JsValue {
    Reflect::get(&js_sys::global(), &name.into()).unwrap()
}

fn g(o: &JsValue, k: &str) -> JsValue {
    Reflect::get(o, &k.into()).unwrap()
}

fn try_get(o: &JsValue, k: &str) -> Result<JsValue, JsValue> {
    Reflect::get(o, &k.into())
}

fn set(o: &JsValue, k: &str, v: Json) {
    assert!(Reflect::set(o, &k.into(), &to_js(&v)).unwrap(), "{k} is settable");
}

fn call(o: &JsValue, name: &str, args: &[JsValue]) -> JsValue {
    let f: Function = g(o, name).dyn_into().unwrap_or_else(|_| panic!("{name} is a function"));
    f.apply(o, &args.iter().cloned().collect::<Array>()).unwrap()
}

fn s(v: &str) -> JsValue {
    v.into()
}

fn code(e: &JsValue) -> String {
    g(e, "code").as_string().unwrap_or_default()
}

/// `Excel.run(batch)` with an async Rust batch; resolves with what the batch returns.
async fn excel_run<F>(batch: impl FnOnce(JsValue) -> F + 'static) -> Result<JsValue, JsValue>
where
    F: std::future::Future<Output = Result<JsValue, JsValue>> + 'static,
{
    let batch = Closure::once_into_js(move |ctx: JsValue| -> Promise { future_to_promise(batch(ctx)) });
    let run = g(&global("Excel"), "run");
    let p: Promise = run.dyn_into::<Function>().unwrap().call1(&JsValue::UNDEFINED, &batch).unwrap().dyn_into().unwrap();
    JsFuture::from(p).await
}

async fn sync(ctx: &JsValue) -> Result<JsValue, JsValue> {
    JsFuture::from(call(ctx, "sync", &[]).dyn_into::<Promise>().unwrap()).await
}

fn range(ctx: &JsValue, sheet: &str, addr: &str) -> JsValue {
    let sheets = g(&g(ctx, "workbook"), "worksheets");
    call(&call(&sheets, "getItem", &[s(sheet)]), "getRange", &[s(addr)])
}

#[wasm_bindgen_test]
async fn excel_run_writes_and_reads_back_after_sync() {
    setup();
    let out = excel_run(|ctx| async move {
        let r = range(&ctx, "Sheet1", "A1:B2");
        set(&r, "values", json!([[1, 2], [3, "=A1+B1"]]));
        set(&g(&g(&r, "format"), "font"), "bold", json!(true));
        call(&r, "load", &[s("values, address, formulas")]);
        // Not loaded until the sync: reading it throws PropertyNotLoaded.
        let early = try_get(&r, "address").unwrap_err();
        assert_eq!(code(&early), "PropertyNotLoaded");
        assert_eq!(from_js(&g(&r, "values"))[0][1], json!(2), "a value just set is known at once");
        sync(&ctx).await?;
        let font = g(&g(&r, "format"), "font");
        call(&font, "load", &[s("bold")]);
        sync(&ctx).await?;
        Ok(to_js(&json!({"values": from_js(&g(&r, "values")), "address": from_js(&g(&r, "address")), "formulas": from_js(&g(&r, "formulas")), "bold": from_js(&g(&font, "bold"))})))
    })
    .await
    .unwrap();
    let out = from_js(&out);
    assert_eq!(out["values"], json!([[1, 2], [3, 3]]), "numbers come back as JavaScript numbers");
    assert_eq!(out["address"], "Sheet1!A1:B2");
    assert_eq!(out["formulas"][1][1], "=A1+B1");
    assert_eq!(out["bold"], true);
    // One undo step took it all.
    engine("edit.undo", json!({}));
    assert_eq!(engine("sheet.read", json!({"range": "A1:B2"}))["values"], json!([[null, null], [null, null]]));
}

#[wasm_bindgen_test]
async fn a_failing_sync_rejects_with_the_code() {
    setup();
    let e = excel_run(|ctx| async move {
        set(&range(&ctx, "Nowhere", "A1"), "values", json!(1));
        sync(&ctx).await
    })
    .await
    .unwrap_err();
    assert_eq!(code(&e), "ItemNotFound");
    assert_eq!(g(&e, "name").as_string().as_deref(), Some("RichApi.Error"));
    assert!(e.is_instance_of::<js_sys::Error>(), "OfficeExtension.Error is an Error");
    // Excel.run without a sync still sends what was queued.
    excel_run(|ctx| async move {
        set(&range(&ctx, "Sheet1", "C3"), "values", json!("queued"));
        Ok(JsValue::UNDEFINED)
    })
    .await
    .unwrap();
    assert_eq!(engine("sheet.read", json!({"range": "C3"}))["values"], json!([["queued"]]));
}

#[wasm_bindgen_test]
async fn worksheets_are_added_listed_and_written() {
    setup();
    let names = excel_run(|ctx| async move {
        let sheets = g(&g(&ctx, "workbook"), "worksheets");
        let prices = call(&sheets, "add", &[s("Prices")]);
        set(&call(&prices, "getRange", &[s("B2")]), "values", json!([["here"]]));
        call(&prices, "activate", &[]);
        call(&sheets, "load", &[s("items/name")]);
        sync(&ctx).await?;
        let items: Array = g(&sheets, "items").dyn_into()?;
        let names: Vec<String> = items.iter().map(|w| g(&w, "name").as_string().unwrap_or_default()).collect();
        let active = call(&sheets, "getActiveWorksheet", &[]);
        call(&active, "load", &[s("name")]);
        sync(&ctx).await?;
        Ok(to_js(&json!({"names": names, "active": from_js(&g(&active, "name"))})))
    })
    .await
    .unwrap();
    let out = from_js(&names);
    assert_eq!(out["names"], json!(["Sheet1", "Prices"]));
    assert_eq!(out["active"], "Prices");
    assert_eq!(engine("sheet.read", json!({"range": "Prices!B2"}))["values"], json!([["here"]]));
}

/// Calls `Office.context.document.<name>(first, callback)` and waits for the callback's result.
async fn document_async(name: &str, first: JsValue) -> JsValue {
    let doc = g(&g(&global("Office"), "context"), "document");
    let name = name.to_string();
    let p = Promise::new(&mut |resolve, _| {
        let _ = call(&doc, &name, &[first.clone(), resolve.into()]);
    });
    JsFuture::from(p).await.unwrap()
}

#[wasm_bindgen_test]
async fn the_common_api_reads_and_writes_the_selection() {
    setup();
    engine("range.setValues", json!({"range": "A1", "values": [["a", 1], ["b", 2]]}));
    engine("selection.set", json!({"range": "A1:B2"}));
    let text = document_async("getSelectedDataAsync", s("text")).await;
    assert_eq!(g(&text, "status").as_string().as_deref(), Some("succeeded"));
    assert_eq!(g(&text, "value").as_string().as_deref(), Some("a\t1\nb\t2"));
    let matrix = document_async("getSelectedDataAsync", s("matrix")).await;
    assert_eq!(from_js(&g(&matrix, "value")), json!([["a", 1], ["b", 2]]));
    engine("selection.set", json!({"range": "D4"}));
    let set_r = document_async("setSelectedDataAsync", to_js(&json!([["x", "y"]]))).await;
    assert_eq!(g(&set_r, "status").as_string().as_deref(), Some("succeeded"));
    assert_eq!(engine("sheet.read", json!({"range": "D4:E4"}))["values"], json!([["x", "y"]]));
    let html = document_async("getSelectedDataAsync", s("html")).await;
    assert_eq!(g(&html, "status").as_string().as_deref(), Some("failed"));
    assert_eq!(g(&g(&html, "error"), "code").as_string().as_deref(), Some("NotImplemented"));
}

#[wasm_bindgen_test]
async fn settings_save_and_come_back_and_requirements_answer() {
    setup();
    let office = global("Office");
    let info = JsFuture::from(call(&office, "onReady", &[]).dyn_into::<Promise>().unwrap()).await.unwrap();
    assert_eq!(g(&info, "host").as_string().as_deref(), Some("Excel"));
    let settings = g(&g(&g(&office, "context"), "document"), "settings");
    call(&settings, "set", &[s("theme"), s("dark")]);
    assert_eq!(call(&settings, "get", &[s("theme")]).as_string().as_deref(), Some("dark"));
    let saved = document_async_on(&settings, "saveAsync").await;
    assert_eq!(g(&saved, "status").as_string().as_deref(), Some("succeeded"));
    // Installed again (the pane reloaded), the settings come back from the host.
    install_with_transport_again();
    let _ = JsFuture::from(call(&office, "onReady", &[]).dyn_into::<Promise>().unwrap()).await.unwrap();
    assert_eq!(call(&settings, "get", &[s("theme")]).as_string().as_deref(), Some("dark"));
    assert!(call(&settings, "get", &[s("missing")]).is_null());
    let req = g(&g(&office, "context"), "requirements");
    assert_eq!(call(&req, "isSetSupported", &[s("ExcelApi"), s("1.4")]).as_bool(), Some(true));
    assert_eq!(call(&req, "isSetSupported", &[s("ExcelApi"), s("1.9")]).as_bool(), Some(false));
    assert_eq!(call(&req, "isSetSupported", &[s("WordApi"), s("1.1")]).as_bool(), Some(false));
    // The globals can't be replaced by a later script (Microsoft's office.js, say).
    let _ = Reflect::set(&js_sys::global(), &"Office".into(), &JsValue::from(1));
    assert!(global("Office").is_object());
}

async fn document_async_on(o: &JsValue, name: &str) -> JsValue {
    let o = o.clone();
    let name = name.to_string();
    let p = Promise::new(&mut |resolve, _| {
        let _ = call(&o, &name, &[resolve.into()]);
    });
    JsFuture::from(p).await.unwrap()
}

/// Reinstalls over the same session (the settings stay in the host).
fn install_with_transport_again() {
    install_with_transport(|request| {
        let req: Json = serde_json::from_str(&request).unwrap_or(Json::Null);
        let addin = ADDIN.with(|a| a.borrow().clone());
        let answer =
            SESSION.with(|cell| cell.borrow_mut().as_mut().unwrap().execute("office.sync", json!({"addin": addin, "actions": req["actions"]})));
        let mut out = answer.unwrap_or_else(|e| json!({"error": {"code": "GeneralException", "message": e.to_string()}}));
        out["id"] = req["id"].clone();
        out.to_string()
    });
}
