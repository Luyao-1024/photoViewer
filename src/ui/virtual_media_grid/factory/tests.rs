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

#[gtk::test]
fn teardown_removes_the_permanent_list_item_child() {
    let _ = gtk::init();
    let list_item: gtk::ListItem = glib::Object::new();
    let tile = SquareTile::new();
    list_item.set_child(Some(&tile));

    teardown_list_item(None, &list_item);

    assert!(
        list_item.child().is_none(),
        "factory teardown must undo setup's set_child call"
    );
    assert!(!list_item.is_activatable());
    assert!(!list_item.is_selectable());
}

#[gtk::test]
fn deferred_thumbnail_paint_waits_for_idle_and_rejects_a_stale_binding() {
    let _ = gtk::init();
    let tile = SquareTile::new();
    tile.show_loading_placeholder();
    let binding = TileBinding::new(2, 3, MediaId::from(7), Some("cache".into()));
    let binding_state = Rc::new(RefCell::new(Some(binding.clone())));
    let bytes = glib::Bytes::from_owned(vec![255_u8, 0, 0, 255]);
    let loaded = crate::core::thumbnails::LoadedThumb {
        texture: gtk::gdk::MemoryTexture::new(1, 1, gtk::gdk::MemoryFormat::R8g8b8a8, &bytes, 4)
            .upcast(),
        is_light: None,
    };

    defer_thumbnail_paint(
        tile.downgrade(),
        glib::WeakRef::new(),
        binding_state.clone(),
        binding.clone(),
        loaded,
    );
    assert!(
        tile.has_css_class("thumb-loading"),
        "painting must not mutate CSS during the factory bind stack"
    );
    while glib::MainContext::default().pending() {
        glib::MainContext::default().iteration(false);
    }
    assert!(!tile.has_css_class("thumb-loading"));

    tile.show_loading_placeholder();
    let stale_bytes = glib::Bytes::from_owned(vec![0_u8, 255, 0, 255]);
    let stale_loaded = crate::core::thumbnails::LoadedThumb {
        texture: gtk::gdk::MemoryTexture::new(
            1,
            1,
            gtk::gdk::MemoryFormat::R8g8b8a8,
            &stale_bytes,
            4,
        )
        .upcast(),
        is_light: None,
    };
    *binding_state.borrow_mut() = Some(TileBinding::new(
        3,
        3,
        MediaId::from(7),
        Some("cache".into()),
    ));
    defer_thumbnail_paint(
        tile.downgrade(),
        glib::WeakRef::new(),
        binding_state,
        binding,
        stale_loaded,
    );
    while glib::MainContext::default().pending() {
        glib::MainContext::default().iteration(false);
    }
    assert!(
        tile.has_css_class("thumb-loading"),
        "a recycled tile must ignore a queued result from its old binding"
    );
}
