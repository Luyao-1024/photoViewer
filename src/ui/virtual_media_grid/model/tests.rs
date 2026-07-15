use super::*;
use crate::core::section_model::SectionKey;
use chrono::{TimeZone, Utc};
use gtk4::glib;
use std::collections::{HashMap, HashSet};

fn item(id: i64) -> MediaItem {
    let stamp = Utc.with_ymd_and_hms(2026, 7, 13, 12, 0, 0).unwrap();
    MediaItem {
        id,
        uri: format!("file:///tmp/{id}.jpg"),
        path: format!("/tmp/{id}.jpg").into(),
        folder_path: "/tmp".into(),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(100),
        height: Some(100),
        video_duration_secs: None,
        taken_at: Some(stamp),
        file_mtime: stamp,
        file_size: 1,
        blake3_hash: format!("hash-{id}"),
        is_favorite: false,
        trashed_at: None,
    }
}

fn layout() -> VirtualGridLayoutIndex {
    let mut counts = HashMap::new();
    counts.insert(
        SectionKey {
            year: Some(2026),
            month: Some(7),
            day: Some(13),
        },
        3,
    );
    counts.insert(
        SectionKey {
            year: Some(2026),
            month: Some(7),
            day: Some(12),
        },
        1,
    );
    VirtualGridLayoutIndex::new(&counts, 2)
}

#[test]
fn model_reports_slot_count_and_placeholder_without_io() {
    let model = VirtualMediaModel::new(layout());

    assert_eq!(model.n_items(), 6);
    assert_eq!(
        model.slot_state(0),
        Some(GridSlotState::Placeholder {
            slot: 0,
            media_offset: 0,
        })
    );
    assert!(matches!(
        model.slot_state(3),
        Some(GridSlotState::Filler { .. })
    ));

    let object = model.item(0).unwrap();
    let boxed = object.downcast::<glib::BoxedAnyObject>().unwrap();
    assert!(matches!(
        *boxed.borrow::<GridSlotState>(),
        GridSlotState::Placeholder {
            media_offset: 0,
            ..
        }
    ));
}

#[test]
fn model_reuses_a_live_slot_object_until_that_slot_changes() {
    let model = VirtualMediaModel::new(layout());

    let first = model.item(0).expect("slot should have an object");
    let same = model.item(0).expect("slot should reuse its live object");
    assert_eq!(first, same);

    model.replace_ready_range(0..1, vec![item(10)]);
    let updated = model.item(0).expect("changed slot should have an object");
    assert_ne!(first, updated);
    let updated = updated
        .downcast::<glib::BoxedAnyObject>()
        .expect("virtual slot items use BoxedAnyObject");
    assert!(matches!(
        *updated.borrow::<GridSlotState>(),
        GridSlotState::Ready { ref item, .. } if item.id == 10
    ));
}

#[test]
fn replacing_ready_range_only_retains_returned_items() {
    let model = VirtualMediaModel::new(layout());
    model.replace_ready_range(0..3, vec![item(10), item(11), item(12)]);

    assert_eq!(model.ready_item_count(), 3);
    assert!(matches!(
        model.slot_state(1),
        Some(GridSlotState::Ready { ref item, .. }) if item.id == 11
    ));
    assert_eq!(
        model
            .ready_item_for_media_id(MediaId::from(12))
            .map(|item| item.id),
        Some(12)
    );

    model.replace_ready_range(1..3, vec![item(21)]);
    assert_eq!(model.ready_item_count(), 2);
    assert!(matches!(
        model.slot_state(1),
        Some(GridSlotState::Ready { ref item, .. }) if item.id == 21
    ));
    assert!(matches!(
        model.slot_state(2),
        Some(GridSlotState::Placeholder {
            media_offset: 2,
            ..
        })
    ));
}

#[test]
fn eviction_turns_distant_items_back_into_placeholders() {
    let model = VirtualMediaModel::new(layout());
    model.replace_ready_range(0..4, vec![item(10), item(11), item(12), item(13)]);
    model.evict_outside(1..3);

    assert_eq!(model.ready_item_count(), 2);
    assert!(matches!(
        model.slot_state(0),
        Some(GridSlotState::Placeholder {
            media_offset: 0,
            ..
        })
    ));
    assert!(matches!(
        model.slot_state(1),
        Some(GridSlotState::Ready { ref item, .. }) if item.id == 11
    ));
}

#[test]
fn layout_replacement_clears_ready_items_and_changes_generation() {
    let model = VirtualMediaModel::new(layout());
    model.replace_ready_range(0..1, vec![item(1)]);
    model.replace_layout(layout(), 7);

    assert_eq!(model.layout_generation(), 7);
    assert_eq!(model.ready_item_count(), 0);
    assert!(matches!(
        model.slot_state(0),
        Some(GridSlotState::Placeholder { .. })
    ));
}

#[test]
fn layout_replacement_can_publish_its_initial_ready_range_atomically() {
    let model = VirtualMediaModel::new(layout());

    model.replace_layout_with_ready_range(layout(), 7, 0..2, vec![item(10), item(11)]);

    assert_eq!(model.layout_generation(), 7);
    assert_eq!(model.ready_item_count(), 2);
    assert!(matches!(
        model.slot_state(0),
        Some(GridSlotState::Ready { ref item, .. }) if item.id == 10
    ));
    assert!(matches!(
        model.slot_state(1),
        Some(GridSlotState::Ready { ref item, .. }) if item.id == 11
    ));
}

#[test]
fn layout_reflow_preserves_ready_media_when_columns_change() {
    let model = VirtualMediaModel::new(layout());
    model.replace_ready_range(0..2, vec![item(10), item(11)]);

    let previous_generation = model.layout_generation();
    model.reflow_layout(layout(), previous_generation + 1);

    assert_eq!(model.layout_generation(), previous_generation + 1);
    assert_eq!(model.ready_item_count(), 2);
    assert!(matches!(
        model.slot_state(0),
        Some(GridSlotState::Ready { ref item, .. }) if item.id == 10
    ));
}

#[test]
fn favorite_update_keeps_resident_slot_identity_and_emits_no_model_change() {
    let model = VirtualMediaModel::new(layout());
    model.replace_ready_range(0..2, vec![item(10), item(11)]);
    let first = model.item(0).expect("ready slot should have an object");

    let changes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let captured_changes = changes.clone();
    model.connect_items_changed(move |_, position, removed, added| {
        captured_changes
            .borrow_mut()
            .push((position, removed, added));
    });

    model.update_ready_favorite_flags(&HashSet::from([MediaId::from(10)]), true);

    assert!(matches!(
        model.slot_state(0),
        Some(GridSlotState::Ready { ref item, .. }) if item.is_favorite
    ));
    assert_eq!(
        model.item(0).expect("slot identity should remain stable"),
        first,
        "favorite-only updates must not replace a GridView list item"
    );
    let first = first
        .downcast::<glib::BoxedAnyObject>()
        .expect("virtual slot items use BoxedAnyObject");
    assert!(matches!(
        *first.borrow::<GridSlotState>(),
        GridSlotState::Ready { ref item, .. } if item.is_favorite
    ));
    assert!(
        changes.borrow().is_empty(),
        "favorite-only updates must not trigger GtkGridView rebinding"
    );
}
