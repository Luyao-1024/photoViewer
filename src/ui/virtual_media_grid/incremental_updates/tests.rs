use super::*;

fn kinds(values: &[IncrementalItemKind]) -> Vec<IncrementalItemKind> {
    values.to_vec()
}

#[test]
fn an_upsert_is_admitted_when_the_count_delta_proves_it_is_new() {
    let resolved = resolve_inserted_kinds(6, 7, &kinds(&[IncrementalItemKind::Upsert]));
    assert_eq!(resolved, Some(vec![true]));
}

#[test]
fn an_existing_item_upsert_falls_back_instead_of_moving_a_resident_offset() {
    assert_eq!(
        resolve_inserted_kinds(6, 6, &kinds(&[IncrementalItemKind::Upsert])),
        None,
        "an unchanged count cannot prove that refresh_ready_item has the same rank"
    );
}

#[test]
fn mixed_batches_require_the_delta_to_cover_every_unknown_upsert() {
    assert_eq!(
        resolve_inserted_kinds(
            6,
            8,
            &kinds(&[IncrementalItemKind::Inserted, IncrementalItemKind::Upsert,])
        ),
        Some(vec![true, true])
    );
    assert_eq!(
        resolve_inserted_kinds(
            6,
            7,
            &kinds(&[IncrementalItemKind::Inserted, IncrementalItemKind::Upsert,])
        ),
        None,
        "one of the unknown items could be an update with a new rank"
    );
}

#[test]
fn updates_stay_accepted_only_when_the_total_is_unchanged() {
    assert_eq!(
        resolve_inserted_kinds(
            6,
            6,
            &kinds(&[IncrementalItemKind::Updated, IncrementalItemKind::Updated,])
        ),
        Some(vec![false, false])
    );
    assert_eq!(
        resolve_inserted_kinds(6, 7, &kinds(&[IncrementalItemKind::Updated])),
        None
    );
}

#[test]
fn a_shrinking_library_falls_back_to_a_full_reload() {
    assert_eq!(
        resolve_inserted_kinds(7, 6, &kinds(&[IncrementalItemKind::Upsert])),
        None
    );
}

#[test]
fn pending_kinds_keep_the_strongest_insertion_claim() {
    use IncrementalItemKind::{Inserted, Updated, Upsert};

    assert_eq!(Updated.merge(Upsert), Upsert);
    assert_eq!(Upsert.merge(Updated), Upsert);
    assert_eq!(Upsert.merge(Inserted), Inserted);
    assert_eq!(Inserted.merge(Updated), Inserted);
    assert_eq!(Updated.merge(Updated), Updated);
}
