//! New charts get style 2 (Excel's default look), so the saved `<c:style>` is not the grey style 1.

use gridcraft_engine::Session;
use serde_json::json;

#[test]
fn insert_chart_defaults_to_style_two() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Q", "N"], ["Q1", "10"], ["Q2", "30"]]})).unwrap();
    s.execute("insert.chart", json!({"range": "A1:B3"})).unwrap();
    assert_eq!(s.doc().unwrap().wb.sheet(0).unwrap().charts[0].style, 2);
}
