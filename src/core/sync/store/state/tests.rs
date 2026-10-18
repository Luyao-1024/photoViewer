use super::*;
#[test]
fn scalar_scope_configuration_and_unknown_bootstrap_types_round_trip() {
    let root = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&root.path().join("photos.db")).unwrap();
    let conn = pool.get().unwrap();
    conn.execute_batch("INSERT INTO sync_connections(id,endpoint,credential_ref) VALUES(1,'https://sync.example.invalid/','ref'); INSERT INTO sync_jobs(id,connection_id,local_root,native_zone_name) VALUES(1,1,'/photos','One');").unwrap();
    let mut scope = crate::core::sync::native::demo_pinned_scope(
        "https://sync.example.invalid/",
        &["One", "Two"],
    );
    let user_id = uuid::Uuid::new_v4().to_string();
    scope.authentication = Some(crate::core::sync::native::NativeAuthentication {
        mode: "session_bearer".into(),
        user: Some(crate::core::sync::native::NativeUser {
            id: user_id.clone(),
            username: "测试用户".into(),
        }),
        resource_bearer_required: true,
    });
    write_scope(&conn, 1, &scope).unwrap();
    assert_eq!(read_scope(&conn, 1).unwrap(), Some(scope.clone()));
    assert_eq!(
        conn.query_row(
            "SELECT native_user_id FROM sync_connections WHERE id=1",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        user_id
    );
    let store = super::super::SyncStore::new(pool.clone());
    store
        .begin_native_receive(
            1,
            scope.namespace.clone(),
            &scope.database_generation,
            "scope",
            crate::core::sync::native::NativeZone::new("One"),
        )
        .unwrap();
    let state = BootstrapState {
        started: true,
        types: std::collections::BTreeMap::from([(
            "future-record".into(),
            TypeMarker {
                status: TypeStatus::Pending,
                continuation_marker: Some("opaque-cursor".into()),
                record_count: 100,
            },
        )]),
    };
    write_bootstrap(&conn, 1, &state).unwrap();
    assert_eq!(read_bootstrap(&conn, 1, true).unwrap(), state);
    for albums in [
        None,
        Some(Vec::new()),
        Some(vec!["相册 A".into(), "B".into()]),
    ] {
        let pending = super::super::PendingChange {
            revision: 7,
            upload_albums: albums,
            direction: Some(crate::core::sync::SyncDirection::Bidirectional),
            delete: false,
        };
        write_change(&conn, 1, &pending).unwrap();
        assert_eq!(read_change(&conn, 1).unwrap(), Some(pending));
    }
    let legacy = serde_json::json!({"Ok":[scope.public_base_url,scope.namespace.tenant,user_id]});
    conn.execute(
        "INSERT INTO settings(key,value) VALUES('native_sync_account',?1)",
        [legacy.to_string()],
    )
    .unwrap();
    migrate_account_selection(&conn).unwrap();
    let selected: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key='native_sync_account'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let account: i64 = conn
        .query_row(
            "SELECT native_account_id FROM sync_connections WHERE id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(selected, account.to_string());
}
