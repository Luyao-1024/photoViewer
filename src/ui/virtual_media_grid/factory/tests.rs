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
        std::time::Instant::now(),
        true,
        false,
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
        std::time::Instant::now(),
        true,
        false,
    );
    while glib::MainContext::default().pending() {
        glib::MainContext::default().iteration(false);
    }
    assert!(
        tile.has_css_class("thumb-loading"),
        "a recycled tile must ignore a queued result from its old binding"
    );
}

/// A 1×1 `LoadedThumb` whose only distinguishing field is `is_light`, so the
/// placeholder-vs-full ordering is observable via `background_is_light()`.
fn loaded_with_light(is_light: Option<bool>) -> crate::core::thumbnails::LoadedThumb {
    let bytes = glib::Bytes::from_owned(vec![255_u8, 0, 0, 255]);
    crate::core::thumbnails::LoadedThumb {
        texture: gtk::gdk::MemoryTexture::new(1, 1, gtk::gdk::MemoryFormat::R8g8b8a8, &bytes, 4)
            .upcast(),
        is_light,
    }
}

/// An EXIF placeholder paints immediately but must NOT mark the full-thumbnail
/// flag — only a real thumbnail arriving later does (and wins).
#[gtk::test]
fn exif_placeholder_paints_without_marking_full_until_real_thumbnail() {
    let _ = gtk::init();
    let tile = SquareTile::new();
    tile.show_loading_placeholder();
    let binding = TileBinding::new(2, 3, MediaId::from(7), Some("cache".into()));
    let binding_state = Rc::new(RefCell::new(Some(binding.clone())));

    defer_thumbnail_paint(
        tile.downgrade(),
        glib::WeakRef::new(),
        binding_state.clone(),
        binding.clone(),
        loaded_with_light(Some(false)),
        std::time::Instant::now(),
        true,
        true,
    );
    while glib::MainContext::default().pending() {
        glib::MainContext::default().iteration(false);
    }
    assert_eq!(tile.background_is_light(), Some(false));
    assert!(
        !tile.full_thumbnail_painted(),
        "a placeholder must not mark the full-thumbnail flag"
    );

    // A real (full) thumbnail arrives and wins: it marks the flag.
    defer_thumbnail_paint(
        tile.downgrade(),
        glib::WeakRef::new(),
        binding_state.clone(),
        binding.clone(),
        loaded_with_light(Some(true)),
        std::time::Instant::now(),
        false,
        false,
    );
    while glib::MainContext::default().pending() {
        glib::MainContext::default().iteration(false);
    }
    assert_eq!(tile.background_is_light(), Some(true));
    assert!(tile.full_thumbnail_painted());
}

/// An EXIF placeholder arriving AFTER a full thumbnail must be skipped, so the
/// low-res preview never overwrites a sharper one already on screen.
#[gtk::test]
fn exif_placeholder_is_not_painted_over_an_existing_full_thumbnail() {
    let _ = gtk::init();
    let tile = SquareTile::new();
    let binding = TileBinding::new(2, 3, MediaId::from(7), Some("cache".into()));
    let binding_state = Rc::new(RefCell::new(Some(binding.clone())));

    defer_thumbnail_paint(
        tile.downgrade(),
        glib::WeakRef::new(),
        binding_state.clone(),
        binding.clone(),
        loaded_with_light(Some(true)),
        std::time::Instant::now(),
        false,
        false,
    );
    while glib::MainContext::default().pending() {
        glib::MainContext::default().iteration(false);
    }
    assert!(tile.full_thumbnail_painted());
    assert_eq!(tile.background_is_light(), Some(true));

    // Late placeholder must be skipped — background stays the full-thumb value.
    defer_thumbnail_paint(
        tile.downgrade(),
        glib::WeakRef::new(),
        binding_state.clone(),
        binding.clone(),
        loaded_with_light(Some(false)),
        std::time::Instant::now(),
        true,
        true,
    );
    while glib::MainContext::default().pending() {
        glib::MainContext::default().iteration(false);
    }
    assert_eq!(
        tile.background_is_light(),
        Some(true),
        "a placeholder must not downgrade an already-painted full thumbnail"
    );
    assert!(tile.full_thumbnail_painted());
}
