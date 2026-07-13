use super::*;
use std::collections::HashMap;

fn key(year: i32, month: u32, day: u32) -> SectionKey {
    SectionKey {
        year: Some(year),
        month: Some(month),
        day: Some(day),
    }
}

fn unknown_key() -> SectionKey {
    SectionKey {
        year: None,
        month: None,
        day: None,
    }
}

fn counts(entries: Vec<(SectionKey, u32)>) -> HashMap<SectionKey, u32> {
    entries.into_iter().collect()
}

#[test]
fn sections_are_sorted_newest_first_regardless_of_map_order() {
    let newest = key(2026, 7, 13);
    let middle = key(2026, 7, 12);
    let oldest = key(2025, 12, 31);
    let unknown = unknown_key();
    let index = VirtualGridLayoutIndex::new(
        &counts(vec![
            (oldest.clone(), 3),
            (unknown.clone(), 4),
            (middle.clone(), 1),
            (newest.clone(), 2),
        ]),
        4,
    );

    assert_eq!(
        index
            .sections()
            .iter()
            .map(|span| span.key.clone())
            .collect::<Vec<_>>(),
        vec![newest, middle, oldest, unknown]
    );
    assert_eq!(
        index
            .sections()
            .iter()
            .map(|span| span.media_start)
            .collect::<Vec<_>>(),
        vec![0, 2, 3, 6]
    );
    assert_eq!(index.media_count(), 10);
}

#[test]
fn section_spans_pad_each_section_to_a_complete_row() {
    let newest = key(2026, 7, 13);
    let oldest = key(2026, 7, 12);
    let index =
        VirtualGridLayoutIndex::new(&counts(vec![(newest.clone(), 5), (oldest.clone(), 3)]), 4);

    assert_eq!(
        index.sections(),
        &[
            SectionSpan {
                key: newest,
                media_start: 0,
                media_len: 5,
                slot_start: 0,
                slot_len: 8,
            },
            SectionSpan {
                key: oldest,
                media_start: 5,
                media_len: 3,
                slot_start: 8,
                slot_len: 4,
            },
        ]
    );
    assert_eq!(index.sections()[0].filler_len(), 3);
    assert_eq!(index.sections()[1].filler_len(), 1);
    assert_eq!(index.slot_count(), 12);
}

#[test]
fn slot_boundaries_distinguish_media_and_filler() {
    let newest = key(2026, 7, 13);
    let oldest = key(2026, 7, 12);
    let index =
        VirtualGridLayoutIndex::new(&counts(vec![(oldest.clone(), 3), (newest.clone(), 5)]), 4);

    assert_eq!(index.slot_at(0), Some(GridSlot::MediaOffset(0)));
    assert_eq!(index.slot_at(4), Some(GridSlot::MediaOffset(4)));
    assert_eq!(index.slot_at(5), Some(GridSlot::Filler { section: 0 }));
    assert_eq!(index.slot_at(7), Some(GridSlot::Filler { section: 0 }));
    assert_eq!(index.slot_at(8), Some(GridSlot::MediaOffset(5)));
    assert_eq!(index.slot_at(10), Some(GridSlot::MediaOffset(7)));
    assert_eq!(index.slot_at(11), Some(GridSlot::Filler { section: 1 }));
    assert_eq!(index.slot_at(12), None);

    assert_eq!(index.section_for_slot(7), Some(&newest));
    assert_eq!(index.section_for_slot(8), Some(&oldest));
    assert_eq!(index.section_for_slot(12), None);
}

#[test]
fn empty_and_zero_count_libraries_have_no_slots() {
    let empty = VirtualGridLayoutIndex::new(&HashMap::new(), 0);
    assert_eq!(empty.columns(), 1);
    assert_eq!(empty.section_count(), 0);
    assert_eq!(empty.media_count(), 0);
    assert_eq!(empty.slot_count(), 0);
    assert_eq!(empty.slot_at(0), None);
    assert_eq!(empty.slot_for_media_offset(0), None);
    assert_eq!(empty.anchor_media_offset_for_slot(0), None);
    assert_eq!(empty.section_for_slot(0), None);

    let only_zero = VirtualGridLayoutIndex::new(&counts(vec![(key(2026, 7, 13), 0)]), 3);
    assert_eq!(only_zero.section_count(), 0);
    assert_eq!(only_zero.slot_count(), 0);
}

#[test]
fn media_offsets_and_media_slots_are_exact_inverses() {
    let index = VirtualGridLayoutIndex::new(
        &counts(vec![
            (key(2026, 7, 13), 1),
            (key(2026, 7, 12), 4),
            (key(2026, 7, 11), 5),
        ]),
        3,
    );

    for offset in 0..index.media_count() {
        let slot = index
            .slot_for_media_offset(offset)
            .expect("every media offset has a slot");
        assert_eq!(index.slot_at(slot), Some(GridSlot::MediaOffset(offset)));
        assert_eq!(index.media_offset_at_slot(slot), Some(offset));
        assert_eq!(index.anchor_media_offset_for_slot(slot), Some(offset));
    }

    for slot in 0..index.slot_count() {
        let span = index
            .span_for_slot(slot)
            .expect("every slot is owned by a section");
        match index.slot_at(slot) {
            Some(GridSlot::MediaOffset(offset)) => {
                assert_eq!(index.anchor_media_offset_for_slot(slot), Some(offset));
            }
            Some(GridSlot::Filler { section }) => {
                assert_eq!(index.section_span(section), Some(span));
                assert_eq!(index.media_offset_at_slot(slot), None);
                assert_eq!(
                    index.anchor_media_offset_for_slot(slot),
                    span.media_end().checked_sub(1)
                );
            }
            None => panic!("slot {slot} should be represented"),
        }
    }
}

#[test]
fn changing_columns_reflows_slots_without_reordering_media() {
    let section_counts = counts(vec![(key(2026, 7, 13), 5), (key(2025, 12, 31), 4)]);
    let one_column = VirtualGridLayoutIndex::new(&section_counts, 1);
    let two_columns = VirtualGridLayoutIndex::new(&section_counts, 2);
    let four_columns = VirtualGridLayoutIndex::new(&section_counts, 4);

    assert_eq!(one_column.slot_count(), 9);
    assert_eq!(two_columns.slot_count(), 10);
    assert_eq!(four_columns.slot_count(), 12);
    assert_eq!(one_column.slot_for_media_offset(5), Some(5));
    assert_eq!(two_columns.slot_for_media_offset(5), Some(6));
    assert_eq!(four_columns.slot_for_media_offset(5), Some(8));

    for offset in 0..one_column.media_count() {
        assert_eq!(
            one_column.section_for_media_offset(offset),
            two_columns.section_for_media_offset(offset)
        );
        assert_eq!(
            two_columns.section_for_media_offset(offset),
            four_columns.section_for_media_offset(offset)
        );
    }
}

#[test]
fn filler_slots_project_to_the_last_media_for_date_and_anchor_helpers() {
    let index = VirtualGridLayoutIndex::new(
        &counts(vec![(key(2026, 7, 13), 5), (key(2026, 7, 12), 3)]),
        4,
    );

    assert_eq!(index.anchor_media_offset_for_slot(5), Some(4));
    assert_eq!(index.anchor_media_offset_for_slot(7), Some(4));
    assert_eq!(index.anchor_media_offset_for_slot(11), Some(7));
    assert_eq!(index.anchor_media_offset_for_slot(12), None);
    assert_eq!(index.slot_for_media_offset_clamped(7), Some(10));
    assert_eq!(index.slot_for_media_offset_clamped(99), Some(10));
}

#[test]
fn very_large_column_count_is_safe_without_allocating_filler_slots() {
    let index = VirtualGridLayoutIndex::new(&counts(vec![(key(2026, 7, 13), 2)]), u32::MAX);

    assert_eq!(index.columns(), u32::MAX);
    assert_eq!(index.slot_count(), u32::MAX);
    assert_eq!(index.sections()[0].slot_len, u32::MAX);
    assert_eq!(index.slot_at(0), Some(GridSlot::MediaOffset(0)));
    assert_eq!(index.slot_at(1), Some(GridSlot::MediaOffset(1)));
    assert_eq!(
        index.slot_at(u32::MAX - 1),
        Some(GridSlot::Filler { section: 0 })
    );
    assert_eq!(index.anchor_media_offset_for_slot(u32::MAX - 1), Some(1));
    assert_eq!(index.slot_at(u32::MAX), None);
}
