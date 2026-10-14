//! Repeat (F4, and Ctrl+Y with nothing to redo): the last repeatable command a person ran from
//! the ribbon, a key tip, a shortcut, a context menu or a dialog, run again on the current
//! selection. UI-only: the record lives in [`crate::SheetApp`], and the engine sees an ordinary
//! command whose target params were dropped, so it acts on the selection as usual.
//!
//! As observed in Excel: toggles (Bold, Wrap Text) repeat the state they set; insert rows or
//! columns follow the size of the new selection; moving the selection, Undo, and a cancelled
//! dialog leave the record alone; typing into a cell and a change Repeat can't run again (Clear
//! Contents, Clear Formats, Clear All) leave nothing to repeat. Commands run over the control
//! channel or MCP are agent actions, not the person's, so they are neither recorded nor clear it.

use serde_json::{Map, Value as Json};

/// The command [`crate::SheetApp::repeat_last`] runs again.
#[derive(Clone, Debug, PartialEq)]
pub struct Last {
    pub id: String,
    pub params: Json,
}

/// Commands that Repeat can run again. Formatting, borders, number formats, fills, styles,
/// alignment, insert/delete of cells, rows, columns and sheets, row height / column width and
/// Paste Special. Clear and Fill are left out: Excel doesn't repeat them.
const REPEATABLE: &[&str] = &[
    // Font
    "home.bold",
    "home.italic",
    "home.underline",
    "home.doubleUnderline",
    "home.strikethrough",
    "home.superscript",
    "home.subscript",
    "home.fontName",
    "home.fontSize",
    "home.increaseFontSize",
    "home.decreaseFontSize",
    "home.fontColor",
    "home.fillColor",
    "home.borders",
    "home.formatCells",
    "home.cellStyle",
    // Alignment
    "home.alignLeft",
    "home.alignCenter",
    "home.alignRight",
    "home.justify",
    "home.alignTop",
    "home.alignMiddle",
    "home.alignBottom",
    "home.wrapText",
    "home.shrinkToFit",
    "home.increaseIndent",
    "home.decreaseIndent",
    "home.orientation",
    "home.mergeCenter",
    "home.mergeAcross",
    "home.mergeCells",
    "home.unmergeCells",
    // Number
    "home.numberFormat",
    "home.accounting",
    "home.percent",
    "home.comma",
    "home.increaseDecimal",
    "home.decreaseDecimal",
    // Cells
    "home.insertCells",
    "home.insertRows",
    "home.insertColumns",
    "home.insertSheet",
    "home.deleteCells",
    "home.deleteRows",
    "home.deleteColumns",
    "home.rowHeight",
    "home.columnWidth",
    "home.autofitRowHeight",
    "home.autofitColumnWidth",
    "home.hideRows",
    "home.hideColumns",
    "home.unhideRows",
    "home.unhideColumns",
    // Clipboard
    "edit.pasteSpecial",
];

/// Params naming where a command acts. Repeat drops them so the command acts on the current
/// selection instead.
const TARGET_PARAMS: &[&str] = &["range", "rows", "cols", "cell", "sheet"];

/// Toggle commands report the state they set as `{"on": bool}` (Bold, Italic, Wrap Text…).
/// Repeat sets that state again rather than flipping it, as Excel does: repeating Bold on a cell
/// that is already bold leaves it bold.
const TOGGLES: &[&str] =
    &["home.bold", "home.italic", "home.underline", "home.strikethrough", "home.superscript", "home.subscript", "home.wrapText", "home.shrinkToFit"];

/// Whether Repeat can run `id` again.
pub fn repeatable(id: &str) -> bool {
    REPEATABLE.contains(&id)
}

/// Whether running `id` (when it isn't repeatable) leaves nothing to repeat: a change to the
/// workbook. Selection, view and Undo/Redo commands don't count, nor do UI-only commands.
pub fn ends_repeat(id: &str) -> bool {
    !repeatable(id)
        && !id.starts_with("view.")
        && !matches!(id, "edit.undo" | "edit.redo")
        && gridcraft_engine::find_command(id).is_some_and(|c| c.undoable)
}

/// The record for `id` run with `params` (which returned `result`), or `None` when Repeat can't
/// run it again.
pub fn record(id: &str, params: &Json, result: &Json) -> Option<Last> {
    if !repeatable(id) {
        return None;
    }
    let mut map = match params {
        Json::Object(map) => map.clone(),
        Json::Null => Map::new(),
        // Commands take object params; anything else can't be retargeted.
        _ => return None,
    };
    for key in TARGET_PARAMS {
        map.remove(*key);
    }
    if TOGGLES.contains(&id)
        && let Some(on) = result.get("on").and_then(Json::as_bool)
    {
        map.insert("on".into(), Json::Bool(on));
    }
    Some(Last { id: id.to_string(), params: Json::Object(map) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn records_repeatable_commands_without_their_target() {
        let last = record("home.borders", &json!({"range": "A1:B2", "preset": "outside"}), &Json::Null).unwrap();
        assert_eq!(last, Last { id: "home.borders".into(), params: json!({"preset": "outside"}) });
        let last = record("home.insertRows", &json!({"rows": "3:4", "count": 2}), &Json::Null).unwrap();
        assert_eq!(last.params, json!({"count": 2}));
        assert_eq!(record("home.fillColor", &Json::Null, &Json::Null).unwrap().params, json!({}));
    }

    #[test]
    fn toggles_repeat_the_state_they_set() {
        assert_eq!(record("home.bold", &json!({}), &json!({"on": true})).unwrap().params, json!({"on": true}));
        assert_eq!(record("home.wrapText", &json!({"range": "C3"}), &json!({"on": false})).unwrap().params, json!({"on": false}));
    }

    #[test]
    fn navigation_typing_undo_and_odd_params_are_not_recorded() {
        for id in ["selection.set", "selection.move", "cell.set", "edit.undo", "edit.redo", "edit.copy", "edit.goTo", "file.save", "view.zoom"] {
            assert_eq!(record(id, &json!({}), &Json::Null), None, "{id}");
        }
        assert_eq!(record("home.bold", &json!([1]), &Json::Null), None);
    }

    #[test]
    fn workbook_changes_that_cant_repeat_end_the_record() {
        for id in ["edit.clearContents", "edit.clearFormats", "edit.fillDown", "cell.set", "data.sortAscending"] {
            assert!(ends_repeat(id), "{id}");
        }
        for id in ["home.bold", "selection.set", "selection.move", "edit.undo", "edit.redo", "edit.copy", "view.zoom", "ui.ribbonTab"] {
            assert!(!ends_repeat(id), "{id}");
        }
    }
}
