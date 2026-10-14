//! The workbook's ribbon part (customUI): modelled when it is tabs, groups and macro buttons,
//! kept unchanged when it is richer, written as `customUI/customUI14.xml`.

use std::io::Read;

use gridcraft_model::{Ribbon, RibbonButton, RibbonGroup, RibbonTab, Workbook};

use super::make_zip;
use crate::custom_ui::{parse_buttons, parse_exact};
use crate::{read_xlsx, write_xlsx};

const WB: &str = r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
const WB_RELS: &str = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#;
const CT: &str = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/></Types>"#;
const SHEET: &str = r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/></worksheet>"#;

const SIMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<customUI xmlns="http://schemas.microsoft.com/office/2009/07/customui">
  <ribbon>
    <tabs>
      <tab id="tabTools" label="Tools">
        <group id="grpReports" label="Reports">
          <button id="btnFill" label="Fill &amp; total" size="large" imageMso="HappyFace" onAction="Module1.FillTable" screentip="Fills the table"/>
          <button id="btnClear" label="Clear" onAction="Clear" tag="all"/>
        </group>
      </tab>
      <tab idMso="TabHome">
        <group id="grpQuick" label="Quick">
          <button id="btnHello" label="Hello" onAction="Hello"/>
        </group>
      </tab>
    </tabs>
  </ribbon>
</customUI>"#;

/// A package with a ribbon part at `customUI/customUI14.xml` (and optionally its own relationships).
fn package(custom_ui: &str, ui_rels: Option<&str>) -> Vec<u8> {
    let root = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/><Relationship Id="rId5" Type="http://schemas.microsoft.com/office/2007/relationships/ui/extensibility" Target="customUI/customUI14.xml"/></Relationships>"#;
    let mut parts: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", CT.as_bytes()),
        ("_rels/.rels", root.as_bytes()),
        ("xl/workbook.xml", WB.as_bytes()),
        ("xl/_rels/workbook.xml.rels", WB_RELS.as_bytes()),
        ("xl/worksheets/sheet1.xml", SHEET.as_bytes()),
        ("customUI/customUI14.xml", custom_ui.as_bytes()),
    ];
    if let Some(r) = ui_rels {
        parts.push(("customUI/_rels/customUI14.xml.rels", r.as_bytes()));
        parts.push(("customUI/images/logo.png", b"\x89PNG not really"));
    }
    make_zip(&parts)
}

fn part(bytes: &[u8], name: &str) -> Option<String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let mut s = String::new();
    z.by_name(name).ok()?.read_to_string(&mut s).ok()?;
    Some(s)
}

#[test]
fn a_ribbon_of_macro_buttons_is_modelled() {
    let (wb, rep) = read_xlsx(&package(SIMPLE, None)).unwrap();
    let r = wb.ribbon.as_ref().expect("modelled");
    assert_eq!(r.tabs.len(), 2);
    assert_eq!(r.tabs[0].label, "Tools");
    assert_eq!(r.tabs[1].builtin.as_deref(), Some("TabHome"));
    let fill = r.button("btnFill").unwrap();
    assert_eq!((fill.label.as_str(), fill.on_action.as_str(), fill.large), ("Fill & total", "Module1.FillTable", true));
    assert_eq!(fill.image.as_deref(), Some("HappyFace"));
    assert_eq!(r.button("btnClear").unwrap().tag.as_deref(), Some("all"));
    assert!(rep.warnings.iter().all(|w| !w.contains("ribbon") && !w.contains("extensibility")), "{:?}", rep.warnings);
    // Modelled means not kept as well: one ribbon part on save, the model's.
    assert!(wb.preserved.as_deref().is_none_or(|p| p.parts.iter().all(|q| !q.name.to_ascii_lowercase().contains("customui"))));

    let out = write_xlsx(&wb).unwrap();
    let xml = part(&out, "customUI/customUI14.xml").expect("written");
    assert!(xml.contains("2009/07/customui") && xml.contains("onAction=\"Module1.FillTable\""), "{xml}");
    let rels = part(&out, "_rels/.rels").unwrap();
    assert!(rels.contains("2007/relationships/ui/extensibility") && rels.contains("customUI/customUI14.xml"), "{rels}");
    let (back, _) = read_xlsx(&out).unwrap();
    assert_eq!(back.ribbon, wb.ribbon, "the ribbon survives a round trip");
}

#[test]
fn a_richer_ribbon_is_kept_unchanged_and_its_buttons_still_found() {
    // A menu, a load callback and a picture of its own: not modelled.
    let rich = r#"<customUI xmlns="http://schemas.microsoft.com/office/2009/07/customui" onLoad="RibbonLoaded"><ribbon><tabs><tab id="t" label="Mine"><group id="g" label="G"><menu id="m" label="More"><button id="b1" label="Inside" onAction="Inside"/></menu><button id="b2" image="logo" onAction="Logo"/><button idMso="Copy"/></group></tab></tabs></ribbon></customUI>"#;
    let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="logo" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="images/logo.png"/></Relationships>"#;
    assert!(parse_exact(rich.as_bytes()).is_none());
    let (wb, rep) = read_xlsx(&package(rich, Some(rels))).unwrap();
    assert!(wb.ribbon.is_none());
    assert!(wb.preserved.is_some(), "{:?}", rep.warnings);
    let out = write_xlsx(&wb).unwrap();
    // Kept byte for byte (under whatever part name the writer gave it).
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(out.clone())).unwrap();
    let names: Vec<String> = z.file_names().map(str::to_string).collect();
    let ui = names.iter().find(|n| n.to_ascii_lowercase().starts_with("customui/") && n.ends_with(".xml")).expect("kept");
    let mut kept = String::new();
    z.by_name(ui).unwrap().read_to_string(&mut kept).unwrap();
    assert_eq!(kept, rich);
    assert!(names.iter().any(|n| n.ends_with("logo.png")));
    let found = parse_buttons(rich.as_bytes());
    let labels: Vec<&str> = found.tabs[0].groups[0].buttons.iter().map(|b| b.label.as_str()).collect();
    assert_eq!(labels, ["Inside", "b2"], "menu buttons found, built-in commands left out");
}

#[test]
fn a_ribbon_is_written_for_a_new_workbook_and_not_when_empty() {
    let mut wb = Workbook::new();
    wb.ribbon = Some(Ribbon::default());
    assert!(part(&write_xlsx(&wb).unwrap(), "customUI/customUI14.xml").is_none(), "an empty ribbon writes nothing");
    wb.ribbon = Some(Ribbon {
        tabs: vec![RibbonTab {
            builtin: Some("TabDeveloper".into()),
            groups: vec![RibbonGroup {
                id: "g1".into(),
                label: "My <macros>".into(),
                buttons: vec![RibbonButton { id: "b1".into(), label: "Run \"it\"".into(), on_action: "Module1.Go".into(), ..Default::default() }],
            }],
            ..Default::default()
        }],
    });
    let out = write_xlsx(&wb).unwrap();
    assert_eq!(read_xlsx(&out).unwrap().0.ribbon, wb.ribbon);
    assert!(parse_exact(b"<customUI><ribbon startFromScratch=\"true\"/></customUI>").is_none());
    assert!(parse_exact(b"<notUI/>").is_none());
    assert!(parse_buttons(b"<<<").tabs.is_empty(), "hostile input finds nothing");
}
