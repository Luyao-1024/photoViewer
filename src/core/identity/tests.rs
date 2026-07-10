use super::*;

#[test]
fn media_id_round_trips_i64() {
    let id = MediaId::from(42_i64);
    assert_eq!(id.get(), 42);
    assert_eq!(i64::from(id), 42);
}
