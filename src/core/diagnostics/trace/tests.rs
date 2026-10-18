use super::*;

#[test]
fn a_full_trace_drops_whole_events_and_finishes_as_valid_json() {
    let output = tempfile::NamedTempFile::new().unwrap();
    let mut writer = Writer::new(output.reopen().unwrap(), 128).unwrap();
    writer.write_all(b"[\n{\"name\":\"first\"},\n").unwrap();
    writer
        .write_all(format!("{{\"name\":\"{}\"}},\n", "x".repeat(256)).as_bytes())
        .unwrap();
    writer.write_all(b"{\"name\":\"last\"}\n]").unwrap();
    writer.flush().unwrap();
    let data = std::fs::read(output.path()).unwrap();
    assert!(data.len() <= 128);
    let values: serde_json::Value = serde_json::from_slice(&data).unwrap();
    assert_eq!(values.as_array().unwrap().len(), 2);
}
