//! The painted grid exposes the active cell to assistive tech: address, value and formula (gap 18).

use egui::accesskit::Role;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use gridcraft_engine::Session;
use gridcraft_ui_egui::SheetApp;
use serde_json::json;

#[test]
fn active_cell_is_announced_with_address_value_and_formula() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "A1", "input": "2"})).unwrap();
    s.execute("cell.set", json!({"cell": "B2", "input": "=A1*3"})).unwrap();
    s.execute("selection.set", json!({"range": "B2"})).unwrap();
    let mut h = crate::common::builder().with_size(egui::vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        crate::common::app(s),
    );
    h.run_steps(4);
    let cell = h.get_by_role_and_label(Role::Cell, "B2");
    let node = cell.accesskit_node();
    assert_eq!(node.value().as_deref(), Some("6"));
    assert_eq!(node.data().description(), Some("=A1*3"));
}
