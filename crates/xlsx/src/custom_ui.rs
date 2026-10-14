//! The workbook's ribbon part (`customUI/customUI14.xml`, Office 2010 namespace, or the Office
//! 2007 `customUI/customUI.xml`), from the published RibbonX schema.
//!
//! [`parse_exact`] models a ribbon only when the model can write all of it back (tabs, groups,
//! macro buttons); anything richer stays in the file as kept content, and [`parse_buttons`] still
//! finds its macro buttons so they show and run.

use std::fmt::Write as _;

use gridcraft_model::ribbon::MAX_RIBBON_ITEMS;
use gridcraft_model::{Ribbon, RibbonButton, RibbonGroup, RibbonTab};

use crate::xml::{self, El, esc_attr};

pub const NS_CUSTOM_UI14: &str = "http://schemas.microsoft.com/office/2009/07/customui";
pub const REL_CUSTOM_UI14: &str = "http://schemas.microsoft.com/office/2007/relationships/ui/extensibility";
pub const PART_CUSTOM_UI14: &str = "customUI/customUI14.xml";

/// Whether the part is the Office 2010 flavour (preferred when a file holds both).
pub fn is_v14(bytes: &[u8]) -> bool {
    bytes.windows(b"2009/07/customui".len()).any(|w| w == b"2009/07/customui")
}

fn only(e: &El, allowed: &[&str]) -> bool {
    e.text.trim().is_empty() && e.attrs.iter().all(|(k, v)| allowed.contains(&k.as_str()) || (k == "startFromScratch" && v == "false"))
}

/// The ribbon, when it is made only of what the model holds; `None` otherwise (or when it isn't
/// a ribbon part at all).
pub fn parse_exact(bytes: &[u8]) -> Option<Ribbon> {
    let root = xml::parse(bytes).ok()?;
    if root.name != "customUI" || !only(&root, &[]) {
        return None;
    }
    let mut out = Ribbon::default();
    for ribbon in &root.children {
        if ribbon.name != "ribbon" || !only(ribbon, &[]) {
            return None;
        }
        for tabs in &ribbon.children {
            if tabs.name != "tabs" || !only(tabs, &[]) {
                return None;
            }
            for tab in &tabs.children {
                if tab.name != "tab" || !only(tab, &["id", "idMso", "label"]) || tab.children.len() > MAX_RIBBON_ITEMS {
                    return None;
                }
                let builtin = tab.attr("idMso").map(str::to_string);
                let id = tab.attr("id").unwrap_or("").to_string();
                if builtin.is_some() == !id.is_empty() {
                    return None;
                }
                let mut t = RibbonTab { id, builtin, label: tab.attr("label").unwrap_or("").to_string(), groups: Vec::new() };
                for group in &tab.children {
                    if group.name != "group" || !only(group, &["id", "label"]) || group.children.len() > MAX_RIBBON_ITEMS {
                        return None;
                    }
                    let mut g =
                        RibbonGroup { id: group.attr("id")?.to_string(), label: group.attr("label").unwrap_or("").to_string(), buttons: Vec::new() };
                    for b in &group.children {
                        let allowed = ["id", "label", "onAction", "imageMso", "size", "screentip", "supertip", "tag"];
                        if b.name != "button" || !only(b, &allowed) || !b.children.is_empty() {
                            return None;
                        }
                        let large = match b.attr("size") {
                            None | Some("normal") => false,
                            Some("large") => true,
                            Some(_) => return None,
                        };
                        g.buttons.push(button(b, b.attr("id")?, large)?);
                    }
                    t.groups.push(g);
                }
                out.tabs.push(t);
            }
        }
    }
    (out.tabs.len() <= MAX_RIBBON_ITEMS).then_some(out)
}

fn button(b: &El, id: &str, large: bool) -> Option<RibbonButton> {
    let s = |k: &str| b.attr(k).map(str::to_string);
    Some(RibbonButton {
        id: id.to_string(),
        label: s("label").unwrap_or_default(),
        on_action: s("onAction")?,
        image: s("imageMso"),
        large,
        screentip: s("screentip"),
        supertip: s("supertip"),
        tag: s("tag"),
    })
}

/// The macro buttons of any ribbon part, wherever they sit in a group (menus and split buttons
/// included), for showing a ribbon that isn't modelled. Buttons without a fixed label are named
/// by their id; built-in commands (`idMso`) and buttons without `onAction` are left out.
pub fn parse_buttons(bytes: &[u8]) -> Ribbon {
    let mut out = Ribbon::default();
    let Ok(root) = xml::parse(bytes) else { return out };
    let tabs = root.path(&["ribbon", "tabs"]).map(|t| t.children.as_slice()).unwrap_or(&[]);
    for tab in tabs.iter().filter(|t| t.name == "tab").take(MAX_RIBBON_ITEMS) {
        if tab.flag("visible", true) {
            let mut t = RibbonTab {
                id: tab.attr("id").or(tab.attr("idQ")).unwrap_or("").to_string(),
                builtin: tab.attr("idMso").map(str::to_string),
                label: tab.attr("label").unwrap_or("").to_string(),
                groups: Vec::new(),
            };
            for group in tab.kids("group").take(MAX_RIBBON_ITEMS) {
                let mut g = RibbonGroup {
                    id: group.attr("id").or(group.attr("idQ")).unwrap_or("").to_string(),
                    label: group.attr("label").unwrap_or("").to_string(),
                    buttons: Vec::new(),
                };
                collect(group, &mut g.buttons, 0);
                if !g.buttons.is_empty() {
                    t.groups.push(g);
                }
            }
            if !t.groups.is_empty() {
                out.tabs.push(t);
            }
        }
    }
    out
}

fn collect(e: &El, out: &mut Vec<RibbonButton>, depth: usize) {
    if depth > 8 {
        return;
    }
    for c in &e.children {
        if out.len() >= MAX_RIBBON_ITEMS {
            return;
        }
        if c.name == "button" && c.attr("idMso").is_none() && c.flag("visible", true) {
            let large = c.attr("size") == Some("large");
            if let Some(mut b) = button(c, c.attr("id").or(c.attr("idQ")).unwrap_or(""), large) {
                if b.label.is_empty() {
                    b.label = b.id.clone();
                }
                out.push(b);
            }
        } else {
            collect(c, out, depth + 1);
        }
    }
}

/// The ribbon as a `customUI14.xml` part.
pub fn write(r: &Ribbon) -> String {
    let mut x = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<customUI xmlns=\"{NS_CUSTOM_UI14}\"><ribbon><tabs>");
    for t in &r.tabs {
        match &t.builtin {
            Some(mso) => {
                let _ = write!(x, "<tab idMso=\"{}\"", esc_attr(mso));
            }
            None => {
                let _ = write!(x, "<tab id=\"{}\"", esc_attr(&t.id));
            }
        }
        if !t.label.is_empty() {
            let _ = write!(x, " label=\"{}\"", esc_attr(&t.label));
        }
        x.push('>');
        for g in &t.groups {
            let _ = write!(x, "<group id=\"{}\" label=\"{}\">", esc_attr(&g.id), esc_attr(&g.label));
            for b in &g.buttons {
                let _ = write!(x, "<button id=\"{}\" label=\"{}\"", esc_attr(&b.id), esc_attr(&b.label));
                if b.large {
                    x.push_str(" size=\"large\"");
                }
                for (k, v) in [("imageMso", &b.image), ("screentip", &b.screentip), ("supertip", &b.supertip), ("tag", &b.tag)] {
                    if let Some(v) = v {
                        let _ = write!(x, " {k}=\"{}\"", esc_attr(v));
                    }
                }
                let _ = write!(x, " onAction=\"{}\"/>", esc_attr(&b.on_action));
            }
            x.push_str("</group>");
        }
        x.push_str("</tab>");
    }
    x.push_str("</tabs></ribbon></customUI>");
    x
}
