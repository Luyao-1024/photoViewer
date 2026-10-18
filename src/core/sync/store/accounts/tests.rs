use super::*;
#[test]
fn single_account_selection_is_persistent_and_keeps_saved_history() {
    let root = tempfile::tempdir().unwrap();
    let photos = root.path().join("photos");
    let archive = root.path().join("archive");
    std::fs::create_dir(&photos).unwrap();
    std::fs::create_dir(&archive).unwrap();
    let pool = crate::core::db::init_pool(&root.path().join("library.db")).unwrap();
    let store = SyncStore::new(pool.clone());
    let a = store
        .create_job(&super::super::tests::new_job(photos.clone()))
        .unwrap();
    let mut other = super::super::tests::new_job(photos);
    other.credential_ref = "second-account".into();
    other.native_pinned.as_mut().unwrap().namespace.tenant = "other-account".into();
    let b = store.create_job(&other).unwrap();
    assert_eq!(store.list_jobs().unwrap(), vec![a.clone()]);
    let mut same = super::super::tests::new_job(archive);
    same.credential_ref = "same-account-archive".into();
    same.native_zone = Some(super::super::super::native::NativeZone::new("ArchiveSync"));
    same.native_pinned = Some(super::super::super::native::demo_pinned_scope(
        "https://sync.example.invalid/",
        &["PrimarySync", "ArchiveSync"],
    ));
    let c = store.create_job(&same).unwrap();
    assert_eq!(store.list_jobs().unwrap().len(), 2);
    let epoch = runtime::Epoch {
        job: a.id,
        config: a.config_generation,
        cycle: "old-account-cycle".into(),
    };
    store
        .runtime(runtime::Command::Begin(epoch.clone()))
        .unwrap();
    assert!(store.epoch_current(&epoch).unwrap());
    store
        .runtime(runtime::Command::SelectAccount { job: Some(b.id) })
        .unwrap();
    assert!(!store.epoch_current(&epoch).unwrap());
    assert_eq!(store.saved_jobs().unwrap().len(), 3);
    assert_eq!(store.list_jobs().unwrap(), vec![b.clone()]);
    let reopened = SyncStore::new(pool);
    reopened.normalize_accounts().unwrap();
    assert!(reopened.job_is_selected(b.id).unwrap());
    // Removing the current account never silently starts an older one.
    reopened.set_job_paused(b.id, true).unwrap();
    reopened.delete_job(b.id).unwrap();
    reopened.normalize_accounts().unwrap();
    assert!(reopened.list_jobs().unwrap().is_empty());
    reopened
        .runtime(runtime::Command::SelectAccount { job: Some(a.id) })
        .unwrap();
    assert!(reopened.job_is_selected(a.id).unwrap());
    assert!(reopened.job_is_selected(c.id).unwrap());
    assert_eq!(
        reopened.get_job(a.id).unwrap().unwrap().config_generation,
        a.config_generation
    );
}

#[test]
fn account_selection_survives_removing_the_first_connection_of_the_same_account() {
    let root = crate::testing::Sandbox::install().scratch("stable-account-reference");
    let pool = crate::core::db::init_pool(&root.path().join("photos.db")).unwrap();
    let store = SyncStore::new(pool.clone());
    let first_root = root.path().join("first");
    let second_root = root.path().join("second");
    std::fs::create_dir_all(&first_root).unwrap();
    std::fs::create_dir_all(&second_root).unwrap();
    let first = store
        .create_job(&super::super::tests::new_job(first_root))
        .unwrap();
    let mut second_config = super::super::tests::new_job(second_root);
    second_config.credential_ref = "credential-second-connection".into();
    second_config.native_zone = Some(NativeZone::new("SecondSync"));
    let mut scope = second_config.native_pinned.clone().unwrap();
    scope.zones.push("SecondSync".into());
    second_config.native_pinned = Some(scope);
    let second = store.create_job(&second_config).unwrap();
    assert_ne!(first.connection_id, second.connection_id);
    let selected: String = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT value FROM settings WHERE key='native_sync_account'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    pool.get()
        .unwrap()
        .execute("DELETE FROM sync_jobs WHERE id=?1", [first.id])
        .unwrap();
    pool.get()
        .unwrap()
        .execute(
            "DELETE FROM sync_connections WHERE id=?1",
            [first.connection_id],
        )
        .unwrap();
    store.normalize_accounts().unwrap();
    assert!(store.job_is_selected(second.id).unwrap());
    let after: String = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT value FROM settings WHERE key='native_sync_account'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(after, selected);
}
