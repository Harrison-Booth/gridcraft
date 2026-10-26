//! Help ▸ Report an Issue…: opens a new GitHub issue with the version and system details filled in.
//!
//! The app only opens an address, which carries the details to GitHub as form defaults; no issue
//! is created until the user edits and submits the form. The address carries the field ids of `.github/ISSUE_TEMPLATE/bug_report.yml`
//! (`version`, `os`, `system-info`, `document-info`) and never a file name, path, sheet name or
//! cell content: the document is described by its file extension alone.

use std::fmt::Write as _;

use gridcraft_engine::io::FileKind;
use serde_json::{Value as Json, json};

use crate::SheetApp;

/// The hover text of every Report an Issue… entry: one line, and the catalogs' translation key.
pub(crate) const TIP: &str =
    "Report an Issue: opens a new GitHub issue with the version and system details filled in. No issue is created until you submit it.";
pub(crate) const NEW_ISSUE: &str = "https://github.com/storytold/gridcraft/issues/new";
const TEMPLATE: &str = "bug_report.yml";
/// The longest `version`, `os` and `document-info` value, in UTF-8 bytes.
const VALUE_CAP: usize = 80;
/// The longest `system-info` value, in UTF-8 bytes.
const SYSTEM_CAP: usize = 400;

/// What the issue form is filled in with. Every field is already capped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Facts {
    version: String,
    os: String,
    system_info: String,
    document_info: String,
}

/// The platform the app was built for, e.g. "macOS (aarch64)"; the web build reports "Web".
pub(crate) fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        "freebsd" => "FreeBSD",
        "openbsd" => "OpenBSD",
        "netbsd" => "NetBSD",
        _ if cfg!(target_arch = "wasm32") => return "Web".into(),
        other => other,
    };
    format!("{os} ({})", std::env::consts::ARCH)
}

/// `text` cut to at most `limit` bytes, on a character boundary.
fn bounded(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or_default().to_string()
}

/// Percent-encodes everything but RFC 3986's unreserved characters, so a value can't add a
/// parameter or end the query.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// "Document type: xlsx" for a saved workbook of a format GridCraft opens, "other" for any other
/// extension, "new (unsaved)" for a workbook with no file yet and "none" when nothing is open.
fn document_info(open: bool, path: Option<&str>) -> String {
    let kind = match (open, path) {
        (false, _) => "none".to_string(),
        (true, None) => "new (unsaved)".to_string(),
        (true, Some(path)) => match std::path::Path::new(path).extension().and_then(std::ffi::OsStr::to_str) {
            Some(extension) if FileKind::from_path(path).is_some() => extension.to_ascii_lowercase(),
            _ => "other".to_string(),
        },
    };
    format!("Document type: {kind}")
}

/// Builds the facts from their sources; `graphics` is the host's adapter description.
fn compose(graphics: Option<&str>, language: &str, scale: f32, open: bool, path: Option<&str>) -> Facts {
    // A driver's adapter name is the one free-form text here: keep it to one short line.
    let graphics = graphics.map(|g| g.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>());
    let graphics = graphics.as_deref().map(str::trim).filter(|g| !g.is_empty()).map_or_else(|| "unknown".to_string(), |g| bounded(g, VALUE_CAP));
    let build = if cfg!(debug_assertions) { "debug" } else { "release" };
    let system_info = format!("Build: {build}\nGraphics: {graphics}\nInterface language: {language}\nDisplay scale: {scale}");
    Facts {
        version: bounded(&format!("GridCraft {}", env!("CARGO_PKG_VERSION")), VALUE_CAP),
        os: bounded(&platform(), VALUE_CAP),
        system_info: bounded(&system_info, SYSTEM_CAP),
        document_info: bounded(&document_info(open, path), VALUE_CAP),
    }
}

fn facts(app: &SheetApp) -> Facts {
    let doc = app.session.active();
    compose(app.services.graphics.as_deref(), app.ui.language.code(), app.pixels_per_point, doc.is_some(), doc.and_then(|d| d.path.as_deref()))
}

/// The issue form's address, with the facts as its four prefilled fields. A trailing `body` carries
/// the same details as plain text: GitHub ignores it when the form template exists and uses it to
/// prefill a blank issue while the template isn't on the repository yet, so no detail is lost.
fn url(facts: &Facts) -> String {
    let mut url = format!("{NEW_ISSUE}?template={TEMPLATE}");
    for (field, value) in
        [("version", &facts.version), ("os", &facts.os), ("system-info", &facts.system_info), ("document-info", &facts.document_info)]
    {
        let _ = write!(url, "&{field}={}", encode(value));
    }
    let _ = write!(url, "&body={}", encode(&details_text(facts)));
    url
}

/// The same facts as plain text, for pasting into an issue by hand.
fn details_text(facts: &Facts) -> String {
    format!("Version: {}\nOperating system: {}\n{}\n{}", facts.version, facts.os, facts.system_info, facts.document_info)
}

/// What "Copy Details" puts on the clipboard: the details and the address of a blank new issue.
pub(crate) fn clipboard_text(details: &str) -> String {
    format!("{details}\n\n{NEW_ISSUE}")
}

/// `help.reportIssue`: opens the prefilled issue form in the browser. `{open: false}` only
/// returns the address and details. Returns `{url, details, opened, error}`; `error` says why no
/// browser opened. Never opens a dialog: [`run_menu`] does that for menus and buttons.
pub(crate) fn run(app: &mut SheetApp, params: &Json) -> Result<Json, String> {
    let facts = facts(app);
    let (url, details) = (url(&facts), details_text(&facts));
    let mut error = None;
    let mut opened = false;
    if params.get("open").and_then(Json::as_bool) != Some(false) {
        match &app.services.open_url {
            Some(open) => match open(&url) {
                Ok(()) => opened = true,
                Err(e) => error = Some(e),
            },
            None => error = Some("This window can't open a web browser.".to_string()),
        }
    }
    Ok(json!({"url": url, "details": details, "opened": opened, "error": error}))
}

/// The menu, ribbon and Account entries: like [`run`], and when no browser opened, shows the
/// address and details so the user can still report the issue.
pub(crate) fn run_menu(app: &mut SheetApp, params: Json) {
    match run(app, &params) {
        Ok(result) => {
            if let Some(error) = result.get("error").and_then(Json::as_str) {
                log::warn!("Report an Issue…: {error}");
                app.open_dialog("failedLink", json!({"url": result.get("url"), "details": result.get("details")}));
            }
        }
        Err(e) => app.message = Some(("Report an Issue…".into(), e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The query's parameters, split the way a browser would.
    fn params(url: &str) -> Vec<(&str, &str)> {
        let query = url.split_once('?').map_or("", |(_, query)| query);
        query.split('&').filter_map(|pair| pair.split_once('=')).collect()
    }

    fn names(url: &str) -> Vec<&str> {
        params(url).into_iter().map(|(name, _)| name).collect()
    }

    #[test]
    fn the_address_is_the_new_issue_form_with_six_parameters() {
        let url = url(&compose(Some("Test GPU (Vulkan, DiscreteGpu)"), "de", 1.25, true, Some("book.xlsx")));
        assert!(url.starts_with(&format!("{NEW_ISSUE}?template=bug_report.yml&")), "{url}");
        assert_eq!(names(&url), ["template", "version", "os", "system-info", "document-info", "body"]);
        assert!(url.contains("version=GridCraft%20"), "{url}");
        assert!(url.contains("document-info=Document%20type%3A%20xlsx"), "{url}");
    }

    #[test]
    fn characters_that_end_or_add_a_parameter_stay_inside_their_value() {
        let hostile = "A&b=c?d#e%f\ng ü 日本語 \u{1F600}";
        let facts = compose(Some(hostile), hostile, 1.0, true, None);
        let url = url(&facts);
        assert_eq!(names(&url), ["template", "version", "os", "system-info", "document-info", "body"], "{url}");
        assert!(url.is_ascii(), "{url}");
        assert!(!url.contains('#') && !url.contains(' ') && !url.contains('\n'), "{url}");
        assert_eq!(url.matches('?').count(), 1, "{url}");
        // Decoding the system information gives back the injected text, not a changed query.
        assert!(url.contains("Graphics%3A%20A%26b%3Dc%3Fd%23e%25f%20g"), "{url}");
    }

    #[test]
    fn unreserved_characters_are_kept_and_all_else_is_encoded() {
        assert_eq!(encode("aZ09-_.~"), "aZ09-_.~");
        assert_eq!(encode(" &=?#%/+"), "%20%26%3D%3F%23%25%2F%2B");
        assert_eq!(encode("é日"), "%C3%A9%E6%97%A5");
        assert_eq!(encode("a\nb"), "a%0Ab");
    }

    #[test]
    fn values_are_cut_on_a_character_boundary() {
        assert_eq!(bounded("short", 80), "short");
        assert_eq!(bounded("abcdef", 3), "abc");
        // "é" is two bytes: a cut in its middle drops it.
        assert_eq!(bounded("aéb", 2), "a");
        assert_eq!(bounded("日本語", 4), "日");
        assert_eq!(bounded("日本語", 0), "");
    }

    #[test]
    fn a_huge_adapter_name_cannot_grow_the_address() {
        let huge = "日".repeat(10_000);
        let facts = compose(Some(&huge), &huge, 2.0, true, Some("book.xlsx"));
        assert!(facts.version.len() <= VALUE_CAP && facts.os.len() <= VALUE_CAP && facts.document_info.len() <= VALUE_CAP);
        assert!(facts.system_info.len() <= SYSTEM_CAP, "{}", facts.system_info.len());
        let url = url(&facts);
        assert!(url.len() < 4096, "{} bytes", url.len());
    }

    #[test]
    fn the_longest_possible_address_stays_under_four_kib() {
        // Every byte of every field at its cap, each encoded to three characters, and again in `body`.
        let facts = Facts { version: "é".repeat(40), os: "é".repeat(40), system_info: "\n".repeat(SYSTEM_CAP), document_info: "é".repeat(40) };
        let url = url(&facts);
        assert!(url.len() < 4096, "{} bytes", url.len());
    }

    #[test]
    fn the_document_is_named_by_its_file_type_only() {
        let secret = "/home/ann/Secret Budget.xlsx";
        let facts = compose(None, "en", 1.0, true, Some(secret));
        assert_eq!(facts.document_info, "Document type: xlsx");
        let url = url(&facts);
        let text = details_text(&facts);
        for leak in ["ann", "Secret", "Budget", "home"] {
            assert!(!url.contains(leak) && !text.contains(leak), "{leak} leaked: {url} {text}");
        }
        assert_eq!(document_info(true, Some("C:\\Users\\ann\\Q3.XLSM")), "Document type: xlsm");
        assert_eq!(document_info(true, Some("/tmp/data.csv")), "Document type: csv");
        assert_eq!(document_info(true, Some("/tmp/notes.docx")), "Document type: other");
        assert_eq!(document_info(true, Some("/tmp/no-extension")), "Document type: other");
        assert_eq!(document_info(true, Some("/tmp/Secret.exe\u{1F600}")), "Document type: other");
        assert_eq!(document_info(true, None), "Document type: new (unsaved)");
        assert_eq!(document_info(false, Some("/tmp/a.xlsx")), "Document type: none");
    }

    #[test]
    fn the_system_information_lists_build_graphics_language_and_scale() {
        let facts = compose(Some("Test GPU (Vulkan, DiscreteGpu)"), "pt-br", 1.5, false, None);
        let lines: Vec<&str> = facts.system_info.lines().collect();
        assert!(lines[0] == "Build: debug" || lines[0] == "Build: release", "{lines:?}");
        assert_eq!(lines[1..], ["Graphics: Test GPU (Vulkan, DiscreteGpu)", "Interface language: pt-br", "Display scale: 1.5"]);
        assert_eq!(compose(None, "en", 1.0, false, None).system_info.lines().nth(1), Some("Graphics: unknown"));
        assert_eq!(compose(Some("  \n "), "en", 1.0, false, None).system_info.lines().nth(1), Some("Graphics: unknown"));
        // A driver string with line breaks stays one line.
        assert_eq!(compose(Some("GPU\r\nOS: pwned"), "en", 1.0, false, None).system_info.lines().count(), 4);
    }

    #[test]
    fn the_version_and_platform_come_from_the_build() {
        let facts = compose(None, "en", 1.0, false, None);
        assert_eq!(facts.version, format!("GridCraft {}", env!("CARGO_PKG_VERSION")));
        assert_eq!(facts.os, platform());
        let text = details_text(&facts);
        assert!(text.starts_with("Version: GridCraft "), "{text}");
        assert!(text.contains("\nOperating system: "), "{text}");
        assert!(clipboard_text(&text).ends_with(&format!("\n\n{NEW_ISSUE}")));
    }
}
