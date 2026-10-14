//! The workbook's own ribbon: tabs, groups and buttons that run the workbook's macros (Excel's
//! ribbon customisation part, `customUI/customUI14.xml`). Buttons the user adds from the
//! Developer tab live here and travel with the file; Excel shows them too.
//!
//! Only ribbons made of tabs, groups and macro buttons are modelled. Richer customisations
//! (pictures, menus, callbacks other than a button's `onAction`) stay in the file unchanged
//! ([`crate::Preserved`]); their buttons still show and run, read-only.

use serde::{Deserialize, Serialize};

/// Most tabs, groups per tab and buttons per group a workbook ribbon may hold.
pub const MAX_RIBBON_ITEMS: usize = 256;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ribbon {
    pub tabs: Vec<RibbonTab>,
}

/// A tab: one of the built-in tabs (`idMso`, `TabHome`) gaining groups, or a tab of its own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RibbonTab {
    /// The control id of a custom tab (`id`); empty for a built-in tab.
    #[serde(default)]
    pub id: String,
    /// The built-in tab (`idMso`, `TabHome`, `TabDeveloper`), when this is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin: Option<String>,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub groups: Vec<RibbonGroup>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RibbonGroup {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub buttons: Vec<RibbonButton>,
}

/// A button that runs a macro: `on_action` is the macro's name (`Module1.Hello`), which Excel
/// calls with the button (`Sub Hello(control As IRibbonControl)`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RibbonButton {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub on_action: String,
    /// A built-in picture by name (`imageMso`): GridCraft draws its own icon for the names it
    /// knows; the name is kept for Excel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default)]
    pub large: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screentip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supertip: Option<String>,
    /// Free text handed to the macro (`control.Tag`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

impl Ribbon {
    pub fn is_empty(&self) -> bool {
        self.tabs.iter().all(|t| t.groups.iter().all(|g| g.buttons.is_empty()))
    }

    /// Every control id in use (tabs, groups, buttons).
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.tabs.iter().flat_map(|t| {
            std::iter::once(t.id.as_str())
                .chain(t.groups.iter().flat_map(|g| std::iter::once(g.id.as_str()).chain(g.buttons.iter().map(|b| b.id.as_str()))))
        })
    }

    /// A control id not in use yet: `prefix1`, `prefix2`…
    pub fn fresh_id(&self, prefix: &str) -> String {
        let mut n = 1usize;
        loop {
            let id = format!("{prefix}{n}");
            if !self.ids().any(|x| x == id) {
                return id;
            }
            n = n.saturating_add(1);
        }
    }

    pub fn button(&self, id: &str) -> Option<&RibbonButton> {
        self.tabs.iter().flat_map(|t| &t.groups).flat_map(|g| &g.buttons).find(|b| b.id == id)
    }

    /// Removes the button `id`, and its group and tab when they are left empty. Whether it was there.
    pub fn remove_button(&mut self, id: &str) -> bool {
        let mut found = false;
        for t in &mut self.tabs {
            for g in &mut t.groups {
                let before = g.buttons.len();
                g.buttons.retain(|b| b.id != id);
                found |= g.buttons.len() != before;
            }
            t.groups.retain(|g| !g.buttons.is_empty());
        }
        self.tabs.retain(|t| !t.groups.is_empty());
        found
    }
}

/// The ribbon tab names GridCraft shows for Excel's built-in tab ids (`TabHome` → Home).
pub const BUILTIN_TABS: &[(&str, &str)] = &[
    ("TabHome", "Home"),
    ("TabInsert", "Insert"),
    ("TabPageLayoutExcel", "Page Layout"),
    ("TabFormulas", "Formulas"),
    ("TabData", "Data"),
    ("TabReview", "Review"),
    ("TabView", "View"),
    ("TabDeveloper", "Developer"),
];

/// The tab GridCraft shows a built-in tab id as (`TabData` → Data).
pub fn builtin_tab_name(id_mso: &str) -> Option<&'static str> {
    BUILTIN_TABS.iter().find(|(k, _)| k.eq_ignore_ascii_case(id_mso)).map(|(_, v)| *v)
}

/// The built-in tab id of a GridCraft tab name (Data → `TabData`).
pub fn builtin_tab_id(name: &str) -> Option<&'static str> {
    BUILTIN_TABS.iter().find(|(_, v)| v.eq_ignore_ascii_case(name)).map(|(k, _)| *k)
}
