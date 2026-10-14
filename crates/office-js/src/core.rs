//! What a `RequestContext` keeps between syncs, independent of JavaScript: the queued actions,
//! where each load's result goes, and the properties known so far, keyed by object path. Proxies
//! are a context and a path, so `range.format.font` reached twice is the same object.

use std::collections::HashMap;

use serde_json::{Map, Value as Json, json};

/// A path from a root (`workbook`, `document`, `settings`) to an object: the steps of
/// `gridcraft_plugin::office::Step` as JSON (`{"p": "worksheets"}`, `{"m": "getItem", "a": ["S"]}`).
pub type Path = Vec<Json>;

/// A property step.
pub fn prop(name: &str) -> Json {
    json!({"p": name})
}

/// A method step.
pub fn method(name: &str, args: Vec<Json>) -> Json {
    json!({"m": name, "a": args})
}

/// `path` with `step` after it.
pub fn child(path: &[Json], step: Json) -> Path {
    let mut p = path.to_vec();
    p.push(step);
    p
}

fn key(path: &[Json]) -> String {
    serde_json::to_string(path).unwrap_or_default()
}

/// An Office.js error (`OfficeExtension.Error`): its code, message and which action failed.
#[derive(Clone, Debug, PartialEq)]
pub struct OfficeError {
    pub code: String,
    pub message: String,
    pub action: Option<u64>,
}

impl OfficeError {
    pub fn new(code: &str, message: impl Into<String>) -> OfficeError {
        OfficeError { code: code.into(), message: message.into(), action: None }
    }
    pub fn not_loaded(prop: &str) -> OfficeError {
        OfficeError::new(
            "PropertyNotLoaded",
            format!(
                "The property '{prop}' is not available. Before reading the property's value, call the load method on the containing object and call \"context.sync()\" on the associated request context."
            ),
        )
    }
    pub fn not_implemented(what: &str) -> OfficeError {
        OfficeError::new("NotImplemented", format!("{what} isn't supported by GridCraft yet"))
    }
}

/// Loads waiting for a sync's answer: the action's index, the object's path.
pub struct Pending {
    loads: Vec<(usize, Path)>,
}

#[derive(Default)]
pub struct Ctx {
    queue: Vec<Json>,
    loads: Vec<(usize, Path)>,
    cache: HashMap<String, Map<String, Json>>,
}

impl Ctx {
    pub fn new() -> Ctx {
        Ctx::default()
    }

    /// Whether actions are waiting for a sync.
    pub fn has_queued(&self) -> bool {
        !self.queue.is_empty()
    }

    /// Queues loading `props` of the object at `path`. A property path (`format/font/bold`)
    /// loads that property of the object it leads to; no properties loads the defaults.
    pub fn load(&mut self, path: &[Json], props: &[String]) {
        let mut own = Vec::new();
        let mut nested: Vec<(Path, Vec<String>)> = Vec::new();
        for p in props.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
            // A collection's `items/name`: its items, with their properties.
            if p == "items" || p.starts_with("items/") {
                if !own.iter().any(|o: &String| o == "items") {
                    own.push("items".to_string());
                }
                continue;
            }
            let parts: Vec<&str> = p.split('/').map(str::trim).collect();
            match parts.split_last() {
                Some((last, [])) => own.push(last.to_string()),
                Some((last, steps)) => {
                    let mut sub = path.to_vec();
                    sub.extend(steps.iter().map(|s| prop(s)));
                    match nested.iter_mut().find(|(q, _)| *q == sub) {
                        Some((_, list)) => list.push(last.to_string()),
                        None => nested.push((sub, vec![last.to_string()])),
                    }
                }
                None => {}
            }
        }
        if !own.is_empty() || nested.is_empty() {
            self.loads.push((self.queue.len(), path.to_vec()));
            self.queue.push(json!({"op": "load", "path": path, "props": own}));
        }
        for (sub, list) in nested {
            self.loads.push((self.queue.len(), sub.clone()));
            self.queue.push(json!({"op": "load", "path": sub, "props": list}));
        }
    }

    /// Queues setting a property; the new value is known at once.
    pub fn set(&mut self, path: &[Json], prop: &str, value: Json) {
        self.queue.push(json!({"op": "set", "path": path, "prop": prop, "value": value.clone()}));
        self.cache.entry(key(path)).or_default().insert(prop.to_string(), value);
    }

    /// Queues a method called for its effect.
    pub fn call(&mut self, path: &[Json], method: &str, args: Vec<Json>) {
        self.queue.push(json!({"op": "call", "path": path, "method": method, "args": args}));
    }

    /// A property known so far (loaded, or set since).
    pub fn get(&self, path: &[Json], prop: &str) -> Option<&Json> {
        self.cache.get(&key(path)).and_then(|m| m.get(prop))
    }

    /// The queued actions, taken for one sync, and the loads waiting for its answer.
    pub fn take(&mut self) -> (Vec<Json>, Pending) {
        (std::mem::take(&mut self.queue), Pending { loads: std::mem::take(&mut self.loads) })
    }

    /// Reads a sync's answer: `{"results": [...]}` fills the loaded objects; `{"error": …}` is the
    /// error the sync rejects with.
    pub fn apply(&mut self, pending: Pending, reply: &Json) -> Result<Json, OfficeError> {
        let results = results(reply)?;
        for (i, path) in pending.loads {
            let Some(Json::Object(props)) = results.get(i) else { continue };
            // A collection's items become objects of their own: `worksheets.items[0].name`.
            if let Some(Json::Array(items)) = props.get("items") {
                for item in items {
                    if let Some(name) = item.get("name").and_then(Json::as_str)
                        && let Json::Object(fields) = item
                    {
                        let at = child(&path, method("getItem", vec![json!(name)]));
                        self.cache.entry(key(&at)).or_default().extend(fields.clone());
                    }
                }
            }
            self.cache.entry(key(&path)).or_default().extend(props.clone());
        }
        Ok(Json::Array(results.to_vec()))
    }
}

/// A sync's answer: the results, one per action, or the error it carries.
pub fn results(reply: &Json) -> Result<&[Json], OfficeError> {
    if let Some(e) = reply.get("error") {
        return Err(OfficeError {
            code: e.get("code").and_then(Json::as_str).unwrap_or("GeneralException").to_string(),
            message: e.get("message").and_then(Json::as_str).unwrap_or("The request failed").to_string(),
            action: e.get("action").and_then(Json::as_u64),
        });
    }
    reply
        .get("results")
        .and_then(Json::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| OfficeError::new("GeneralException", "the host's answer has no results"))
}

/// The property names a `load()` argument names: `"a, b"`, `["a", "b"]`, `{select: "a,b"}` or
/// `{a: true, b: true}` (already turned into JSON).
pub fn load_props(arg: &Json) -> Vec<String> {
    match arg {
        Json::String(s) => s.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect(),
        Json::Array(list) => list.iter().filter_map(Json::as_str).map(str::to_string).collect(),
        Json::Object(o) => match o.get("select") {
            Some(sel) => load_props(sel),
            None => o.iter().filter(|(_, v)| v.as_bool() == Some(true)).map(|(k, _)| k.clone()).collect(),
        },
        _ => Vec::new(),
    }
}

/// `Office.context.requirements.isSetSupported(name, minVersion)`.
pub fn is_set_supported(name: &str, min: Option<&str>) -> bool {
    let supported: &[(&str, (u32, u32))] =
        &[("ExcelApi", (1, 4)), ("Settings", (1, 1)), ("Selection", (1, 1)), ("TextCoercion", (1, 1)), ("MatrixCoercion", (1, 1))];
    let Some((_, have)) = supported.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) else { return false };
    let want = min.map(version).unwrap_or((1, 1));
    want <= *have
}

fn version(v: &str) -> (u32, u32) {
    let mut parts = v.trim().split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (parts.next().unwrap_or(1), parts.next().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range() -> Path {
        vec![prop("workbook"), method("getSelectedRange", vec![])]
    }

    #[test]
    fn loads_fill_the_objects_their_paths_name() {
        let mut c = Ctx::new();
        let r = range();
        assert!(c.get(&r, "values").is_none());
        c.load(&r, &["values".into(), "format/font/bold".into(), "address".into()]);
        let (actions, pending) = c.take();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0], json!({"op": "load", "path": r, "props": ["values", "address"]}));
        assert_eq!(actions[1]["path"][3], json!({"p": "font"}));
        assert!(!c.has_queued());
        c.apply(pending, &json!({"results": [{"values": [[1]], "address": "Sheet1!A1"}, {"bold": true}]})).unwrap();
        assert_eq!(c.get(&r, "values"), Some(&json!([[1]])));
        let font = child(&child(&r, prop("format")), prop("font"));
        assert_eq!(c.get(&font, "bold"), Some(&json!(true)));
    }

    #[test]
    fn sets_are_known_at_once_and_errors_carry_their_code() {
        let mut c = Ctx::new();
        let r = range();
        c.set(&r, "values", json!([[5]]));
        assert_eq!(c.get(&r, "values"), Some(&json!([[5]])));
        c.call(&r, "clear", vec![json!("Formats")]);
        let (actions, pending) = c.take();
        assert_eq!(actions[1]["method"], "clear");
        let e = c.apply(pending, &json!({"error": {"code": "ItemNotFound", "message": "no sheet", "action": 1}})).unwrap_err();
        assert_eq!((e.code.as_str(), e.action), ("ItemNotFound", Some(1)));
    }

    #[test]
    fn collection_items_become_objects() {
        let mut c = Ctx::new();
        let ws = vec![prop("workbook"), prop("worksheets")];
        c.load(&ws, &["items/name".into(), "items/position".into()]);
        assert_eq!(c.take().0, vec![json!({"op": "load", "path": ws, "props": ["items"]})]);
        c.load(&ws, &["items".into()]);
        let (_, pending) = c.take();
        c.apply(pending, &json!({"results": [{"items": [{"name": "Data", "position": 0}]}]})).unwrap();
        assert_eq!(c.get(&child(&ws, method("getItem", vec![json!("Data")])), "position"), Some(&json!(0)));
    }

    #[test]
    fn load_arguments_and_requirement_sets() {
        assert_eq!(load_props(&json!("values, address")), ["values", "address"]);
        assert_eq!(load_props(&json!(["text"])), ["text"]);
        assert_eq!(load_props(&json!({"select": "a,b"})), ["a", "b"]);
        assert_eq!(load_props(&json!({"values": true, "text": false})), ["values"]);
        assert!(is_set_supported("ExcelApi", Some("1.4")) && is_set_supported("excelapi", Some("1.1")) && is_set_supported("ExcelApi", None));
        assert!(!is_set_supported("ExcelApi", Some("1.5")) && !is_set_supported("ExcelApi", Some("1.10")));
        assert!(!is_set_supported("WordApi", Some("1.1")));
    }
}
