use super::*;
use crate::core::sync::store::dirty::ChangeCategory;

fn batch(entry_id: i64, version: i64) -> MutationBatch {
    MutationBatch {
        entry_id,
        relative_path: "camera/one.jpg".into(),
        category: ChangeCategory::Metadata,
        captured_version: version,
        operations: vec![MutationOperation::update_metadata(
            "photo-1",
            "rev-1".into(),
            MetadataPatch {
                favorite: Some(true),
            },
        )],
    }
}

#[test]
fn a_metadata_update_carries_the_current_revision_and_a_typed_field() {
    let operation = MutationOperation::update_metadata(
        "photo-1",
        "rev-1".into(),
        MetadataPatch {
            favorite: Some(true),
        },
    );
    let wire = MutationOperation::wire(&operation);
    assert_eq!(wire["operationType"], "update");
    assert_eq!(wire["record"]["recordName"], "photo-1");
    // The current tag is what turns "apply my change" into a conditional
    // update; without it the service cannot tell the two apart.
    assert_eq!(wire["record"]["recordChangeTag"], "rev-1");
    assert_eq!(
        wire["record"]["fields"]["isFavorite"],
        serde_json::json!({"type": "INT64", "value": 1})
    );
    // No `_syncserver` block: the service derives its own event, and one that
    // disagrees with the fields is rejected.
    assert!(wire["record"].get("_syncserver").is_none());
}

#[test]
fn a_patch_with_nothing_to_send_sends_no_fields() {
    let operation =
        MutationOperation::update_metadata("photo-1", "rev-1".into(), MetadataPatch::default());
    let wire = MutationOperation::wire(&operation);
    let fields = wire["record"]["fields"].as_object().unwrap();
    assert!(fields.is_empty());
}

#[test]
fn the_operation_id_follows_the_intent_not_the_request_bytes() {
    // A retry of the same obligation, possibly with a refreshed body, is the
    // same logical operation. Deriving the id from the body would report an
    // idempotency conflict for a retry.
    let first = batch(7, 3);
    let mut retried = first.clone();
    retried.operations = vec![MutationOperation::update_metadata(
        "photo-1",
        "rev-2".into(),
        MetadataPatch {
            favorite: Some(false),
        },
    )];
    assert_eq!(first.operation_id(), retried.operation_id());

    // A newer obligation is a different operation.
    assert_ne!(first.operation_id(), batch(7, 4).operation_id());
    // And so is another entry's.
    assert_ne!(first.operation_id(), batch(8, 3).operation_id());
    // The key stays inside the service's accepted shape: printable ASCII only.
    let key = first.operation_id();
    assert!(key.len() <= 128, "{key}");
    assert!(key.bytes().all(|byte| (33..=126).contains(&byte)), "{key}");
}

#[test]
fn an_operation_id_names_its_category() {
    for (category, name) in [
        (ChangeCategory::Metadata, "metadata"),
        (ChangeCategory::Content, "content"),
        (ChangeCategory::Deletion, "deletion"),
    ] {
        let mut item = batch(1, 1);
        item.category = category;
        assert!(
            item.operation_id().contains(name),
            "{} should mention {name}",
            item.operation_id()
        );
    }
}

#[test]
fn a_confirmed_record_is_read_from_the_services_own_answer() {
    let confirmed = read_confirmed(&serde_json::json!({
        "recordName": "photo-1",
        "recordType": "CPLAsset",
        "recordChangeTag": "rev-9",
        "fields": {
            "masterRef": {"type": "REFERENCE", "value": {"recordName": "master-1", "action": "NONE"}},
            "contentGeneration": {"type": "STRING", "value": "gen-1"},
        },
    }))
    .unwrap();
    assert_eq!(confirmed.record_name, "photo-1");
    assert_eq!(confirmed.record_change_tag.as_deref(), Some("rev-9"));
    assert_eq!(confirmed.master_id.as_deref(), Some("master-1"));
    assert_eq!(confirmed.content_generation.as_deref(), Some("gen-1"));
}

#[test]
fn an_in_band_failure_is_read_before_the_records_are_trusted() {
    // HTTP 200 with a failure in the body is the case that a status-code-only
    // client mistakes for success.
    let conflict = serde_json::json!({
        "serverErrorCode": "CONFLICT",
        "reason": "recordChangeTag does not match current revision",
        "recordName": "photo-1",
    });
    let reason = inline_error(&conflict).expect("an in-band failure");
    assert!(reason.contains("CONFLICT"));
    assert!(reason.contains("does not match"));

    // A successful record carries no code at all.
    assert!(inline_error(&serde_json::json!({
        "recordName": "photo-1",
        "recordChangeTag": "rev-9",
    }))
    .is_none());
}

#[test]
fn a_batch_that_confirmed_nothing_is_a_failure_not_a_success() {
    // `{"records": []}` must not be read as "accepted": there is nothing to
    // have accepted.
    assert_eq!(inline_error(&serde_json::json!({"records": []})), None);
}
