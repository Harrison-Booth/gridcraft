//! Office add-ins' bridge (`office.sync`): one `context.sync()` of an add-in's task pane — a batch
//! of [`Action`]s on objects reached by paths from the workbook — run against the workbook. It
//! is one undoable step; if any action fails, the workbook goes back to how it was and the error
//! names the action. What an add-in may do is bounded by its manifest's permissions.
//!
//! The objects follow the Excel JavaScript API's public reference: `workbook`, `worksheets`,
//! `worksheet`, `range` (values, formulas, text, number formats), `range.format.font`/`fill`,
//! and the common API's selection (`document`) and settings. Writes go through GridCraft's own
//! commands (`range.setValues`, `home.bold`…), reads through `sheet.read` and `cell.get`.

use std::collections::HashMap;

use gridcraft_core::{CellRef, RangeRef};
use gridcraft_plugin::office::{Action, MAX_ACTIONS, Permission, Step};
use serde_json::{Value as Json, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::{EngineError, Result, Session};

/// Most cells one read or write may cover.
const MAX_CELLS: u64 = 1_000_000;
/// Most cells whose formats are read one by one (`numberFormat`).
const MAX_FORMAT_CELLS: u64 = 10_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "office.sync",
        "Office Add-in",
        [],
        None,
        "{addin, actions: [{op: load|set|call, path: [{p}|{m, a}], props?|prop+value|method+args?}]} → {results}. One context.sync() of an Office add-in, as one undoable step",
        always,
        sync
    )]
}

/// An object an action reaches.
#[derive(Clone, Debug, PartialEq)]
enum Obj {
    Root,
    Workbook,
    Worksheets,
    Worksheet(usize),
    Range(usize, RangeRef),
    Format(usize, RangeRef),
    Font(usize, RangeRef),
    Fill(usize, RangeRef),
    /// The common API's `Office.context.document` (selection as text or a matrix).
    Document,
    /// `Office.context.document.settings`.
    Settings,
}

/// An Office.js error: its code (`ItemNotFound`, `InvalidArgument`, `AccessDenied`…) and message.
struct OfficeError(&'static str, String);

type R<T> = std::result::Result<T, OfficeError>;

fn invalid(msg: impl Into<String>) -> OfficeError {
    OfficeError("InvalidArgument", msg.into())
}

fn engine(e: EngineError) -> OfficeError {
    OfficeError("GeneralException", e.to_string())
}

/// Add-in settings, per open workbook and add-in, until they're saved in the file.
fn settings_store() -> &'static std::sync::Mutex<HashMap<(u64, String), Json>> {
    static S: std::sync::OnceLock<std::sync::Mutex<HashMap<(u64, String), Json>>> = std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

fn sync(s: &mut Session, p: &Json) -> Result<Json> {
    const ID: &str = "office.sync";
    let addin = str_param(p, "addin").ok_or_else(|| bad(ID, "missing `addin`"))?;
    let plugin = gridcraft_plugin::get(addin).ok_or_else(|| bad(ID, format!("no add-in `{addin}` is loaded")))?;
    let office = plugin.manifest.office.clone().ok_or_else(|| bad(ID, format!("{addin} isn't an Office add-in")))?;
    if !plugin.enabled() {
        return Err(bad(ID, format!("{addin} is turned off")));
    }
    let actions: Vec<Action> =
        serde_json::from_value(p.get("actions").cloned().unwrap_or(Json::Array(Vec::new()))).map_err(|e| bad(ID, format!("`actions`: {e}")))?;
    if actions.len() > MAX_ACTIONS {
        return Err(bad(ID, format!("at most {MAX_ACTIONS} actions in one sync")));
    }
    // One step: the commands' own undo and journal entries are dropped (this command's entry
    // covers them), and if one fails the workbook goes back to how it was.
    let before = s.active().map(|d| (d.uid, d.wb.clone(), d.selection.clone(), d.undo.len()));
    let journal_len = s.journal.len();
    let mut results = Vec::with_capacity(actions.len());
    let mut failure = None;
    for (i, a) in actions.iter().enumerate() {
        match run_action(s, addin, office.permissions, a) {
            Ok(v) => results.push(v),
            Err(OfficeError(code, message)) => {
                failure = Some(json!({"code": code, "message": message, "action": i}));
                break;
            }
        }
    }
    s.journal.truncate(journal_len);
    if let Some((uid, wb, sel, undo_len)) = before
        && let Some(d) = s.docs.iter_mut().find(|d| d.uid == uid)
    {
        d.undo.truncate(undo_len);
        if failure.is_some() {
            d.wb = wb;
            d.selection = sel;
            d.calc.rebuild(&d.wb);
        }
    }
    match failure {
        // A failed sync is an answer the task pane reads (the promise rejects), not a command error.
        Some(error) => Ok(json!({"error": error})),
        None => Ok(json!({"results": results})),
    }
}

fn need_read(perm: Permission) -> R<()> {
    if perm.reads() { Ok(()) } else { Err(OfficeError("AccessDenied", "the add-in's manifest doesn't let it read the workbook".into())) }
}

fn need_write(perm: Permission) -> R<()> {
    if perm.writes() { Ok(()) } else { Err(OfficeError("AccessDenied", "the add-in's manifest doesn't let it change the workbook".into())) }
}

fn run_action(s: &mut Session, addin: &str, perm: Permission, a: &Action) -> R<Json> {
    match a {
        Action::Load { path, props } => {
            let obj = resolve(s, path)?;
            if !matches!(obj, Obj::Settings) {
                need_read(perm)?;
            }
            load(s, addin, &obj, props)
        }
        Action::Set { path, prop, value } => {
            let obj = resolve(s, path)?;
            need_write(perm)?;
            set(s, &obj, prop, value).map(|_| Json::Null)
        }
        Action::Call { path, method, args } => {
            let obj = resolve(s, path)?;
            match (&obj, method.as_str()) {
                (Obj::Settings, _) => {}
                (Obj::Document, "getSelectedData") => need_read(perm)?,
                _ => need_write(perm)?,
            }
            call(s, addin, &obj, method, args)
        }
    }
}

// ---------------------------------------------------------------- paths

fn sheet_names(s: &Session) -> Vec<String> {
    s.active().map(|d| d.wb.sheets.iter().map(|sh| sh.name.clone()).collect()).unwrap_or_default()
}

fn active_sheet(s: &Session) -> usize {
    s.active().map(|d| d.wb.active_sheet).unwrap_or(0)
}

fn arg_str(args: &[Json], i: usize) -> Option<&str> {
    args.get(i).and_then(Json::as_str)
}

fn arg_i64(args: &[Json], i: usize) -> R<i64> {
    args.get(i)
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().filter(|f| f.is_finite() && f.fract() == 0.0).map(|f| f as i64)))
        .ok_or_else(|| invalid(format!("argument {} must be a whole number", i + 1)))
}

/// `A1:B2`, `B3`, `C:D`, `2:4` (a sheet name before `!` is the caller's to check).
fn parse_range(addr: &str) -> R<RangeRef> {
    let a = addr.trim().trim_start_matches('$');
    let a = a.rsplit('!').next().unwrap_or(a).replace('$', "");
    RangeRef::parse(&a).or_else(|| CellRef::parse(&a).map(RangeRef::cell)).ok_or_else(|| invalid(format!("`{addr}` isn't a range address")))
}

fn shifted(r: RangeRef, dr: i64, dc: i64, height: i64, width: i64) -> R<RangeRef> {
    if height < 1 || width < 1 {
        return Err(invalid("a range needs at least one row and one column"));
    }
    let start = r.start.offset(dr, dc).ok_or_else(|| invalid("the range would leave the sheet"))?;
    let end = start.offset(height - 1, width - 1).ok_or_else(|| invalid("the range would leave the sheet"))?;
    Ok(RangeRef::new(start, end))
}

fn resolve(s: &Session, path: &[Step]) -> R<Obj> {
    let mut obj = Obj::Root;
    for step in path {
        obj = match (&obj, step) {
            (Obj::Root, Step::Prop { p }) if p == "workbook" => Obj::Workbook,
            (Obj::Root, Step::Prop { p }) if p == "document" => Obj::Document,
            (Obj::Root, Step::Prop { p }) if p == "settings" => Obj::Settings,
            (Obj::Workbook, Step::Prop { p }) if p == "worksheets" => Obj::Worksheets,
            (Obj::Workbook, Step::Method { m, .. }) if m == "getSelectedRange" || m == "getActiveCell" => {
                let d = s.active().ok_or_else(|| OfficeError("GeneralException", "no workbook is open".into()))?;
                let r = if m == "getActiveCell" {
                    RangeRef::cell(d.selection.active)
                } else {
                    d.selection.ranges.last().copied().unwrap_or(RangeRef::cell(d.selection.active))
                };
                Obj::Range(d.wb.active_sheet, r)
            }
            (Obj::Worksheets, Step::Method { m, .. }) if m == "getActiveWorksheet" => Obj::Worksheet(active_sheet(s)),
            (Obj::Worksheets, Step::Method { m, a }) if m == "getItem" => {
                let name = arg_str(a, 0).ok_or_else(|| invalid("getItem needs a worksheet name"))?;
                let i = sheet_names(s)
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case(name))
                    .ok_or_else(|| OfficeError("ItemNotFound", format!("no worksheet `{name}`")))?;
                Obj::Worksheet(i)
            }
            (Obj::Worksheet(i), Step::Method { m, a }) if m == "getRange" => {
                let r = match arg_str(a, 0) {
                    Some(addr) if !addr.trim().is_empty() => parse_range(addr)?,
                    _ => RangeRef::all(),
                };
                Obj::Range(*i, r)
            }
            (Obj::Worksheet(i), Step::Method { m, a }) if m == "getCell" => {
                Obj::Range(*i, shifted(RangeRef::cell(CellRef::new(0, 0)), arg_i64(a, 0)?, arg_i64(a, 1)?, 1, 1)?)
            }
            (Obj::Worksheet(i), Step::Method { m, .. }) if m == "getUsedRange" => {
                let used = s.active().and_then(|d| d.wb.sheets.get(*i)).and_then(|sh| sh.used_range()).unwrap_or(RangeRef::cell(CellRef::new(0, 0)));
                Obj::Range(*i, used)
            }
            (Obj::Range(i, r), Step::Method { m, a }) => {
                let (h, w) = (i64::from(r.height()), i64::from(r.width()));
                let r = *r;
                Obj::Range(
                    *i,
                    match m.as_str() {
                        "getCell" => shifted(r, arg_i64(a, 0)?, arg_i64(a, 1)?, 1, 1)?,
                        "getOffsetRange" => shifted(r, arg_i64(a, 0)?, arg_i64(a, 1)?, h, w)?,
                        "getResizedRange" => shifted(r, 0, 0, h + arg_i64(a, 0)?, w + arg_i64(a, 1)?)?,
                        "getRow" => shifted(r, arg_i64(a, 0)?, 0, 1, w)?,
                        "getColumn" => shifted(r, 0, arg_i64(a, 0)?, h, 1)?,
                        "getEntireRow" => RangeRef::rows(r.start.row, r.end.row),
                        "getEntireColumn" => RangeRef::cols(r.start.col, r.end.col),
                        "getUsedRange" | "getUsedRangeOrNullObject" => {
                            let used = s.active().and_then(|d| d.wb.sheets.get(*i)).and_then(|sh| sh.used_range());
                            used.and_then(|u| u.intersection(&r)).unwrap_or(RangeRef::cell(r.start))
                        }
                        other => return Err(OfficeError("NotImplemented", format!("Range.{other}() isn't supported yet"))),
                    },
                )
            }
            (Obj::Range(i, r), Step::Prop { p }) if p == "format" => Obj::Format(*i, *r),
            (Obj::Range(i, _), Step::Prop { p }) if p == "worksheet" => Obj::Worksheet(*i),
            (Obj::Format(i, r), Step::Prop { p }) if p == "font" => Obj::Font(*i, *r),
            (Obj::Format(i, r), Step::Prop { p }) if p == "fill" => Obj::Fill(*i, *r),
            (o, step) => return Err(OfficeError("NotImplemented", format!("{o:?} has no {}", step_name(step)))),
        };
    }
    if let Obj::Worksheet(i) | Obj::Range(i, _) | Obj::Format(i, _) | Obj::Font(i, _) | Obj::Fill(i, _) = obj
        && i >= sheet_names(s).len()
    {
        return Err(OfficeError("ItemNotFound", "the worksheet is gone".into()));
    }
    Ok(obj)
}

fn step_name(step: &Step) -> String {
    match step {
        Step::Prop { p } => format!("property `{p}`"),
        Step::Method { m, .. } => format!("method `{m}()`"),
    }
}

/// `Sheet1!A1:B2`, quoting the sheet's name when it needs it.
fn address(s: &Session, sheet: usize, r: RangeRef) -> String {
    let name = sheet_names(s).get(sheet).cloned().unwrap_or_default();
    let plain = name.chars().all(|c| c.is_alphanumeric() || c == '_') && !name.chars().next().is_some_and(|c| c.is_ascii_digit());
    let sheet = if plain { name } else { format!("'{}'", name.replace('\'', "''")) };
    let a = if r.is_single() { r.start.a1() } else { r.a1() };
    format!("{sheet}!{a}")
}

fn check_size(r: RangeRef) -> R<()> {
    if r.count() > MAX_CELLS { Err(invalid(format!("at most {MAX_CELLS} cells at a time"))) } else { Ok(()) }
}

// ---------------------------------------------------------------- reading

fn read_rows(s: &mut Session, sheet: usize, r: RangeRef, how: Json) -> R<Json> {
    check_size(r)?;
    let mut params = json!({"range": address(s, sheet, r)});
    if let (Some(p), Json::Object(extra)) = (params.as_object_mut(), how) {
        p.extend(extra);
    }
    let out = s.execute("sheet.read", params).map_err(engine)?;
    Ok(out.get("values").cloned().unwrap_or(Json::Array(Vec::new())))
}

/// `cell.get` of the range's top-left cell: its style.
fn style(s: &mut Session, sheet: usize, r: RangeRef) -> R<Json> {
    let out = s.execute("cell.get", json!({"cell": address(s, sheet, RangeRef::cell(r.start))})).map_err(engine)?;
    Ok(out.get("style").cloned().unwrap_or(Json::Null))
}

/// Excel's `#RRGGBB` for a colour as GridCraft writes it (`Auto` is the default).
fn color(v: &Json, auto: &str) -> Json {
    match v.as_str() {
        Some(c) if c.len() == 6 && c.chars().all(|x| x.is_ascii_hexdigit()) => json!(format!("#{}", c.to_ascii_uppercase())),
        Some(c) if c.starts_with('#') => json!(c.to_ascii_uppercase()),
        _ => json!(auto),
    }
}

fn load(s: &mut Session, addin: &str, obj: &Obj, props: &[String]) -> R<Json> {
    let wanted = |all: &[&str]| -> Vec<String> { if props.is_empty() { all.iter().map(|p| p.to_string()).collect() } else { props.to_vec() } };
    let mut out = serde_json::Map::new();
    match obj {
        Obj::Range(i, r) => {
            let (i, r) = (*i, *r);
            for p in wanted(&["address", "values", "rowCount", "columnCount"]) {
                let v = match p.as_str() {
                    "address" | "addressLocal" => json!(address(s, i, r)),
                    "values" | "valuesAsJson" => read_rows(s, i, r, json!({}))?,
                    "formulas" | "formulasLocal" => read_rows(s, i, r, json!({"formulas": true}))?,
                    "text" => read_rows(s, i, r, json!({"formatted": true}))?,
                    "numberFormat" | "numberFormatLocal" => {
                        if r.count() > MAX_FORMAT_CELLS {
                            return Err(invalid(format!("numberFormat reads at most {MAX_FORMAT_CELLS} cells")));
                        }
                        let mut rows = Vec::new();
                        for row in r.start.row..=r.end.row {
                            let mut cells = Vec::new();
                            for col in r.start.col..=r.end.col {
                                let st = style(s, i, RangeRef::cell(CellRef::new(row, col)))?;
                                cells.push(st.get("num_fmt").cloned().unwrap_or(json!("General")));
                            }
                            rows.push(Json::Array(cells));
                        }
                        Json::Array(rows)
                    }
                    "rowCount" => json!(r.height()),
                    "columnCount" => json!(r.width()),
                    "cellCount" => json!(r.count()),
                    "rowIndex" => json!(r.start.row),
                    "columnIndex" => json!(r.start.col),
                    other => return Err(OfficeError("NotImplemented", format!("Range.{other} isn't supported yet"))),
                };
                out.insert(p, v);
            }
        }
        Obj::Font(i, r) => {
            let font = style(s, *i, *r)?.get("font").cloned().unwrap_or(Json::Null);
            for p in wanted(&["bold", "italic", "color", "name", "size", "underline", "strikethrough"]) {
                let v = match p.as_str() {
                    "bold" | "italic" | "name" | "size" => font.get(&p).cloned().unwrap_or(Json::Null),
                    "strikethrough" => font.get("strike").cloned().unwrap_or(Json::Null),
                    "color" => color(font.get("color").unwrap_or(&Json::Null), "#000000"),
                    "underline" => json!(font.get("underline").and_then(Json::as_str).unwrap_or("None")),
                    other => return Err(OfficeError("NotImplemented", format!("RangeFont.{other} isn't supported yet"))),
                };
                out.insert(p, v);
            }
        }
        Obj::Fill(i, r) => {
            let fill = style(s, *i, *r)?.get("fill").cloned().unwrap_or(Json::Null);
            for p in wanted(&["color"]) {
                let v = match p.as_str() {
                    "color" => {
                        let none = fill.get("pattern").and_then(Json::as_str) == Some("None");
                        if none { json!("#FFFFFF") } else { color(fill.get("fg").unwrap_or(&Json::Null), "#FFFFFF") }
                    }
                    other => return Err(OfficeError("NotImplemented", format!("RangeFill.{other} isn't supported yet"))),
                };
                out.insert(p, v);
            }
        }
        Obj::Worksheet(i) => {
            for p in wanted(&["name", "position", "id"]) {
                let v = match p.as_str() {
                    "name" | "id" => json!(sheet_names(s).get(*i).cloned().unwrap_or_default()),
                    "position" => json!(i),
                    other => return Err(OfficeError("NotImplemented", format!("Worksheet.{other} isn't supported yet"))),
                };
                out.insert(p, v);
            }
        }
        Obj::Worksheets => {
            let items: Vec<Json> = sheet_names(s).into_iter().enumerate().map(|(i, n)| json!({"name": n, "id": n, "position": i})).collect();
            out.insert("items".into(), Json::Array(items));
        }
        Obj::Workbook => {
            let name = s.active().map(|d| d.display_title()).unwrap_or_default();
            out.insert("name".into(), json!(name));
        }
        Obj::Settings => return Ok(settings_of(s, addin)),
        Obj::Root | Obj::Format(..) | Obj::Document => return Err(invalid("nothing to load here")),
    }
    Ok(Json::Object(out))
}

fn settings_of(s: &Session, addin: &str) -> Json {
    let uid = s.active().map(|d| d.uid).unwrap_or(0);
    let store = match settings_store().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    store.get(&(uid, addin.to_string())).cloned().unwrap_or_else(|| json!({}))
}

// ---------------------------------------------------------------- writing

/// `value` as rows the size of `r`: a single value fills the range, as in Excel.
fn grid(r: RangeRef, value: &Json) -> R<Json> {
    match value {
        Json::Array(rows) => {
            let ok = rows.len() == r.height() as usize && rows.iter().all(|row| row.as_array().is_some_and(|c| c.len() == r.width() as usize));
            if ok {
                Ok(value.clone())
            } else {
                Err(invalid(format!("the values are not {} rows of {} columns, the range's size", r.height(), r.width())))
            }
        }
        v => {
            check_size(r)?;
            let row = Json::Array(vec![v.clone(); r.width() as usize]);
            Ok(Json::Array(vec![row; r.height() as usize]))
        }
    }
}

fn run(s: &mut Session, id: &str, params: Json) -> R<Json> {
    s.execute(id, params).map_err(engine)
}

fn set(s: &mut Session, obj: &Obj, prop: &str, value: &Json) -> R<()> {
    match (obj, prop) {
        (Obj::Range(i, r), "values" | "formulas" | "formulasLocal" | "valuesAsJson") => {
            check_size(*r)?;
            let values = grid(*r, value)?;
            let at = address(s, *i, RangeRef::cell(r.start));
            run(s, "range.setValues", json!({"range": at, "values": values}))?;
        }
        (Obj::Range(i, r), "numberFormat" | "numberFormatLocal") => {
            check_size(*r)?;
            match value {
                Json::String(code) => {
                    run(s, "home.numberFormat", json!({"range": address(s, *i, *r), "format": code}))?;
                }
                Json::Array(_) => {
                    let rows = grid(*r, value)?;
                    for (dr, row) in rows.as_array().into_iter().flatten().enumerate() {
                        for (dc, code) in row.as_array().into_iter().flatten().enumerate() {
                            let code = code.as_str().ok_or_else(|| invalid("number formats are text"))?;
                            let cell = shifted(*r, dr as i64, dc as i64, 1, 1)?;
                            run(s, "home.numberFormat", json!({"range": address(s, *i, cell), "format": code}))?;
                        }
                    }
                }
                _ => return Err(invalid("numberFormat is text, or rows of text")),
            }
        }
        (Obj::Font(i, r), p) => {
            let range = address(s, *i, *r);
            let on = || value.as_bool().ok_or_else(|| invalid(format!("font.{p} is true or false")));
            match p {
                "bold" => run(s, "home.bold", json!({"range": range, "on": on()?}))?,
                "italic" => run(s, "home.italic", json!({"range": range, "on": on()?}))?,
                "strikethrough" => run(s, "home.strikethrough", json!({"range": range, "on": on()?}))?,
                "underline" => {
                    let style = value.as_str().unwrap_or("None");
                    let (on, style) = match style {
                        "None" => (false, "single"),
                        "Double" => (true, "double"),
                        "SingleAccountant" => (true, "singleAccounting"),
                        "DoubleAccountant" => (true, "doubleAccounting"),
                        _ => (true, "single"),
                    };
                    run(s, "home.underline", json!({"range": range, "on": on, "style": style}))?
                }
                "color" => {
                    run(s, "home.fontColor", json!({"range": range, "color": value.as_str().ok_or_else(|| invalid("a colour is text: #RRGGBB"))?}))?
                }
                "size" => run(s, "home.fontSize", json!({"range": range, "size": value.as_f64().ok_or_else(|| invalid("font.size is a number"))?}))?,
                "name" => run(s, "home.fontName", json!({"range": range, "name": value.as_str().ok_or_else(|| invalid("font.name is text"))?}))?,
                other => return Err(OfficeError("NotImplemented", format!("RangeFont.{other} can't be set yet"))),
            };
        }
        (Obj::Fill(i, r), "color") => {
            let c = value.as_str().ok_or_else(|| invalid("a colour is text: #RRGGBB"))?;
            run(s, "home.fillColor", json!({"range": address(s, *i, *r), "color": c}))?;
        }
        (Obj::Worksheet(i), "name") => {
            let name = value.as_str().ok_or_else(|| invalid("a worksheet's name is text"))?;
            run(s, "sheet.rename", json!({"sheet": i, "name": name}))?;
        }
        (o, p) => return Err(OfficeError("NotImplemented", format!("{o:?}.{p} can't be set yet"))),
    }
    Ok(())
}

fn call(s: &mut Session, addin: &str, obj: &Obj, method: &str, args: &[Json]) -> R<Json> {
    match (obj, method) {
        (Obj::Range(i, r), "clear") => {
            let id = match arg_str(args, 0).unwrap_or("All") {
                "Formats" => "edit.clearFormats",
                "Contents" => "edit.clearContents",
                _ => "edit.clearAll",
            };
            run(s, id, json!({"range": address(s, *i, *r)}))?;
            Ok(Json::Null)
        }
        (Obj::Range(i, r), "select") => {
            if *i != active_sheet(s) {
                run(s, "sheet.activate", json!({"sheet": i}))?;
            }
            let a = if r.is_single() { r.start.a1() } else { r.a1() };
            run(s, "selection.set", json!({"range": a}))?;
            Ok(Json::Null)
        }
        (Obj::Worksheet(i), "activate") => {
            run(s, "sheet.activate", json!({"sheet": i}))?;
            Ok(Json::Null)
        }
        (Obj::Worksheet(i), "delete") => {
            run(s, "home.deleteSheet", json!({"sheet": i}))?;
            Ok(Json::Null)
        }
        (Obj::Worksheets, "add") => {
            let mut params = json!({});
            if let Some(name) = arg_str(args, 0) {
                params["name"] = json!(name);
            }
            let out = run(s, "home.insertSheet", params)?;
            Ok(json!({"name": out.get("name").cloned().unwrap_or(Json::Null)}))
        }
        (Obj::Document, "getSelectedData") => {
            let d = s.active().ok_or_else(|| OfficeError("GeneralException", "no workbook is open".into()))?;
            let (sheet, r) = (d.wb.active_sheet, d.selection.ranges.last().copied().unwrap_or(RangeRef::cell(d.selection.active)));
            let rows = read_rows(s, sheet, r, json!({"formatted": true}))?;
            match arg_str(args, 0).unwrap_or("text") {
                "matrix" => read_rows(s, sheet, r, json!({})),
                _ => {
                    let lines: Vec<String> = rows
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|row| {
                            row.as_array()
                                .into_iter()
                                .flatten()
                                .map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))
                                .collect::<Vec<_>>()
                                .join("\t")
                        })
                        .collect();
                    Ok(json!(lines.join("\n")))
                }
            }
        }
        (Obj::Document, "setSelectedData") => {
            let at = s.active().map(|d| d.selection.ranges.last().map(|r| r.start).unwrap_or(d.selection.active)).unwrap_or_default();
            let data = args.first().cloned().unwrap_or(Json::Null);
            let values = match &data {
                Json::Array(rows) if rows.iter().all(Json::is_array) => data.clone(),
                Json::String(t) => Json::Array(t.split('\n').map(|l| Json::Array(l.split('\t').map(|c| json!(c)).collect())).collect()),
                v => json!([[v]]),
            };
            run(s, "range.setValues", json!({"range": at.a1(), "values": values}))?;
            Ok(Json::Null)
        }
        (Obj::Settings, "save") => {
            let all = args.first().cloned().filter(Json::is_object).ok_or_else(|| invalid("settings are an object"))?;
            if all.to_string().len() > 2 << 20 {
                return Err(invalid("settings are limited to 2 MB"));
            }
            let uid = s.active().map(|d| d.uid).unwrap_or(0);
            let mut store = match settings_store().lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            store.insert((uid, addin.to_string()), all);
            Ok(Json::Null)
        }
        (o, m) => Err(OfficeError("NotImplemented", format!("{o:?}.{m}() isn't supported yet"))),
    }
}

#[cfg(test)]
mod tests;
