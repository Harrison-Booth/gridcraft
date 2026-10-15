//! File menu: new, open, save, close, export, properties, options.

use serde_json::{Value as Json, json};

use super::*;
use crate::DocState;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "file.new", "New Workbook", ["File"], Some("Cmd+N"), "{sample?: \"budget\"|\"sales\"|\"grades\"}", always, new_workbook),
        cmd!(noundo "file.open", "Open…", ["File"], Some("Cmd+O"), "{path} | {name, base64}, plus {password} for a password-protected workbook (xlsx, xlsm, xls import, xlsb and ods data import, csv, tsv, txt, json)", always, open),
        cmd!(noundo "file.save", "Save", ["File"], Some("Cmd+S"), "{path?, password?} (xlsx by default; .csv/.tsv/.json/.html/.pdf by extension). password: a password to open, encrypting the .xlsx (\"\" saves it without one); left out, the workbook's own password applies", has_doc, save),
        cmd!(noundo "file.saveAs", "Save As…", ["File"], Some("Cmd+Shift+S"), "{path, password?} (.xlsx, .csv, .tsv, .json, .html, or .pdf: a PDF of the active sheet); password as for file.save", has_doc, save_as),
        cmd!(query "file.saveBytes", "Encode Workbook", [], None, "{format?: xlsx|csv|tsv|json|html, password?} → {base64}; an xlsx is encrypted with password, else with the workbook's own password", has_doc, save_bytes),
        // Not journaled: the params are a password, and replayed without it the command would fail.
        CommandSpec {
            id: "file.encrypt",
            label: "Encrypt with Password",
            menu: &["File", "Info"],
            shortcut: None,
            params: "{password, confirmPassword?} (\"\" removes it) → {encrypted}: the password to open the workbook with, applied when it is next saved as .xlsx",
            enabled: has_doc,
            run: encrypt,
            journal: false,
            undoable: false,
        },
        cmd!(noundo "file.close", "Close", ["File"], Some("Cmd+W"), "{force?: bool}", has_doc, close),
        cmd!(noundo "file.exportCsv", "Export as CSV", ["File", "Export"], None, "{path, sheet?}", has_doc, |s, p| export(s, p, "csv")),
        cmd!(noundo "file.exportHtml", "Save as Web Page", ["File", "Export"], None, "{path}", has_doc, |s, p| export(s, p, "html")),
        cmd!("file.properties", "Properties", ["File"], None, "{title?, subject?, author?, company?, keywords?, description?}", has_doc, properties),
        cmd!(
            query "file.options",
            "Options",
            ["File"],
            None,
            "{} → {calculation: {mode, iterative, maxIterations, maxChange}} for the active workbook (the interface opens File › Options; change settings with their own commands, e.g. formulas.calculationOptions, view.theme, app.language.set)",
            always,
            options
        ),
        cmd!(noundo "window.activate", "Switch Window", ["Window"], None, "{index}", has_doc, activate_window),
    ]
}

fn new_workbook(s: &mut Session, p: &Json) -> Result<Json> {
    let i = match str_param(p, "sample") {
        Some(name) => {
            let wb = crate::sample::build(name).ok_or_else(|| bad("file.new", format!("unknown sample `{name}`")))?;
            let title = crate::sample::title(name);
            s.add_document(DocState::new(wb, None, title))
        }
        None => s.new_workbook(),
    };
    Ok(json!({"index": i, "title": s.doc()?.display_title()}))
}

fn open(s: &mut Session, p: &Json) -> Result<Json> {
    let (name, bytes) = if let Some(b) = str_param(p, "base64") {
        (str_param(p, "name").unwrap_or("Book.xlsx").to_string(), crate::io::base64_decode(b).ok_or_else(|| bad("file.open", "invalid base64"))?)
    } else if let Some(path) = str_param(p, "path") {
        // Already open? Switch to it.
        if let Some(i) = s.documents().iter().position(|d| d.path.as_deref() == Some(path)) {
            s.set_active(i);
            return Ok(json!({"index": i, "alreadyOpen": true}));
        }
        (path.to_string(), crate::io::read_file(path)?)
    } else {
        s.ui_requests.push(crate::UiRequest::Dialog("open".into(), json!({})));
        return ok();
    };
    let encrypted = gridcraft_xlsx::sniff(&bytes) == gridcraft_xlsx::Format::Encrypted;
    let (mut wb, warnings) = crate::io::open_bytes_with_password(&name, &bytes, str_param(p, "password"))?;
    wb.source_dir = str_param(p, "path").and_then(crate::io::source_dir);
    let imported =
        matches!(crate::io::FileKind::from_path(&name), Some(crate::io::FileKind::Ods | crate::io::FileKind::Xlsb | crate::io::FileKind::Xls))
            || matches!(gridcraft_xlsx::sniff(&bytes), gridcraft_xlsx::Format::Ods | gridcraft_xlsx::Format::Xlsb | gridcraft_xlsx::Format::Xls);
    let title = if imported {
        format!("{} (imported).xlsx", std::path::Path::new(&name).file_stem().and_then(|n| n.to_str()).unwrap_or("Book"))
    } else {
        std::path::Path::new(&name).file_name().and_then(|n| n.to_str()).unwrap_or("Book").to_string()
    };
    // An import has no save target: ordinary Save must ask for a new destination. A workbook
    // decrypted with its password keeps its file and its password, so Save encrypts it again.
    let path = str_param(p, "path").filter(|_| !imported).map(str::to_string);
    // Replace an untouched blank Book1 like Excel does.
    if s.documents().len() == 1
        && s.active().is_some_and(|d| d.path.is_none() && !d.is_dirty() && d.undo.is_empty() && d.wb.sheets.iter().all(|sh| sh.cells.is_empty()))
    {
        s.close_document(0);
    }
    let mut doc = DocState::new(wb, path, title);
    if encrypted {
        doc.password = str_param(p, "password").map(crate::OpenPassword::new);
        doc.saved_password = doc.password.clone();
    }
    if crate::io::FileKind::from_path(&name).is_none() && gridcraft_xlsx::sniff(&bytes) == gridcraft_xlsx::Format::Xlsx {
        doc.content_kind = Some(crate::io::FileKind::Xlsx);
    }
    let i = s.add_document(doc);
    // The decrypted-workbook notice is for the info bar, not a message box.
    let shown: Vec<&str> = warnings.iter().map(String::as_str).filter(|w| *w != crate::io::DECRYPTED_NOTICE).collect();
    if !shown.is_empty() {
        let mut message = shown.iter().take(5).copied().collect::<Vec<_>>().join("\n\n");
        if shown.len() > 5 {
            message.push_str(&format!("\n\n{} additional import warnings.", shown.len() - 5));
        }
        s.ui_requests.push(crate::UiRequest::Message(message));
    }
    let d = s.doc()?;
    Ok(
        json!({"index": i, "title": d.display_title(), "sheets": d.wb.sheets.iter().map(|s| s.name.clone()).collect::<Vec<_>>(), "warnings": warnings}),
    )
}

fn save(s: &mut Session, p: &Json) -> Result<Json> {
    let path = match str_param(p, "path").map(str::to_string).or_else(|| s.active().and_then(|d| d.path.clone())) {
        Some(p) => p,
        None => {
            s.ui_requests.push(crate::UiRequest::Dialog("saveAs".into(), json!({})));
            return ok();
        }
    };
    // `password`: a password to open ("" for none); left out, the workbook's own applies.
    let explicit = str_param(p, "password");
    let is_pdf = std::path::Path::new(&path).extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    if is_pdf && explicit.is_some_and(|pw| !pw.is_empty()) {
        return Err(bad("file.save", crate::io::PASSWORD_NEEDS_XLSX));
    }
    // A PDF is a print of the sheet (Page Setup applies), not a copy of the workbook: the
    // document keeps its file and its unsaved changes.
    if is_pdf {
        return super::print::export_pdf(s, &json!({"path": path}));
    }
    let d = s.doc_mut()?;
    // The document's own file keeps the format its content had when opened; any other path
    // goes by its extension. Relative paths to linked workbooks are rewritten for the new folder.
    let kind = crate::io::FileKind::from_path(&path).or(d.content_kind.filter(|_| d.path.as_deref() == Some(path.as_str())));
    let rebased = crate::io::source_dir(&path).and_then(|f| gridcraft_xlsx::rebase_links(&d.wb, &f));
    let wb = rebased.as_ref().unwrap_or(&d.wb);
    let password = match explicit {
        Some(pw) => (!pw.is_empty()).then(|| crate::OpenPassword::new(pw)),
        None => d.password.clone(),
    };
    // Only an .xlsx holds a password to open; other formats are written plain, as Excel writes
    // them, unless a password was asked for this save.
    let xlsx = kind.unwrap_or(crate::io::FileKind::Xlsx) == crate::io::FileKind::Xlsx;
    if !xlsx && explicit.is_some_and(|pw| !pw.is_empty()) {
        return Err(bad("file.save", crate::io::PASSWORD_NEEDS_XLSX));
    }
    let mut bytes = match kind {
        Some(kind) => crate::io::save_bytes_as(wb, &path, kind)?,
        None => crate::io::save_bytes(wb, &path)?,
    };
    let encrypted = xlsx && password.is_some();
    if let (true, Some(pw)) = (xlsx, &password) {
        bytes = crate::io::encrypt_xlsx(&bytes, pw.expose())?;
    }
    crate::io::write_file(&path, &bytes)?;
    // Only a full-fidelity format clears the dirty flag and becomes the document's file.
    if matches!(kind, Some(crate::io::FileKind::Xlsx | crate::io::FileKind::Json)) {
        d.saved = d.wb.clone();
        d.path = Some(path.clone());
        if explicit.is_some() {
            d.password = password;
        }
        // A JSON copy holds no password, but nothing about the password is unsaved either.
        d.saved_password = d.password.clone();
    }
    Ok(json!({"path": path, "bytes": bytes.len(), "encrypted": encrypted}))
}

fn save_as(s: &mut Session, p: &Json) -> Result<Json> {
    if str_param(p, "path").is_none() {
        s.ui_requests.push(crate::UiRequest::Dialog("saveAs".into(), json!({})));
        return ok();
    }
    save(s, p)
}

fn save_bytes(s: &mut Session, p: &Json) -> Result<Json> {
    let fmt = str_param(p, "format").unwrap_or("xlsx");
    let d = s.doc()?;
    let explicit = str_param(p, "password");
    let password = match explicit {
        Some(pw) => (!pw.is_empty()).then_some(pw),
        None => d.password.as_ref().map(crate::OpenPassword::expose),
    };
    let xlsx = crate::io::FileKind::from_path(&format!("x.{fmt}")) == Some(crate::io::FileKind::Xlsx);
    if !xlsx && explicit.is_some_and(|pw| !pw.is_empty()) {
        return Err(bad("file.saveBytes", crate::io::PASSWORD_NEEDS_XLSX));
    }
    let mut bytes = crate::io::save_bytes(&d.wb, &format!("x.{fmt}"))?;
    if let (true, Some(pw)) = (xlsx, password) {
        bytes = crate::io::encrypt_xlsx(&bytes, pw)?;
    }
    let name = format!("{}.{fmt}", std::path::Path::new(&d.display_title()).file_stem().and_then(|x| x.to_str()).unwrap_or("Book"));
    Ok(json!({"name": name, "base64": crate::io::base64_encode(&bytes), "bytes": bytes.len()}))
}

/// File › Info › Protect Workbook › Encrypt with Password: sets (or, with "", removes) the
/// password to open, which the next save as .xlsx applies. The workbook becomes dirty. A
/// missing password is an error, never a dialog; the interface asks for it itself.
fn encrypt(s: &mut Session, p: &Json) -> Result<Json> {
    let pw = str_param(p, "password").ok_or_else(|| bad("file.encrypt", "missing `password` (\"\" removes the password)"))?;
    if str_param(p, "confirmPassword").is_some_and(|c| c != pw) {
        return Err(bad("file.encrypt", "The confirmation password is not identical."));
    }
    if pw.encode_utf16().count() > gridcraft_xlsx::MAX_PASSWORD_LEN {
        return Err(bad("file.encrypt", "A password to open can have at most 255 characters."));
    }
    let d = s.doc_mut()?;
    d.password = (!pw.is_empty()).then(|| crate::OpenPassword::new(pw));
    Ok(json!({"encrypted": d.password.is_some()}))
}

fn close(s: &mut Session, p: &Json) -> Result<Json> {
    let force = bool_param(p, "force").unwrap_or(false);
    let d = s.doc()?;
    if d.is_dirty() && !force {
        s.ui_requests.push(crate::UiRequest::Dialog("saveChanges".into(), json!({"title": d.display_title()})));
        return Ok(json!({"closed": false, "dirty": true}));
    }
    let i = s.active_index();
    s.close_document(i);
    Ok(json!({"closed": true}))
}

fn export(s: &mut Session, p: &Json, ext: &str) -> Result<Json> {
    let path = str_param(p, "path").ok_or_else(|| bad("file.export", "missing `path`"))?.to_string();
    let d = s.doc()?;
    let mut wb = (*d.wb).clone();
    if p.get("sheet").is_some() {
        wb.active_sheet = target_sheet(s, p)?;
    }
    let target = if path.ends_with(&format!(".{ext}")) { path.clone() } else { format!("{path}.{ext}") };
    let bytes = crate::io::save_bytes(&wb, &target)?;
    crate::io::write_file(&target, &bytes)?;
    Ok(json!({"path": target, "bytes": bytes.len()}))
}

fn properties(s: &mut Session, p: &Json) -> Result<Json> {
    edit(s, |cx| {
        let pr = &mut cx.wb.props;
        for (k, slot) in [
            ("title", &mut pr.title),
            ("subject", &mut pr.subject),
            ("author", &mut pr.author),
            ("company", &mut pr.company),
            ("keywords", &mut pr.keywords),
            ("description", &mut pr.description),
        ] {
            if let Some(v) = str_param(p, k) {
                *slot = v.to_string();
            }
        }
        Ok(serde_json::to_value(&cx.wb.props).unwrap_or_default())
    })
}

/// File › Options: reports the workbook-level settings and asks the interface to show its Options
/// page. Nothing changes here; every setting has its own command.
fn options(s: &mut Session, _: &Json) -> Result<Json> {
    s.ui_requests.push(crate::UiRequest::Dialog("options".into(), json!({})));
    let calculation = s.active().map(|d| {
        let c = &d.wb.calc;
        let mode = match c.mode {
            gridcraft_model::CalcMode::Automatic => "automatic",
            gridcraft_model::CalcMode::AutomaticExceptTables => "automaticExceptTables",
            gridcraft_model::CalcMode::Manual => "manual",
        };
        json!({"mode": mode, "iterative": c.iterative, "maxIterations": c.max_iterations, "maxChange": c.max_change})
    });
    Ok(json!({"calculation": calculation}))
}

fn activate_window(s: &mut Session, p: &Json) -> Result<Json> {
    let i = u32_param(p, "index").ok_or_else(|| bad("window.activate", "missing `index`"))? as usize;
    if i >= s.documents().len() {
        return Err(bad("window.activate", "no such window"));
    }
    s.set_active(i);
    ok()
}
