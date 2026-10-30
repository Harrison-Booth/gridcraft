use egui::{Event, Key, Modifiers};
use gridcraft_engine::Session;
use gridcraft_engine::core::{CellRef, DateSystem, Value};
use serde_json::json;

fn harness(session: Session) -> egui_kittest::Harness<'static, gridcraft_ui_egui::SheetApp> {
    crate::support::harness(session, 4)
}

#[test]
fn date_picker_selects_a_date_as_one_undo_and_keeps_the_cell_format() {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("cell.set", json!({"cell":"A1", "input":"=DATE(2024,2,1)"})).unwrap();
    session.execute("home.numberFormat", json!({"range":"A1", "code":"yyyy-mm-dd"})).unwrap();
    let mut h = harness(session);
    h.state_mut().run("ui.datePicker", json!({})).unwrap();
    let state = h.state_mut().grid.date_picker.as_mut().unwrap();
    state.year = 2024;
    state.month = 2;
    state.day = 29;
    h.input_mut().events.push(Event::Key {
        key: Key::Enter,
        physical_key: Some(Key::Enter),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(1);
    let doc = h.state().session.active().unwrap();
    let sheet = doc.wb.active().unwrap();
    assert_eq!(
        sheet.value_ref(CellRef::new(0, 0)),
        Some(&Value::Number(gridcraft_engine::core::date::serial_from_ymd(DateSystem::D1900, 2024, 2, 29).unwrap()))
    );
    assert_eq!(doc.wb.styles.get(sheet.style_id(CellRef::new(0, 0))).num_fmt, "yyyy-mm-dd");
    assert!(h.state().grid.date_picker.is_none());
    h.state_mut().run("edit.undo", json!({})).unwrap();
    assert_eq!(h.state().session.active().and_then(|d| d.wb.active()).and_then(|s| s.value_ref(CellRef::new(0, 0))), Some(&Value::Number(45323.0)));
}

#[test]
fn date_picker_uses_the_1904_epoch() {
    let mut session = Session::new();
    session.new_workbook();
    session.active_mut().unwrap().wb.date_system = DateSystem::D1904;
    let mut h = harness(session);
    h.state_mut().run("ui.datePicker", json!({})).unwrap();
    let state = h.state_mut().grid.date_picker.as_mut().unwrap();
    state.year = 1904;
    state.month = 1;
    state.day = 1;
    h.input_mut().events.push(Event::Key {
        key: Key::Enter,
        physical_key: Some(Key::Enter),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(1);
    assert_eq!(h.state().session.active().and_then(|d| d.wb.active()).and_then(|s| s.value_ref(CellRef::new(0, 0))), Some(&Value::Number(0.0)));
}

#[test]
fn escape_cancels_without_changing_the_cell() {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("cell.set", json!({"cell":"A1", "input":"42"})).unwrap();
    let mut h = harness(session);
    h.state_mut().run("ui.datePicker", json!({})).unwrap();
    h.input_mut().events.push(Event::Key {
        key: Key::Escape,
        physical_key: Some(Key::Escape),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(1);
    assert!(h.state().grid.date_picker.is_none());
    assert_eq!(h.state().session.active().and_then(|d| d.wb.active()).and_then(|s| s.value_ref(CellRef::new(0, 0))), Some(&Value::Number(42.0)));
}
