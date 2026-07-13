use super::*;

#[test]
fn tile_binding_key_requires_the_slot_generation_media_and_cache_identity() {
    let first = TileBinding::new(1, 3, MediaId::from(7), Some("old".into()));
    let same = TileBinding::new(1, 3, MediaId::from(7), Some("old".into()));
    let rebinding = TileBinding::new(2, 3, MediaId::from(7), Some("old".into()));
    let moved_slot = TileBinding::new(1, 4, MediaId::from(7), Some("old".into()));
    let changed_mtime = TileBinding::new(1, 3, MediaId::from(7), Some("new".into()));

    assert_eq!(first, same);
    assert_ne!(first, rebinding);
    assert_ne!(first, moved_slot);
    assert_ne!(first, changed_mtime);
}
