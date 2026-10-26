//! Help ▸ Report an Issue…: opens GitHub's issue form with the version and system details filled
//! in, creates no issue itself, and when no browser opens keeps the details on screen.

use std::sync::{Arc, Mutex};

use egui::{Key, OutputCommand};
use egui_kittest::kittest::Queryable as _;
use gridcraft_ui_egui::{Services, SheetApp};
use serde_json::json;

use crate::common::Harness;

const FORM: &str = "https://github.com/storytold/gridcraft/issues/new?template=bug_report.yml&";

type Opened = Arc<Mutex<Vec<String>>>;

/// A host whose `open_url` records each address and answers with `result`.
fn host(result: Result<(), String>) -> (Services, Opened) {
    let opened = Opened::default();
    let capture = opened.clone();
    let services = Services {
        open_url: Some(Box::new(move |url| {
            capture.lock().unwrap().push(url.to_string());
            result.clone()
        })),
        graphics: Some("Test GPU (Vulkan, DiscreteGpu)".into()),
        ..Default::default()
    };
    (services, opened)
}

fn harness(services: Services) -> Harness {
    crate::support::services_harness(services, 4)
}

fn click_report(h: &mut Harness) {
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Report an Issue…").click();
    h.run_steps(3);
}

fn automate(h: &mut Harness) {
    h.state_mut().ui.ribbon_tab = "Automate".into();
    h.run_steps(3);
}

fn account(h: &mut Harness) {
    h.get_by_label("File").click();
    h.run_steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Account").click();
    h.run_steps(3);
}

/// The query's parameter names, in order.
fn names(url: &str) -> Vec<&str> {
    url.split_once('?').map_or("", |(_, query)| query).split('&').filter_map(|pair| pair.split_once('=')).map(|(name, _)| name).collect()
}

fn assert_prefilled(url: &str) {
    assert!(url.starts_with(FORM), "{url}");
    assert_eq!(names(url), ["template", "version", "os", "system-info", "document-info", "body"], "{url}");
    assert!(url.contains("Graphics%3A%20Test%20GPU%20%28Vulkan%2C%20DiscreteGpu%29"), "the host's adapter is in the system information: {url}");
    assert!(url.contains("Interface%20language%3A%20en"), "{url}");
    assert!(url.len() < 2048, "{} bytes", url.len());
}

#[test]
fn the_automate_tab_opens_the_prefilled_form_once() {
    let (services, opened) = host(Ok(()));
    let mut h = harness(services);
    automate(&mut h);
    click_report(&mut h);
    let urls = opened.lock().unwrap();
    assert_eq!(urls.len(), 1, "one click opens one page");
    assert_prefilled(&urls[0]);
    assert!(urls[0].contains("document-info=Document%20type%3A%20new%20%28unsaved%29"), "{}", urls[0]);
    assert!(h.state().dialog.is_none(), "the GitHub form is the review screen: no dialog of ours");
}

#[test]
fn the_account_page_opens_the_prefilled_form_over_the_file_view() {
    let (services, opened) = host(Ok(()));
    let mut h = harness(services);
    account(&mut h);
    click_report(&mut h);
    let urls = opened.lock().unwrap();
    assert_eq!(urls.len(), 1);
    assert_prefilled(&urls[0]);
    assert!(h.state().backstage.is_some(), "the File view stays open");
    assert!(h.state().dialog.is_none());
}

#[test]
fn a_failed_open_keeps_the_address_and_details_until_closed() {
    let (services, opened) = host(Err("no web browser is installed".into()));
    let mut h = harness(services);
    automate(&mut h);
    click_report(&mut h);
    assert_eq!(opened.lock().unwrap().len(), 1);
    assert_eq!(h.state().dialog.as_ref().map(|d| d.name()), Some("failedLink"));
    for label in ["Link", "Details"] {
        assert!(h.query_all_by_label(label).count() >= 1, "the dialog labels its {label} field");
    }
    h.run_steps(40);
    assert_eq!(h.state().dialog.as_ref().map(|d| d.name()), Some("failedLink"), "it stays open until it is closed");

    // Copy Details puts the details and the address of a blank new issue on the clipboard.
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Copy Details").click();
    h.step();
    let copied = h.output().platform_output.commands.iter().find_map(|c| match c {
        OutputCommand::CopyText(text) => Some(text.clone()),
        _ => None,
    });
    let copied = copied.expect("Copy Details copies text");
    assert!(copied.starts_with("Version: GridCraft "), "{copied}");
    assert!(copied.contains("\nGraphics: Test GPU (Vulkan, DiscreteGpu)\n"), "{copied}");
    assert!(copied.contains("\nDocument type: new (unsaved)"), "{copied}");
    assert!(copied.ends_with("\n\nhttps://github.com/storytold/gridcraft/issues/new"), "{copied}");
    assert_eq!(h.state().dialog.as_ref().map(|d| d.name()), Some("failedLink"), "copying doesn't close it");

    h.get_by_role_and_label(egui::accesskit::Role::Button, "Close").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
}

#[test]
fn escape_closes_the_failed_link_dialog() {
    let (services, _) = host(Err("blocked".into()));
    let mut h = harness(services);
    automate(&mut h);
    click_report(&mut h);
    assert_eq!(h.state().dialog.as_ref().map(|d| d.name()), Some("failedLink"));
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
}

#[test]
fn without_a_host_opener_the_menu_still_shows_the_details() {
    let mut h = harness(Services::default());
    automate(&mut h);
    click_report(&mut h);
    assert_eq!(h.state().dialog.as_ref().map(|d| d.name()), Some("failedLink"));
}

#[test]
fn the_command_returns_the_address_and_opens_it() {
    let (services, opened) = host(Ok(()));
    let mut h = harness(services);
    let result = h.state_mut().run("help.reportIssue", json!({})).unwrap();
    let url = result["url"].as_str().unwrap();
    assert_prefilled(url);
    assert_eq!(result["opened"], true);
    assert!(result["error"].is_null());
    assert!(result["details"].as_str().unwrap().starts_with("Version: GridCraft "));
    assert_eq!(*opened.lock().unwrap(), [url]);
}

#[test]
fn open_false_returns_the_address_and_opens_nothing() {
    let (services, opened) = host(Ok(()));
    let mut h = harness(services);
    let result = h.state_mut().run("help.reportIssue", json!({"open": false})).unwrap();
    assert_prefilled(result["url"].as_str().unwrap());
    assert_eq!(result["opened"], false);
    assert!(result["error"].is_null());
    assert!(opened.lock().unwrap().is_empty());
    assert!(h.state().dialog.is_none());
}

#[test]
fn a_programmatic_call_reports_a_failed_open_without_a_dialog() {
    let (services, opened) = host(Err("no web browser is installed".into()));
    let mut h = harness(services);
    let result = h.state_mut().run("help.reportIssue", json!({})).unwrap();
    assert_eq!(result["opened"], false);
    assert_eq!(result["error"], "no web browser is installed");
    assert_prefilled(result["url"].as_str().unwrap());
    assert_eq!(opened.lock().unwrap().len(), 1);
    h.run_steps(3);
    assert!(h.state().dialog.is_none(), "programmatic calls never open dialogs");
}

#[test]
fn a_host_that_cannot_open_pages_says_so() {
    let mut app = SheetApp::new(gridcraft_engine::Session::new(), Services::default());
    let result = app.run("help.reportIssue", json!({})).unwrap();
    assert_eq!(result["opened"], false);
    assert!(result["error"].as_str().is_some_and(|e| !e.is_empty()), "{result}");
    assert!(result["url"].as_str().is_some_and(|url| url.starts_with(FORM)));
}

#[test]
fn a_saved_workbook_is_described_by_its_file_type_alone() {
    let dir = std::env::temp_dir().join(format!("gridcraft-report-ann-private-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Secret Budget.xlsx");
    let (services, _) = host(Ok(()));
    let mut h = harness(services);
    h.state_mut().run("cell.set", json!({"cell": "A1", "input": "Confidential figure"})).unwrap();
    h.state_mut().run("file.saveAs", json!({"path": path.to_string_lossy()})).unwrap();
    let result = h.state_mut().run("help.reportIssue", json!({"open": false})).unwrap();
    let url = result["url"].as_str().unwrap();
    let details = result["details"].as_str().unwrap();
    assert!(url.contains("document-info=Document%20type%3A%20xlsx"), "{url}");
    assert!(details.contains("\nDocument type: xlsx"), "{details}");
    for leak in ["ann-private", "Secret", "Budget", "Confidential", "gridcraft-report"] {
        assert!(!url.contains(leak) && !details.contains(leak), "{leak} leaked: {url}\n{details}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
