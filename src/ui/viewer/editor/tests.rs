use super::*;

#[test]
fn save_result_closes_editor_only_on_success() {
    assert!(save_result_closes_editor(SaveResultKind::Success));
    assert!(!save_result_closes_editor(SaveResultKind::Error));
}
