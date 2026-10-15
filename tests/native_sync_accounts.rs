//! Actual client login + actor + dual-resource synchronization across two users.
mod common;
use photo_viewer::core::{
    db,
    db_actor::{start_db_actor, DbCommand},
    events::{ChangeSource, DomainEventSender},
    sync::{SyncDirection, SyncService, UploadScope},
};
use std::{
    io::Write,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct Server {
    child: Child,
    endpoint: String,
    _root: tempfile::TempDir,
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Server {
    async fn start() -> Self {
        Self::start_mode(false).await
    }
    async fn start_mode(legacy: bool) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        std::fs::create_dir(&data).unwrap();
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
        let signing = root.path().join("signing.key");
        std::fs::write(&signing, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
        std::fs::set_permissions(&signing, std::fs::Permissions::from_mode(0o600)).unwrap();
        let binary = std::env::var_os("SYNC_SERVER_BIN")
            .unwrap_or_else(|| "../syncserver/target/debug/syncserver".into());
        let api = root.path().join("api.key");
        std::fs::write(&api, "synthetic-legacy-key-0000000000000000").unwrap();
        std::fs::set_permissions(&api, std::fs::Permissions::from_mode(0o600)).unwrap();
        for name in if legacy { vec![] } else { vec!["alice", "bob"] } {
            let mut child = Command::new(&binary)
                .args(["users", "create", name, "--password-stdin"])
                .env("SYNC_DATA_DIR", &data)
                .env("SYNC_SIGNING_KEY_FILE", &signing)
                .env_remove("SYNC_API_KEY_FILE")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            writeln!(child.stdin.take().unwrap(), "synthetic account password").unwrap();
            assert!(child.wait().unwrap().success());
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let endpoint = format!("http://{address}/");
        let mut command = Command::new(binary);
        if legacy {
            command.env("SYNC_API_KEY_FILE", &api);
        } else {
            command.env_remove("SYNC_API_KEY_FILE");
        }
        let child = command
            .env("SYNC_DATA_DIR", &data)
            .env("SYNC_SIGNING_KEY_FILE", &signing)
            .env("SYNC_BIND", address.to_string())
            .env("SYNC_PUBLIC_BASE_URL", &endpoint)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let server = Self {
            child,
            endpoint,
            _root: root,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if reqwest::get(format!("{}readyz", server.endpoint))
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                break;
            }
            assert!(Instant::now() < deadline, "server start");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        server
    }
}
#[tokio::test(flavor = "multi_thread")]
async fn two_accounts_share_an_endpoint_without_sharing_data_or_local_progress() {
    let sandbox = photo_viewer::testing::Sandbox::install();
    common::credentials::install();
    let server = Server::start().await;
    let root = sandbox.scratch("native-accounts");
    let left = root.path().join("alice");
    let right = root.path().join("bob");
    let received = root.path().join("receiver-alice");
    for path in [&left, &right, &received] {
        std::fs::create_dir_all(path).unwrap();
    }
    let alice_photo = common::write_plain_jpeg(&left, "photo.jpg");
    let bob_photo = right.join("photo.jpg");
    common::write_distinct_jpeg(&bob_photo, 64, 48, [210, 40, 60]);
    let pool = db::init_pool(&root.path().join("client.sqlite")).unwrap();
    let (events, mut receiver) = DomainEventSender::new();
    tokio::spawn(async move { while receiver.recv().await.is_some() {} });
    let actor = start_db_actor(pool.clone(), events);
    let service = SyncService::with_actor(pool.clone(), actor.clone());
    let (alice, scope_a) = service
        .login_native_connection(&server.endpoint, "alice", "synthetic account password")
        .await
        .unwrap();
    let (bob, scope_b) = service
        .login_native_connection(&server.endpoint, "bob", "synthetic account password")
        .await
        .unwrap();
    assert_ne!(scope_a.namespace, scope_b.namespace);
    for path in [&alice_photo, &bob_photo] {
        let item = photo_viewer::core::backend::local::LocalBackend::new_item_from_path(path)
            .unwrap()
            .unwrap();
        actor
            .execute_blocking(DbCommand::UpsertMediaBatch {
                source: ChangeSource::UserInteractive,
                items: vec![item],
            })
            .unwrap();
    }
    let a = service
        .create_native_job_scoped(
            &server.endpoint,
            "test-alice-session",
            &alice.access_token,
            &left,
            "PrimarySync",
            SyncDirection::Bidirectional,
            UploadScope::All,
            vec![],
        )
        .await
        .unwrap();
    let b = service
        .create_native_job_scoped(
            &server.endpoint,
            "test-bob-session",
            &bob.access_token,
            &right,
            "PrimarySync",
            SyncDirection::Bidirectional,
            UploadScope::All,
            vec![],
        )
        .await
        .unwrap();
    assert_ne!(a.connection_id, b.connection_id);
    for job in [&a, &b] {
        let report = service.run_cycle(job.id).await.unwrap();
        assert!(report.failures.is_empty(), "{report:?}");
        assert_eq!(report.outstanding, 0);
        assert_eq!(report.mutations_confirmed, 1);
    }
    let target_pool = db::init_pool(&root.path().join("receiver.sqlite")).unwrap();
    let (events, mut receiver) = DomainEventSender::new();
    tokio::spawn(async move { while receiver.recv().await.is_some() {} });
    let target_actor = start_db_actor(target_pool.clone(), events);
    let target = SyncService::with_actor(target_pool.clone(), target_actor);
    let (second_session, second_scope) = target
        .login_native_connection(&server.endpoint, "alice", "synthetic account password")
        .await
        .unwrap();
    assert_eq!(scope_a.namespace, second_scope.namespace);
    assert_ne!(alice.access_token, second_session.access_token);
    let duplicate = root.path().join("duplicate-alice");
    std::fs::create_dir(&duplicate).unwrap();
    assert!(service
        .create_native_job_scoped(
            &server.endpoint,
            "duplicate-alice-reference",
            &second_session.access_token,
            &duplicate,
            "PrimarySync",
            SyncDirection::Bidirectional,
            UploadScope::All,
            vec![]
        )
        .await
        .is_err());
    let job = target
        .create_native_job_scoped(
            &server.endpoint,
            "test-alice-receiver",
            &second_session.access_token,
            &received,
            "PrimarySync",
            SyncDirection::Bidirectional,
            UploadScope::SelectedAlbums,
            vec![],
        )
        .await
        .unwrap();
    let report = target.run_cycle(job.id).await.unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(report.outstanding, 0);
    assert_eq!(
        std::fs::read(received.join("photo.jpg")).unwrap(),
        std::fs::read(&alice_photo).unwrap()
    );
    assert_ne!(
        std::fs::read(received.join("photo.jpg")).unwrap(),
        std::fs::read(&bob_photo).unwrap()
    );
    assert_eq!(db::list_all_media(&target_pool).unwrap().len(), 1);
    let before = service.store().native_progress(a.id).unwrap();
    // Inject a wrong account at the credential boundary; no foreign feed may be ingested.
    photo_viewer::platform::credentials::store_api_key(&a.credential_ref, &bob.access_token)
        .unwrap();
    assert!(service
        .run_cycle(a.id)
        .await
        .unwrap_err()
        .to_string()
        .contains("identity changed"));
    assert_eq!(service.store().native_progress(a.id).unwrap(), before);
    assert_eq!(service.run_cycle(b.id).await.unwrap().outstanding, 0);
    assert!(service
        .reauthenticate_native_job(a.id, "bob", "synthetic account password")
        .await
        .is_err());
    service
        .reauthenticate_native_job(a.id, "alice", "synthetic account password")
        .await
        .unwrap();
    let report = service.run_cycle(a.id).await.unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(report.outstanding, 0);
    assert_eq!(report.mutations_sent, 0);
    // A session revoked elsewhere can still be signed out locally.
    let revoked = photo_viewer::platform::credentials::load_api_key(&a.credential_ref).unwrap();
    let endpoint = photo_viewer::core::sync::NativeEndpoint::parse(&server.endpoint).unwrap();
    photo_viewer::core::sync::NativeClient::new(&endpoint, &revoked)
        .unwrap()
        .logout()
        .await
        .unwrap();
    service.logout_native_job(a.id).await.unwrap();
    assert!(service.store().get_job(a.id).unwrap().unwrap().paused);
    assert!(photo_viewer::platform::credentials::load_api_key(&a.credential_ref).is_err());
    assert_eq!(service.run_cycle(b.id).await.unwrap().outstanding, 0);
    assert_eq!(target.run_cycle(job.id).await.unwrap().outstanding, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn explicit_legacy_owner_login_preserves_the_existing_photo_and_progress() {
    let sandbox = photo_viewer::testing::Sandbox::install();
    common::credentials::install();
    let server = Server::start_mode(true).await;
    let root = sandbox.scratch("native-legacy-adoption");
    let library = root.path().join("library");
    std::fs::create_dir(&library).unwrap();
    let source = common::write_plain_jpeg(&library, "existing.jpg");
    let pool = db::init_pool(&root.path().join("client.sqlite")).unwrap();
    let (sender, mut events) = DomainEventSender::new();
    tokio::spawn(async move { while events.recv().await.is_some() {} });
    let actor = start_db_actor(pool.clone(), sender);
    let service = SyncService::with_actor(pool.clone(), actor);
    let job = service
        .create_native_job_scoped(
            &server.endpoint,
            "legacy-owner-reference",
            "synthetic-legacy-key-0000000000000000",
            &library,
            "PrimarySync",
            SyncDirection::Bidirectional,
            UploadScope::All,
            vec![],
        )
        .await
        .unwrap();
    let created = service.run_cycle(job.id).await.unwrap();
    assert!(created.failures.is_empty(), "{created:?}");
    assert_eq!(created.mutations_confirmed, 1);
    let settled = service.run_cycle(job.id).await.unwrap();
    assert_eq!(settled.outstanding, 0);
    let progress = service.store().native_progress(job.id).unwrap().unwrap();
    let source_bytes = std::fs::read(&source).unwrap();
    let binary = std::env::var_os("SYNC_SERVER_BIN")
        .unwrap_or_else(|| "../syncserver/target/debug/syncserver".into());
    let mut admin = Command::new(binary)
        .args([
            "users",
            "create",
            "owner",
            "--adopt-legacy",
            "--password-stdin",
        ])
        .env("SYNC_DATA_DIR", server._root.path().join("data"))
        .env(
            "SYNC_SIGNING_KEY_FILE",
            server._root.path().join("signing.key"),
        )
        .env("SYNC_API_KEY_FILE", server._root.path().join("api.key"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    writeln!(admin.stdin.take().unwrap(), "synthetic account password").unwrap();
    assert!(admin.wait().unwrap().success());
    assert!(service.run_cycle(job.id).await.is_err());
    assert_eq!(
        service.store().native_progress(job.id).unwrap().unwrap(),
        progress
    );
    service
        .reauthenticate_native_job(job.id, "owner", "synthetic account password")
        .await
        .unwrap();
    let scope = service
        .store()
        .native_scope(job.connection_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        scope.authentication.unwrap().user.unwrap().username,
        "owner"
    );
    assert_eq!(scope.database_generation, progress.database_generation);
    let cycle = service.run_cycle(job.id).await.unwrap();
    assert!(cycle.failures.is_empty(), "{cycle:?}");
    assert_eq!(cycle.outstanding, 0);
    assert_eq!(cycle.mutations_sent, 0);
    assert_eq!(std::fs::read(source).unwrap(), source_bytes);
    assert_eq!(db::list_all_media(&pool).unwrap().len(), 1);
}
