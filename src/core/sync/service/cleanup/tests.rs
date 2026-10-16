use super::*;
#[test]
fn failed_credential_cleanup_preserves_account_references_for_retry() {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("photos.db")).unwrap();
    pool.get().unwrap().execute("INSERT INTO sync_connections(endpoint,credential_ref) VALUES('https://sync.invalid/','login-reference')", []).unwrap();
    let result = delete_credentials(&pool, |_| Err(AppError::Backend("locked keyring".into())));
    assert!(result.is_err());
    assert_eq!(
        pool.get()
            .unwrap()
            .query_row("SELECT credential_ref FROM sync_connections", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        "login-reference"
    );
}
