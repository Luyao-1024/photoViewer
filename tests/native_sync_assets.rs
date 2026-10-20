//! C1.4 acceptance against the real SyncServer binary: a complete photo pair.
//!
//! This is the first stage that moves bytes. Everything it asserts is a
//! property the service enforces and a client can get wrong in a way unit
//! tests cannot catch: that a reservation wants a digest in one encoding and
//! returns another, that the upload URL is its own authority, that a role
//! swapped into the wrong master field is refused, and that the pair only binds
//! once both resources are durably published.
//!
//! The suite drives the real asset endpoints rather than a photo create,
//! because writing a full `CPLMaster` + `CPLAsset` pair needs the client's own
//! atomic binding (C1.4's own path, covered by its unit tests) and the point
//! here is that the *transfer* is real.

mod common;
#[path = "native_sync_assets/logging.rs"]
mod logging;

use logging::Capture;
#[path = "native_sync_assets/presence.rs"]
mod presence;
#[path = "native_sync_assets/upload_failures.rs"]
mod upload_failures;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use photo_viewer::core::sync::content::{snapshot_source, ContentDigest};
use photo_viewer::core::sync::native::assets::{
    AssetRole, ReservationRequest, TransferError, UploadIntent,
};
use photo_viewer::core::sync::native::lease::UploadPhase;
use photo_viewer::core::sync::{
    NativeClient, NativeEndpoint, NativePinnedScope, NativeZone, SyncError, UploadLease,
};
use serde_json::json;

// These integration tests share the Sandbox preference file. Serialize its
// read-modify-write changes so trash-backend setup cannot overwrite Sync On.
static PREFERENCES: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn server_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("SYNC_SERVER_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    for build in ["debug", "release"] {
        let path = Path::new("../syncserver/target")
            .join(build)
            .join("syncserver");
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

struct Server {
    child: Child,
    base_url: String,
    api_key: String,
    #[allow(dead_code)]
    root: tempfile::TempDir,
}

impl Server {
    fn start() -> Option<Self> {
        Self::start_with_lease(90, 30)
    }
    fn start_with_lease(ttl: u64, heartbeat: u64) -> Option<Self> {
        photo_viewer::testing::Sandbox::install();
        let binary = server_binary()?;
        let root = tempfile::tempdir().expect("temp dir");
        let api_key = secret_value();
        write_secret(&root, "api.key", api_key.clone());
        write_secret(&root, "signing.key", base64_32_bytes());
        let data = root.path().join("data");
        std::fs::create_dir(&data).expect("data dir");
        std::fs::set_permissions(&data, mode(0o700)).expect("data dir mode");
        let port = free_port();
        let base_url = format!("http://127.0.0.1:{port}/");

        let child = Command::new(&binary)
            .env("SYNC_DATA_DIR", &data)
            .env("SYNC_API_KEY_FILE", root.path().join("api.key"))
            .env("SYNC_SIGNING_KEY_FILE", root.path().join("signing.key"))
            .env("SYNC_BIND", format!("127.0.0.1:{port}"))
            .env("SYNC_PUBLIC_BASE_URL", &base_url)
            .env("SYNC_LEASE_TTL_SECS", ttl.to_string())
            .env("SYNC_LEASE_HEARTBEAT_SECS", heartbeat.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn syncserver");
        let mut server = Self {
            child,
            base_url,
            api_key,
            root,
        };
        server.wait_ready();
        Some(server)
    }

    fn wait_ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            let url = format!("{}readyz", self.base_url);
            let ready = block_on(async {
                matches!(
                    reqwest::Client::new().get(url).send().await,
                    Ok(response) if response.status().is_success()
                )
            });
            if ready {
                return;
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!("syncserver exited during startup: {status}");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("syncserver did not become ready within 30s");
    }

    fn client(&self) -> NativeClient {
        NativeClient::new(
            &NativeEndpoint::parse(&self.base_url).expect("loopback endpoint"),
            &self.api_key,
        )
        .expect("client")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A lease walked to `create`, which is the only phase a reservation accepts.
fn lease_at_create(client: &NativeClient, zone: &NativeZone) -> UploadLease {
    let (grant, started) =
        block_on(client.acquire_lease("photoviewer-test", zone)).expect("acquire the zone");
    let lease = UploadLease::from_grant(&grant, started);
    let token = block_on(client.snapshot(zone))
        .expect("snapshot")
        .sync_token;
    let check = block_on(client.check_lease(lease.proof(), zone, &token, true)).expect("check");
    assert!(check.may_upload, "the gate must open for a current client");
    for phase in UploadPhase::ORDER {
        block_on(client.set_phase(lease.proof(), phase)).expect("advance phase");
    }
    lease
}

/// A stable original and a rendered thumbnail for the asset endpoint fixtures.
fn photo_fixtures() -> (ContentDigest, Vec<u8>, Vec<u8>, ContentDigest) {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("original.jpg");
    let image = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 800, 600)
        .expect("build fixture");
    image.fill(0x3366CC);
    std::fs::write(&source, image.save_to_bufferv("jpeg", &[]).unwrap()).expect("write fixture");

    let snapshot = snapshot_source(
        &source,
        &dir.path().join("staging"),
        "original",
        64 * 1024 * 1024,
    )
    .expect("snapshot");
    let original_bytes = snapshot.read().expect("read snapshot");
    // Protocol fixtures explicitly render their thumbnail. Production uplink
    // instead reads a thumbnail already produced by the local cache pipeline.
    let (thumbnail_bytes, thumbnail_digest) =
        photo_viewer::core::sync::content::thumbnail_from(&snapshot, 2048, 2 * 1024 * 1024)
            .expect("thumbnail");
    (
        snapshot.digest,
        original_bytes,
        thumbnail_bytes,
        thumbnail_digest,
    )
}

fn skip() {
    eprintln!(
        "skipping: no syncserver binary. Build the sibling checkout with \
         `cargo build --locked --manifest-path ../syncserver/Cargo.toml` or set \
         SYNC_SERVER_BIN."
    );
}

fn mode(bits: u32) -> std::fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    std::fs::Permissions::from_mode(bits)
}

fn write_secret(root: &tempfile::TempDir, name: &str, value: String) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.path().join(name))
        .expect("create secret file");
    writeln!(file, "{value}").expect("write secret");
}

fn secret_value() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("pv-assets-key-{nanos:032x}")
}

fn base64_32_bytes() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let raw: Vec<u8> = (0..32u8).collect();
    let mut out = String::new();
    for chunk in raw.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(future)
}

/// A complete round trip: reserve both roles, upload both, read the receipts,
/// download the bytes back, and verify they are exactly what was sent.
#[test]
fn a_reserved_pair_uploads_and_downloads_back_byte_for_byte() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let client = server.client();
    let pinned: NativePinnedScope = block_on(client.probe()).expect("probe");
    let zone = NativeZone::new("PrimarySync");
    let (original_digest, original_bytes, thumbnail_bytes, thumbnail_digest) = photo_fixtures();

    let lease = lease_at_create(&client, &zone);
    let reservations = block_on(client.reserve_assets(
        &pinned.namespace,
        &zone,
        lease.proof(),
        "master-pair",
        UploadIntent::Create,
        &[
            ReservationRequest {
                role: AssetRole::Original,
                intent: UploadIntent::Create,
                digest: original_digest.clone(),
            },
            ReservationRequest {
                role: AssetRole::Thumbnail,
                intent: UploadIntent::Create,
                digest: thumbnail_digest.clone(),
            },
        ],
    ))
    .expect("reserve both resources");
    assert_eq!(reservations.len(), 2);

    for (reservation, bytes) in reservations.iter().zip([&original_bytes, &thumbnail_bytes]) {
        let descriptor = block_on(client.upload_asset_bytes(reservation, lease.proof(), bytes))
            .unwrap_or_else(|error| panic!("upload {}: {error}", reservation.role.as_str()));
        assert_eq!(descriptor.size, bytes.len() as u64);
        // The answer is the server's own descriptor of what it stored, in
        // Base64 — the same digest the reservation declared in hex.
        let sent = if reservation.role == AssetRole::Original {
            &original_digest
        } else {
            &thumbnail_digest
        };
        assert_eq!(
            ContentDigest::hex_from_descriptor_base64(&descriptor.file_checksum).as_deref(),
            Some(sent.hex.as_str()),
            "{} must hash back to the digest it was reserved with",
            reservation.role.as_str()
        );
    }

    // Every session reached the only state that carries an asset id.
    for reservation in &reservations {
        let session = block_on(client.upload_session(&reservation.session_id)).expect("session");
        assert!(session.state.is_published(), "{:?}", session.state);
        assert!(session.asset.is_some());
    }

    // Describe and download: the bytes come back exactly as sent.
    let described =
        block_on(client.describe_asset(&reservations[0].session_id, AssetRole::Original));
    // A session id is not an asset id; this is a miss, not a lesser truth.
    assert!(
        described.is_err(),
        "a session id must not describe an asset"
    );

    let asset_id = block_on(client.upload_session(&reservations[0].session_id))
        .expect("session")
        .asset
        .expect("descriptor")
        .asset_id;
    let descriptor = block_on(client.describe_asset(&asset_id, AssetRole::Original))
        .expect("describe the original");
    let downloaded = block_on(client.download_asset(&descriptor, AssetRole::Original, None))
        .expect("download the original");
    assert_eq!(
        downloaded.bytes, original_bytes,
        "the original must round-trip"
    );
    assert!(!downloaded.partial);
    assert!(
        downloaded
            .etag
            .as_deref()
            .is_some_and(|etag| etag.contains(&original_digest.hex)),
        "the ETag must be the digest we declared: {:?}",
        downloaded.etag
    );

    // The role is part of the request and of the capability it returns. Asking
    // for the other role either is a miss, or yields a capability bound to that
    // role — never the original's capability handed back under a different name.
    match block_on(client.describe_asset(&asset_id, AssetRole::Thumbnail)) {
        Err(_) => {}
        Ok(other) => assert!(
            other.download_url.contains("role=thumbnail"),
            "a thumbnail description must carry a thumbnail capability: {}",
            other.download_url
        ),
    }
}

/// The service refuses bytes that do not match the digest the reservation
/// declared — which is why the staged copy exists at all.
#[test]
fn bytes_that_do_not_match_the_declared_digest_are_refused() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let client = server.client();
    let pinned: NativePinnedScope = block_on(client.probe()).expect("probe");
    let zone = NativeZone::new("PrimarySync");
    let (original_digest, original_bytes, _, _) = photo_fixtures();

    let lease = lease_at_create(&client, &zone);
    let reservations = block_on(client.reserve_assets(
        &pinned.namespace,
        &zone,
        lease.proof(),
        "master-mismatch",
        UploadIntent::Create,
        &[ReservationRequest {
            role: AssetRole::Original,
            intent: UploadIntent::Create,
            digest: original_digest.clone(),
        }],
    ))
    .expect("reserve");

    // Send a different original under the first one's digest.
    let mut other = original_bytes.clone();
    other.extend_from_slice(b"tampered");
    let error = block_on(client.upload_asset_bytes(&reservations[0], lease.proof(), &other))
        .expect_err("tampered bytes must not be stored");
    assert!(
        matches!(
            error,
            TransferError::ChecksumMismatch(_) | TransferError::SizeMismatch(_)
        ),
        "{error}"
    );

    // The session is now Failed, and there is nothing to resume: recovery is a
    // fresh reservation.
    let session = block_on(client.upload_session(&reservations[0].session_id)).expect("session");
    assert!(session.state.is_terminal_failure(), "{:?}", session.state);
    assert!(session.asset.is_none());
}

/// A role in the wrong master field is refused. The client encodes the pairing
/// and the service checks it, so this asserts the *service* by hand rather than
/// through the client: a client that guessed would bind a thumbnail into the
/// original's slot and produce a photo whose "original" is a thumbnail.
#[test]
fn the_role_and_field_pairing_is_the_services_to_enforce() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let client = server.client();
    let pinned: NativePinnedScope = block_on(client.probe()).expect("probe");
    let zone = NativeZone::new("PrimarySync");
    let (original_digest, _, _, _) = photo_fixtures();

    let lease = lease_at_create(&client, &zone);
    let url = format!(
        "{}database/1/{}/{}/{}/assets/upload",
        server.base_url,
        pinned.namespace.container,
        pinned.namespace.environment,
        pinned.namespace.database
    );
    // A thumbnail role in the original's field: exactly the swap a careless
    // client would produce.
    let body = json!({
        "zoneID": {"zoneName": "PrimarySync", "ownerRecordName": "_defaultOwner"},
        "tokens": [{
            "recordName": "master-swapped",
            "recordType": "CPLMaster",
            "fieldName": "resOriginalRes",
            "_syncserver": {
                "role": "thumbnail",
                "intent": "create",
                "expectedSize": original_digest.size,
                "expectedSha256": original_digest.hex,
            },
        }],
    });

    let status = block_on(async {
        let mut request = reqwest::Client::new()
            .post(&url)
            .bearer_auth(&server.api_key)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(&body).unwrap());
        for (name, value) in photo_viewer::core::sync::native::lease::lease_headers(lease.proof()) {
            request = request.header(name, value);
        }
        request.send().await.expect("reservation request")
    });
    assert!(
        !status.status().is_success(),
        "a swapped role/field pairing must be refused"
    );
}

/// A reservation only exists in the phase it was made for.
#[test]
fn a_reservation_in_the_wrong_phase_is_refused() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let client = server.client();
    let pinned: NativePinnedScope = block_on(client.probe()).expect("probe");
    let zone = NativeZone::new("PrimarySync");
    let (original_digest, _, _, _) = photo_fixtures();

    // Acquire, but stop at `modify`.
    let (grant, started) =
        block_on(client.acquire_lease("photoviewer-test", &zone)).expect("acquire");
    let lease = UploadLease::from_grant(&grant, started);
    let token = block_on(client.snapshot(&zone))
        .expect("snapshot")
        .sync_token;
    block_on(client.check_lease(lease.proof(), &zone, &token, true)).expect("check");
    block_on(client.set_phase(lease.proof(), UploadPhase::Delete)).unwrap();
    block_on(client.set_phase(lease.proof(), UploadPhase::Modify)).unwrap();

    // A create reservation while the lease is in `modify` is refused: the
    // client cannot create a record in a phase that is still repairing.
    let error = block_on(client.reserve_assets(
        &pinned.namespace,
        &zone,
        lease.proof(),
        "master-phase",
        UploadIntent::Create,
        &[ReservationRequest {
            role: AssetRole::Original,
            intent: UploadIntent::Create,
            digest: original_digest.clone(),
        }],
    ))
    .expect_err("a create reservation during modify must be refused");
    assert!(
        error.to_string().contains("UPLOAD_PHASE_MISMATCH"),
        "{error:?}"
    );
}

/// A description is per-read and expiring; the identity is the asset id plus its
/// digest, which is what makes a re-download verifiable.
#[test]
fn a_description_is_fresh_and_still_verifiable() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let client = server.client();
    let pinned: NativePinnedScope = block_on(client.probe()).expect("probe");
    let zone = NativeZone::new("PrimarySync");
    let (original_digest, original_bytes, _, _) = photo_fixtures();

    let lease = lease_at_create(&client, &zone);
    let reservations = block_on(client.reserve_assets(
        &pinned.namespace,
        &zone,
        lease.proof(),
        "master-fresh",
        UploadIntent::Create,
        &[ReservationRequest {
            role: AssetRole::Original,
            intent: UploadIntent::Create,
            digest: original_digest.clone(),
        }],
    ))
    .expect("reserve");
    let asset_id =
        block_on(client.upload_asset_bytes(&reservations[0], lease.proof(), &original_bytes))
            .expect("upload")
            .asset_id;

    let first = block_on(client.describe_asset(&asset_id, AssetRole::Original)).expect("describe");
    let second = block_on(client.describe_asset(&asset_id, AssetRole::Original)).expect("describe");
    // Same asset, same digest: identity is stable across reads.
    assert_eq!(first.asset_id(), second.asset_id());
    assert_eq!(
        first.descriptor.file_checksum,
        second.descriptor.file_checksum
    );
    assert_eq!(
        ContentDigest::hex_from_descriptor_base64(&first.descriptor.file_checksum).as_deref(),
        Some(original_digest.hex.as_str())
    );
    // The capability carries an expiry, which is the whole reason it is never
    // stored as identity. Two reads inside one window legitimately return the
    // same URL; what matters is that it cannot outlive its window.
    assert!(
        first.download_url.contains("expires=") && first.download_url.contains("token="),
        "{}",
        first.download_url
    );
    assert_eq!(first.receipt, second.receipt, "one window, one capability");

    // And the bytes are still exactly what was declared.
    let bytes =
        block_on(client.download_asset(&first, AssetRole::Original, None)).expect("download");
    assert_eq!(bytes.bytes, original_bytes);
    assert_eq!(
        ContentDigest::of(&bytes.bytes).hex,
        original_digest.hex,
        "what comes back must hash to what was declared"
    );
}

/// A value the service never stored must be reported, not coerced.
#[test]
fn an_unknown_asset_is_a_miss_not_an_empty_descriptor() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let client = server.client();
    let error = block_on(
        client.describe_asset("00000000-0000-4000-8000-000000000000", AssetRole::Original),
    )
    .expect_err("an unknown asset must not describe");
    assert!(matches!(error, SyncError::NotFound(_)), "{error:?}");

    // A receipt for an operation nobody issued is unknown, not a failure.
    assert_eq!(
        block_on(client.operation_status("never-issued")).unwrap(),
        photo_viewer::core::sync::OperationState::Unknown
    );
}

/// Reserve more than the service allows in one request, so the refusal is
/// observed rather than assumed.
#[test]
fn an_empty_reservation_is_refused() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let client = server.client();
    let pinned: NativePinnedScope = block_on(client.probe()).expect("probe");
    let zone = NativeZone::new("PrimarySync");
    let lease = lease_at_create(&client, &zone);
    block_on(client.reserve_assets(
        &pinned.namespace,
        &zone,
        lease.proof(),
        "master-empty",
        UploadIntent::Create,
        &[],
    ))
    .expect_err("an empty reservation must be refused");
}

/// One actual client cycle, including the actor, disk publication and server.
/// Unlike transport tests, this does not manufacture verified resource rows.
#[test]
fn two_clients_create_receive_edit_and_replace_a_complete_photo() {
    let (logs, _subscriber) = logging::Capture::install();
    let sandbox = photo_viewer::testing::Sandbox::install();
    let Some(server) = Server::start_with_lease(2, 1) else {
        return skip();
    };
    let root = sandbox.scratch("native-cycle");
    let source = root.path().join("source");
    let target = root.path().join("target");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&target).unwrap();
    let path = common::write_plain_jpeg(&source, "photo.jpg");
    block_on(async {
        use photo_viewer::core::{
            db,
            db_actor::{start_db_actor, DbCommand},
            events::{ChangeSource, DomainEventSender},
            identity::MediaId,
            media::NewMediaItem,
            sync::{NewSyncJob, ProviderKind, SyncDirection, SyncService, UploadScope},
        };
        let client = server.client();
        let scope = client.probe().await.unwrap();
        let (sender_a, _events_a) = DomainEventSender::new();
        let pool_a = db::init_pool(&root.path().join("a.sqlite")).unwrap();
        assert!(pool_a
            .get()
            .unwrap()
            .prepare("SELECT capture_present,capture_photo FROM sync_tasks")
            .is_ok());
        assert!(pool_a
            .get()
            .unwrap()
            .prepare("SELECT native_capture FROM sync_tasks")
            .is_err());
        let actor_a = start_db_actor(pool_a.clone(), sender_a);
        let key = server.api_key.clone();
        let reads = std::sync::atomic::AtomicUsize::new(0);
        let a = SyncService::with_actor(pool_a.clone(), actor_a.clone()).with_credential_reader(
            move |_| {
                // The third credential lookup happens during held-lock catch-up.
                // It blocks a worker, while the independent heartbeat must renew.
                if reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 2 {
                    std::thread::sleep(Duration::from_millis(3200));
                }
                Ok(key.clone())
            },
        );
        let (sender_b, mut events_b) = DomainEventSender::new();
        let pool_b = db::init_pool(&root.path().join("b.sqlite")).unwrap();
        let actor_b = start_db_actor(pool_b.clone(), sender_b);
        let key = server.api_key.clone();
        let b = SyncService::with_actor(pool_b.clone(), actor_b.clone())
            .with_credential_reader(move |_| Ok(key.clone()));
        let item = |path: &Path| {
            common::cache_local_thumbnail(path);
            NewMediaItem {
                uri: photo_viewer::core::file_uri::from_path(path),
                path: path.into(),
                folder_path: path.parent().unwrap().into(),
                mime_type: "image/jpeg".into(),
                media_subkind: "standard".into(),
                media_attributes: "{}".into(),
                width: Some(64),
                height: Some(48),
                video_duration_secs: None,
                taken_at: None,
                file_mtime: std::fs::metadata(path).unwrap().modified().unwrap().into(),
                file_size: std::fs::metadata(path).unwrap().len(),
                blake3_hash: String::new(),
            }
        };
        actor_a
            .execute_blocking(DbCommand::UpsertMediaBatch {
                source: ChangeSource::UserInteractive,
                items: vec![item(&path)],
            })
            .unwrap();
        let job = |root: PathBuf| NewSyncJob {
            provider_kind: ProviderKind::NativeSyncServer,
            endpoint: server.base_url.clone(),
            credential_ref: "isolated-test-key".into(),
            local_root: root,
            direction: SyncDirection::Bidirectional,
            upload_scope: UploadScope::All,
            upload_albums: vec![],
            native_zone: Some(NativeZone::new("PrimarySync")),
            native_pinned: Some(scope.clone()),
        };
        let job_a = a.store().create_job(&job(source.clone())).unwrap();
        let job_b = b.store().create_job(&job(target.clone())).unwrap();
        let created = a.run_cycle(job_a.id).await.unwrap();
        assert!(created.failures.is_empty(), "{created:?}");
        assert_eq!(created.mutations_confirmed, 1, "{created:?}");
        assert_eq!(created.outstanding, 0);
        let source_id = db::list_all_media(&pool_a).unwrap()[0].id;
        actor_a
            .execute_blocking(DbCommand::SetFavorite {
                ids: vec![MediaId::from(source_id)],
                is_favorite: true,
            })
            .unwrap();
        let metadata = a.run_cycle(job_a.id).await.unwrap();
        assert!(metadata.failures.is_empty(), "{metadata:?}");
        assert_eq!(metadata.mutations_confirmed, 1);
        let received = b.run_cycle(job_b.id).await.unwrap();
        assert!(received.failures.is_empty(), "{received:?}");
        assert_eq!(received.outstanding, 0);
        assert_eq!(
            std::fs::read(target.join("photo.jpg")).unwrap(),
            std::fs::read(&path).unwrap()
        );
        let local = db::list_all_media(&pool_b).unwrap();
        assert_eq!(local.len(), 1);
        assert!(local[0].is_favorite);
        assert_eq!(
            local[0].storage_location,
            photo_viewer::core::media::MediaLocation::Both
        );
        assert!(local[0].thumbnail_ready);
        let received_id = local[0].id;
        assert!(
            std::iter::from_fn(|| events_b.try_recv().ok()).any(|event| matches!(
                event,
                photo_viewer::core::events::DomainEvent::MediaUpserted {
                    source: ChangeSource::Synchronization,
                    ..
                }
            ))
        );
        // Both installations have local entry id 1 and first metadata version 1.
        // Their actual server mutations must still carry distinct operation IDs.
        actor_b
            .execute_blocking(DbCommand::SetFavorite {
                ids: vec![MediaId::from(received_id)],
                is_favorite: false,
            })
            .unwrap();
        let changed = b.run_cycle(job_b.id).await.unwrap();
        assert!(changed.failures.is_empty(), "{changed:?}");
        assert_eq!(changed.outstanding, 0);
        let pulled = a.run_cycle(job_a.id).await.unwrap();
        assert!(pulled.failures.is_empty(), "{pulled:?}");
        assert!(!db::get_media_item(&pool_a, source_id).unwrap().is_favorite);
        common::write_distinct_jpeg(&path, 96, 80, [220, 30, 60]);
        actor_a
            .execute_blocking(DbCommand::UpdateEditedMedia {
                id: MediaId::from(source_id),
                item: item(&path),
            })
            .unwrap();
        let updated = a.run_cycle(job_a.id).await.unwrap();
        assert!(updated.failures.is_empty(), "{updated:?}");
        assert_eq!(updated.outstanding, 0);
        let replaced = b.run_cycle(job_b.id).await.unwrap();
        assert!(replaced.failures.is_empty(), "{replaced:?}");
        assert_eq!(replaced.outstanding, 0);
        assert_eq!(
            std::fs::read(target.join("photo.jpg")).unwrap(),
            std::fs::read(&path).unwrap()
        );
        let local = db::list_all_media(&pool_b).unwrap();
        assert_eq!(local.len(), 1);
        assert_eq!(local[0].id, received_id);
        let report = b.run_cycle(job_b.id).await.unwrap();
        assert!(report.failures.is_empty(), "{report:?}");
        assert_eq!(report.outstanding, 0);
        assert_eq!(report.mutations_sent, 0);
        // A uploads first, but B edited later: edit time, not upload order,
        // chooses B's bytes for this same cloud record on both installations.
        use photo_viewer::core::sync::store::PhotoAction;
        common::write_distinct_jpeg(&path, 96, 80, [10, 100, 200]);
        actor_a
            .execute_blocking(DbCommand::UpdateEditedMedia {
                id: MediaId::from(source_id),
                item: item(&path),
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(5));
        common::write_distinct_jpeg(&target.join("photo.jpg"), 96, 80, [90, 80, 70]);
        actor_b
            .execute_blocking(DbCommand::UpdateEditedMedia {
                id: MediaId::from(received_id),
                item: item(&target.join("photo.jpg")),
            })
            .unwrap();
        let newer_b = std::fs::read(target.join("photo.jpg")).unwrap();
        assert_eq!(a.run_cycle(job_a.id).await.unwrap().outstanding, 0);
        let merged = b.run_cycle(job_b.id).await.unwrap();
        assert!(merged.failures.is_empty(), "{merged:?}");
        assert_eq!(merged.outstanding, 0);
        assert!(b.store().managed_photos(job_b.id).unwrap()[0]
            .conflict
            .is_none());
        assert_eq!(a.run_cycle(job_a.id).await.unwrap().outstanding, 0);
        assert_eq!(std::fs::read(&path).unwrap(), newer_b);
        assert_eq!(std::fs::read(target.join("photo.jpg")).unwrap(), newer_b);

        // B edits first, A edits later and publishes: B automatically receives
        // A's winner while retaining its independently newer favorite edit.
        common::write_distinct_jpeg(&target.join("photo.jpg"), 96, 80, [44, 55, 66]);
        actor_b
            .execute_blocking(DbCommand::UpdateEditedMedia {
                id: MediaId::from(received_id),
                item: item(&target.join("photo.jpg")),
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(5));
        common::write_distinct_jpeg(&path, 96, 80, [11, 22, 33]);
        actor_a
            .execute_blocking(DbCommand::UpdateEditedMedia {
                id: MediaId::from(source_id),
                item: item(&path),
            })
            .unwrap();
        actor_b
            .execute_blocking(DbCommand::SetFavorite {
                ids: vec![MediaId::from(received_id)],
                is_favorite: true,
            })
            .unwrap();
        let newer_a = std::fs::read(&path).unwrap();
        assert_eq!(a.run_cycle(job_a.id).await.unwrap().outstanding, 0);
        let merged = b.run_cycle(job_b.id).await.unwrap();
        assert!(merged.failures.is_empty(), "{merged:?}");
        assert_eq!(merged.outstanding, 0);
        assert!(b.store().managed_photos(job_b.id).unwrap()[0]
            .conflict
            .is_none());
        assert_eq!(std::fs::read(target.join("photo.jpg")).unwrap(), newer_a);
        assert!(
            db::get_media_item(&pool_b, received_id)
                .unwrap()
                .is_favorite
        );
        assert_eq!(a.run_cycle(job_a.id).await.unwrap().outstanding, 0);
        assert!(db::get_media_item(&pool_a, source_id).unwrap().is_favorite);
        assert_eq!(std::fs::read(&path).unwrap(), newer_a);
        // A live local original cannot manufacture deletion state 4.
        let captured = a.store().managed_photos(job_a.id).unwrap().remove(0);
        assert!(a
            .accept_photo_action(job_a.id, captured, PhotoAction::DeleteCloud)
            .await
            .is_err());
        assert_eq!(a.run_cycle(job_a.id).await.unwrap().mutations_sent, 0);
        assert!(path.is_file());
        assert!(target.join("photo.jpg").is_file());

        // Identity, not path, ties the edits together: A renames the record
        // while B edits its content. Both changes must eventually be local.
        common::write_distinct_jpeg(&target.join("photo.jpg"), 96, 80, [76, 54, 32]);
        actor_b
            .execute_blocking(DbCommand::UpdateEditedMedia {
                id: MediaId::from(received_id),
                item: item(&target.join("photo.jpg")),
            })
            .unwrap();
        let winning_content = std::fs::read(target.join("photo.jpg")).unwrap();
        photo_viewer::core::repository::MediaRepository::new(pool_a.clone())
            .rename_media_file_with_actor(MediaId::from(source_id), "renamed", &actor_a)
            .unwrap();
        assert_eq!(a.run_cycle(job_a.id).await.unwrap().outstanding, 0);
        for _ in 0..2 {
            let round = b.run_cycle(job_b.id).await.unwrap();
            assert!(round.failures.is_empty(), "{round:?}");
            assert_eq!(round.outstanding, 0);
        }
        assert_eq!(a.run_cycle(job_a.id).await.unwrap().outstanding, 0);
        assert_eq!(
            std::fs::read(source.join("renamed.jpg")).unwrap(),
            winning_content
        );
        assert_eq!(
            std::fs::read(target.join("renamed.jpg")).unwrap(),
            winning_content
        );
        assert!(!path.exists());
        assert!(!target.join("photo.jpg").exists());
        assert_eq!(
            db::get_media_item(&pool_b, received_id).unwrap().path,
            target.join("renamed.jpg")
        );
        assert!(b
            .store()
            .managed_photos(job_b.id)
            .unwrap()
            .iter()
            .all(|photo| photo.conflict.is_none()));
        actor_b
            .execute_blocking(DbCommand::SetFavorite {
                ids: vec![MediaId::from(received_id)],
                is_favorite: false,
            })
            .unwrap();
        let after_rename = b.run_cycle(job_b.id).await.unwrap();
        assert!(after_rename.failures.is_empty(), "{after_rename:?}");
        assert_eq!(after_rename.mutations_confirmed, 1);
        assert_eq!(b.store().managed_photos(job_b.id).unwrap().len(), 1);
        assert_eq!(a.run_cycle(job_a.id).await.unwrap().outstanding, 0);
        assert!(!db::get_media_item(&pool_a, source_id).unwrap().is_favorite);
        assert!(target.read_dir().unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".photoviewer-sync-")));
    });
    for message in [
        "云同步全量数据批次已保存",
        "云同步增量数据批次已保存",
        "云同步资源下载完成",
        "云同步照片发布批次完成",
        "云同步下载批次结束",
    ] {
        assert!(
            !logs.lines(message).is_empty(),
            "missing {message}: {}",
            logs.text()
        );
    }
    assert!(
        !logs.text().contains(&server.api_key),
        "credentials must not enter sync logs"
    );
}

#[test]
fn a_second_job_downloads_from_its_own_server_and_credential() {
    use photo_viewer::core::{
        db,
        db_actor::{start_db_actor, DbCommand},
        events::{ChangeSource, DomainEventSender},
        media::NewMediaItem,
        sync::{NewSyncJob, ProviderKind, SyncDirection, SyncService, UploadScope},
    };
    let sandbox = photo_viewer::testing::Sandbox::install();
    let Some(first) = Server::start() else {
        return skip();
    };
    let Some(second) = Server::start() else {
        return skip();
    };
    let root = sandbox.scratch("native-multiple-connections");
    let source = root.path().join("source");
    let unused = root.path().join("unused-first-job");
    let target = root.path().join("target-second-job");
    for path in [&source, &unused, &target] {
        std::fs::create_dir_all(path).unwrap();
    }
    let path = common::write_plain_jpeg(&source, "second-server.jpg");
    common::cache_local_thumbnail(&path);
    block_on(async {
        let first_scope = first.client().probe().await.unwrap();
        let second_scope = second.client().probe().await.unwrap();
        let job = |scope: NativePinnedScope, root: PathBuf, reference: &str| NewSyncJob {
            provider_kind: ProviderKind::NativeSyncServer,
            endpoint: scope.public_base_url.clone(),
            credential_ref: reference.into(),
            local_root: root,
            direction: SyncDirection::Bidirectional,
            upload_scope: UploadScope::All,
            upload_albums: vec![],
            native_zone: Some(NativeZone::new("PrimarySync")),
            native_pinned: Some(scope),
        };
        let pool_a = db::init_pool(&root.path().join("source.sqlite")).unwrap();
        let (sender, _events) = DomainEventSender::new();
        let actor_a = start_db_actor(pool_a.clone(), sender);
        let key = second.api_key.clone();
        let a = SyncService::with_actor(pool_a.clone(), actor_a.clone())
            .with_credential_reader(move |_| Ok(key.clone()));
        let metadata = photo_viewer::core::metadata::extract(&path).unwrap();
        actor_a
            .execute_blocking(DbCommand::UpsertMediaBatch {
                source: ChangeSource::UserInteractive,
                items: vec![NewMediaItem {
                    uri: photo_viewer::core::file_uri::from_path(&path),
                    path: path.clone(),
                    folder_path: source.clone(),
                    mime_type: metadata.mime_type,
                    media_subkind: "standard".into(),
                    media_attributes: "{}".into(),
                    width: metadata.width,
                    height: metadata.height,
                    video_duration_secs: None,
                    taken_at: None,
                    file_mtime: std::fs::metadata(&path).unwrap().modified().unwrap().into(),
                    file_size: std::fs::metadata(&path).unwrap().len(),
                    blake3_hash: String::new(),
                }],
            })
            .unwrap();
        let source_job = a
            .store()
            .create_job(&job(second_scope.clone(), source, "second"))
            .unwrap();
        let sent = a.run_cycle(source_job.id).await.unwrap();
        assert!(sent.failures.is_empty(), "{sent:?}");
        assert_eq!(sent.outstanding, 0);
        let pool_b = db::init_pool(&root.path().join("target.sqlite")).unwrap();
        let (sender, _events) = DomainEventSender::new();
        let actor_b = start_db_actor(pool_b.clone(), sender);
        let keys = std::collections::HashMap::from([
            ("first".to_string(), first.api_key.clone()),
            ("second".to_string(), second.api_key.clone()),
        ]);
        let b = SyncService::with_actor(pool_b.clone(), actor_b).with_credential_reader(
            move |reference| {
                keys.get(reference).cloned().ok_or_else(|| {
                    photo_viewer::core::error::AppError::Backend(
                        "unknown isolated credential".into(),
                    )
                })
            },
        );
        b.store()
            .create_job(&job(first_scope, unused, "first"))
            .unwrap();
        let second_job = b
            .store()
            .create_job(&job(second_scope, target.clone(), "second"))
            .unwrap();
        assert_eq!(second_job.id, 2);
        b.switch_native_account(second_job.id).await.unwrap();
        let downloaded = b.run_cycle(second_job.id).await.unwrap();
        assert!(downloaded.failures.is_empty(), "{downloaded:?}");
        assert_eq!(downloaded.outstanding, 0);
        assert_eq!(
            std::fs::read(target.join("second-server.jpg")).unwrap(),
            std::fs::read(&path).unwrap()
        );
        assert_eq!(db::list_all_media(&pool_b).unwrap().len(), 1);
    });
}

#[test]
fn receiving_path_collisions_merge_matching_bytes_and_preserve_local_variants() {
    let sandbox = photo_viewer::testing::Sandbox::install();
    let Some(server) = Server::start() else {
        return skip();
    };
    let root = sandbox.scratch("native-path-collisions");
    let source = root.path().join("source");
    let target = root.path().join("target");
    std::fs::create_dir_all(source.join("album")).unwrap();
    std::fs::create_dir_all(target.join("album")).unwrap();
    let same = common::write_plain_jpeg(&source.join("album"), "same.jpg");
    std::fs::copy(&same, target.join("album/same.jpg")).unwrap();
    common::write_distinct_jpeg(&source.join("album/different.jpg"), 64, 48, [10, 20, 30]);
    common::write_distinct_jpeg(&target.join("album/different.jpg"), 64, 48, [30, 10, 20]);
    let mut cloud = std::fs::read(source.join("album/different.jpg")).unwrap();
    let mut local = std::fs::read(target.join("album/different.jpg")).unwrap();
    let length = cloud.len().max(local.len());
    cloud.resize(length, 0);
    local.resize(length, 0);
    assert_ne!(cloud, local);
    std::fs::write(source.join("album/different.jpg"), &cloud).unwrap();
    std::fs::write(target.join("album/different.jpg"), &local).unwrap();
    std::fs::write(target.join("album/different_1.jpg"), b"occupied one").unwrap();
    std::fs::write(target.join("album/different_2.jpg"), b"occupied two").unwrap();
    let size = common::write_plain_jpeg(&source.join("album"), "size.jpg");
    let mut local_size = std::fs::read(&size).unwrap();
    local_size.extend_from_slice(b"local variant");
    std::fs::write(target.join("album/size.jpg"), &local_size).unwrap();
    block_on(async {
        use photo_viewer::core::{
            db,
            db_actor::{start_db_actor, DbCommand},
            events::{ChangeSource, DomainEventSender},
            sync::{NewSyncJob, ProviderKind, SyncDirection, SyncService, UploadScope},
        };
        let scope = server.client().probe().await.unwrap();
        let make_service = |name: &str| {
            let pool = db::init_pool(&root.path().join(format!("{name}.db"))).unwrap();
            let (sender, _receiver) = DomainEventSender::new();
            let actor = start_db_actor(pool.clone(), sender);
            let key = server.api_key.clone();
            let service = SyncService::with_actor(pool.clone(), actor.clone())
                .with_credential_reader(move |_| Ok(key.clone()));
            (service, pool, actor)
        };
        let (a, pool_a, actor_a) = make_service("a");
        let (b, pool_b, _actor_b) = make_service("b");
        let items = ["same.jpg", "different.jpg", "size.jpg"]
            .into_iter()
            .map(|name| {
                common::cache_local_thumbnail(&source.join("album").join(name));
                photo_viewer::core::backend::local::LocalBackend::new_item_from_path(
                    &source.join("album").join(name),
                )
                .unwrap()
                .unwrap()
            })
            .collect();
        actor_a
            .execute_blocking(DbCommand::UpsertMediaBatch {
                source: ChangeSource::UserInteractive,
                items,
            })
            .unwrap();
        let make_job = |path: PathBuf, upload_scope| NewSyncJob {
            provider_kind: ProviderKind::NativeSyncServer,
            endpoint: server.base_url.clone(),
            credential_ref: "collision-test".into(),
            local_root: path,
            direction: SyncDirection::Bidirectional,
            upload_scope,
            upload_albums: vec![],
            native_zone: Some(NativeZone::new("PrimarySync")),
            native_pinned: Some(scope.clone()),
        };
        let job_a = a
            .store()
            .create_job(&make_job(source.clone(), UploadScope::All))
            .unwrap();
        let job_b = b
            .store()
            .create_job(&make_job(target.clone(), UploadScope::SelectedAlbums))
            .unwrap();
        let uploaded = a.run_cycle(job_a.id).await.unwrap();
        assert!(uploaded.failures.is_empty(), "{uploaded:?}");
        assert_eq!(uploaded.mutations_confirmed, 3);
        let received = b.run_cycle(job_b.id).await.unwrap();
        assert!(received.failures.is_empty(), "{received:?}");
        assert_eq!(received.outstanding, 0);
        assert_eq!(received.mutations_sent, 0);
        for name in ["same.jpg", "different.jpg", "size.jpg"] {
            assert_eq!(
                std::fs::read(target.join("album").join(name)).unwrap(),
                std::fs::read(source.join("album").join(name)).unwrap(),
                "{name}"
            );
        }
        assert!(!target.join("album/same_1.jpg").exists());
        assert_eq!(
            std::fs::read(target.join("album/different_3.jpg")).unwrap(),
            local
        );
        assert_eq!(
            std::fs::read(target.join("album/different_1.jpg")).unwrap(),
            b"occupied one"
        );
        assert_eq!(
            std::fs::read(target.join("album/different_2.jpg")).unwrap(),
            b"occupied two"
        );
        assert_eq!(
            std::fs::read(target.join("album/size_1.jpg")).unwrap(),
            local_size
        );
        assert!(b
            .store()
            .managed_photos(job_b.id)
            .unwrap()
            .iter()
            .all(|photo| photo.conflict.is_none()));
        assert_eq!(db::list_all_media(&pool_b).unwrap().len(), 5);
        assert_eq!(db::list_all_media(&pool_a).unwrap().len(), 3);
        let again = b.run_cycle(job_b.id).await.unwrap();
        assert!(again.failures.is_empty(), "{again:?}");
        assert_eq!(again.outstanding, 0);
        assert!(!target.join("album/different_4.jpg").exists());
    });
}

#[test]
fn uplink_batches_limit_assets_to_5_and_metadata_to_20_and_resume() {
    let (logs, _subscriber) = logging::Capture::install();
    let sandbox = photo_viewer::testing::Sandbox::install();
    let Some(server) = Server::start() else {
        return skip();
    };
    let root = sandbox.scratch("native-uplink-batches");
    let source = root.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    block_on(async {
        use photo_viewer::core::{
            db,
            db_actor::{start_db_actor, DbCommand},
            events::{ChangeSource, DomainEventSender},
            identity::MediaId,
            media::NewMediaItem,
            sync::{NewSyncJob, ProviderKind, SyncDirection, SyncService, UploadScope},
        };
        let scope = server.client().probe().await.unwrap();
        let (sender, _events) = DomainEventSender::new();
        let pool = db::init_pool(&root.path().join("library.sqlite")).unwrap();
        let actor = start_db_actor(pool.clone(), sender);
        let key = server.api_key.clone();
        let service = SyncService::with_actor(pool.clone(), actor.clone())
            .with_credential_reader(move |_| Ok(key.clone()));
        let item = |path: &Path| {
            common::cache_local_thumbnail(path);
            NewMediaItem {
                uri: photo_viewer::core::file_uri::from_path(path),
                path: path.into(),
                folder_path: path.parent().unwrap().into(),
                mime_type: "image/jpeg".into(),
                media_subkind: "standard".into(),
                media_attributes: "{}".into(),
                width: Some(64),
                height: Some(48),
                video_duration_secs: None,
                taken_at: None,
                file_mtime: std::fs::metadata(path).unwrap().modified().unwrap().into(),
                file_size: std::fs::metadata(path).unwrap().len(),
                blake3_hash: String::new(),
            }
        };
        let items = (0..21)
            .map(|i| {
                item(&common::write_plain_jpeg(
                    &source,
                    &format!("photo-{i:02}.jpg"),
                ))
            })
            .collect();
        actor
            .execute_blocking(DbCommand::UpsertMediaBatch {
                source: ChangeSource::UserInteractive,
                items,
            })
            .unwrap();
        let job = service
            .store()
            .create_job(&NewSyncJob {
                provider_kind: ProviderKind::NativeSyncServer,
                endpoint: server.base_url.clone(),
                credential_ref: "batch-test-key".into(),
                local_root: source.clone(),
                direction: SyncDirection::Bidirectional,
                upload_scope: UploadScope::All,
                upload_albums: vec![],
                native_zone: Some(NativeZone::new("PrimarySync")),
                native_pinned: Some(scope),
            })
            .unwrap();
        // New photos each upload an original/thumbnail pair. Deferred entries
        // must stay dirty and drain across independently released leases.
        for (confirmed, outstanding) in [(5, 16), (5, 11), (5, 6), (5, 1), (1, 0)] {
            let report = service.run_cycle(job.id).await.unwrap();
            assert!(report.failures.is_empty(), "{report:?}");
            assert!(report.released, "{report:?}");
            assert_eq!(report.mutations_confirmed, confirmed, "{report:?}");
            assert_eq!(report.outstanding, outstanding, "{report:?}");
            assert_eq!(report.uplink_deferred, outstanding, "{report:?}");
            let overview = service.store().overview().unwrap();
            assert_eq!(overview.synced_items, 21 - outstanding, "{overview:?}");
            assert_eq!(overview.total_items, 21, "{overview:?}");
            assert!(service
                .store()
                .get_job(job.id)
                .unwrap()
                .unwrap()
                .last_error
                .is_none());
        }
        let media = db::list_all_media(&pool).unwrap();
        actor
            .execute_blocking(DbCommand::SetFavorite {
                ids: media.iter().map(|row| MediaId::from(row.id)).collect(),
                is_favorite: true,
            })
            .unwrap();
        let new_items = (21..27)
            .map(|i| {
                item(&common::write_plain_jpeg(
                    &source,
                    &format!("photo-{i:02}.jpg"),
                ))
            })
            .collect();
        actor
            .execute_blocking(DbCommand::UpsertMediaBatch {
                source: ChangeSource::UserInteractive,
                items: new_items,
            })
            .unwrap();
        // Metadata-only and asset allowances are independent within a cycle.
        for (confirmed, outstanding) in [(25, 2), (2, 0)] {
            let report = service.run_cycle(job.id).await.unwrap();
            assert!(report.failures.is_empty(), "{report:?}");
            assert!(report.released, "{report:?}");
            assert_eq!(report.mutations_confirmed, confirmed, "{report:?}");
            assert_eq!(report.outstanding, outstanding, "{report:?}");
            assert_eq!(report.uplink_deferred, outstanding, "{report:?}");
            assert!(service
                .store()
                .get_job(job.id)
                .unwrap()
                .unwrap()
                .last_error
                .is_none());
        }
        // Content replacement is an asset upload too, even in Modify phase.
        for row in media.iter().take(6) {
            common::write_distinct_jpeg(&row.path, 96, 80, [220, 30, 60]);
            actor
                .execute_blocking(DbCommand::UpdateEditedMedia {
                    id: MediaId::from(row.id),
                    item: item(&row.path),
                })
                .unwrap();
        }
        for (confirmed, outstanding) in [(5, 1), (1, 0)] {
            let report = service.run_cycle(job.id).await.unwrap();
            assert!(report.failures.is_empty(), "{report:?}");
            assert!(report.released, "{report:?}");
            assert_eq!(report.mutations_confirmed, confirmed, "{report:?}");
            assert_eq!(report.outstanding, outstanding, "{report:?}");
            assert_eq!(report.uplink_deferred, outstanding, "{report:?}");
            assert!(service
                .store()
                .get_job(job.id)
                .unwrap()
                .unwrap()
                .last_error
                .is_none());
        }
        let idle = service.run_cycle(job.id).await.unwrap();
        assert!(idle.failures.is_empty(), "{idle:?}");
        assert_eq!(idle.mutations_sent, 0, "{idle:?}");
        assert_eq!(idle.outstanding, 0, "{idle:?}");
        let failed = service.clone().with_credential_reader(|_| {
            Err(photo_viewer::core::error::AppError::Backend(
                "test credential unavailable".into(),
            ))
        });
        assert!(failed.run_cycle(job.id).await.is_err());
    });
    let starts = logs.lines("云同步批次开始");
    let ends = logs.lines("云同步批次结束");
    let failures = logs.lines("云同步批次失败");
    assert_eq!(starts.len(), 11, "{}", logs.text());
    assert_eq!(ends.len(), 10, "{}", logs.text());
    assert_eq!(failures.len(), 1, "{}", logs.text());
    for (line, (confirmed, outstanding)) in ends.iter().zip([
        (5, 16),
        (5, 11),
        (5, 6),
        (5, 1),
        (1, 0),
        (25, 2),
        (2, 0),
        (5, 1),
        (1, 0),
        (0, 0),
    ]) {
        assert!(line.contains("INFO sync:"), "{line}");
        assert!(
            line.contains(&format!("mutations_confirmed={confirmed} ")),
            "{line}"
        );
        assert!(
            line.contains(&format!("outstanding={outstanding} ")),
            "{line}"
        );
        assert!(
            line.contains(if outstanding > 0 {
                "outcome=\"pending\""
            } else {
                "outcome=\"complete\""
            }),
            "{line}"
        );
        for field in ["job_id=", "cycle_id=", "elapsed_ms=", "uplink_deferred="] {
            assert!(line.contains(field), "missing {field}: {line}");
        }
    }
    for (start, end) in starts.iter().zip(ends.iter().chain(failures.iter())) {
        let cycle = start
            .split_whitespace()
            .find(|field| field.starts_with("cycle_id="))
            .unwrap();
        assert!(
            end.contains(cycle),
            "batch correlation lost: {start} -> {end}"
        );
    }
    assert!(failures[0].contains("WARN sync:"), "{}", failures[0]);
    assert!(
        failures[0].contains("test credential unavailable"),
        "{}",
        failures[0]
    );
    assert!(
        !logs.text().contains(&server.api_key),
        "credentials must not enter sync logs"
    );
}

#[test]
fn successful_batches_chain_immediately_while_work_remains() {
    let sandbox = photo_viewer::testing::Sandbox::install();
    let _preferences = PREFERENCES.lock().unwrap();
    photo_viewer::core::prefs::set_sync_enabled(true).unwrap();
    let Some(server) = Server::start() else {
        return skip();
    };
    let (logs, _logs_guard) = Capture::install();
    let root = sandbox.scratch("native-uplink-auto-chain");
    let source = root.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    block_on(async {
        use photo_viewer::core::{
            db,
            db_actor::{start_db_actor, DbCommand},
            events::{ChangeSource, DomainEventSender},
            media::NewMediaItem,
            sync::{NewSyncJob, ProviderKind, SyncDirection, SyncService, UploadScope},
        };
        let scope = server.client().probe().await.unwrap();
        let (sender, _events) = DomainEventSender::new();
        let pool = db::init_pool(&root.path().join("library.sqlite")).unwrap();
        let actor = start_db_actor(pool.clone(), sender);
        let key = server.api_key.clone();
        let service = SyncService::with_actor(pool.clone(), actor.clone())
            .with_credential_reader(move |_| Ok(key.clone()));
        let items = (0..7)
            .map(|index| {
                let path = common::write_plain_jpeg(&source, &format!("photo-{index:02}.jpg"));
                common::cache_local_thumbnail(&path);
                NewMediaItem {
                    uri: photo_viewer::core::file_uri::from_path(&path),
                    path: path.clone(),
                    folder_path: path.parent().unwrap().into(),
                    mime_type: "image/jpeg".into(),
                    media_subkind: "standard".into(),
                    media_attributes: "{}".into(),
                    width: Some(64),
                    height: Some(48),
                    video_duration_secs: None,
                    taken_at: None,
                    file_mtime: std::fs::metadata(&path).unwrap().modified().unwrap().into(),
                    file_size: std::fs::metadata(&path).unwrap().len(),
                    blake3_hash: String::new(),
                }
            })
            .collect();
        actor
            .execute_blocking(DbCommand::UpsertMediaBatch {
                source: ChangeSource::UserInteractive,
                items,
            })
            .unwrap();
        let job = service
            .store()
            .create_job(&NewSyncJob {
                provider_kind: ProviderKind::NativeSyncServer,
                endpoint: server.base_url.clone(),
                credential_ref: "auto-chain-key".into(),
                local_root: source.clone(),
                direction: SyncDirection::Bidirectional,
                upload_scope: UploadScope::All,
                upload_albums: vec![],
                native_zone: Some(NativeZone::new("PrimarySync")),
                native_pinned: Some(scope),
            })
            .unwrap();

        service.request_job_sync(job.id).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let confirmed: i64 = pool
                .get()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM sync_entries
                      WHERE job_id=?1 AND native_dirty=0
                        AND native_confirmed_generation IS NOT NULL",
                    [job.id],
                    |row| row.get(0),
                )
                .unwrap();
            if confirmed == 7 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "seven photos must drain through two immediate batches: enabled={}, pending={}, job={:?}; logs: {}",
                photo_viewer::core::prefs::sync_enabled(),
                service.has_pending_work(job.id).unwrap(),
                service.store().get_job(job.id).unwrap(),
                logs.text()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(service
            .store()
            .get_job(job.id)
            .unwrap()
            .unwrap()
            .last_error
            .is_none());
        let confirmed: i64 = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM sync_entries
                  WHERE job_id=?1 AND native_dirty=0
                    AND native_confirmed_generation IS NOT NULL",
                [job.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(confirmed, 7);
        let delta_rounds = logs.lines("云同步增量数据批次已保存");
        assert_eq!(
            delta_rounds.len(),
            1,
            "continuation batches must upload without another downlink projection: {}",
            logs.text()
        );
    });
}

#[test]
fn a_bounded_chain_yields_then_finishes_deferred_work() {
    let sandbox = photo_viewer::testing::Sandbox::install();
    let _preferences = PREFERENCES.lock().unwrap();
    photo_viewer::core::prefs::set_sync_enabled(true).unwrap();
    let Some(server) = Server::start() else {
        return skip();
    };
    let (logs, _logs_guard) = Capture::install();
    let root = sandbox.scratch("native-uplink-continuation");
    let source = root.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    block_on(async {
        use photo_viewer::core::{
            db,
            db_actor::{start_db_actor, DbCommand},
            events::{ChangeSource, DomainEventSender},
            media::NewMediaItem,
            sync::{NewSyncJob, ProviderKind, SyncDirection, SyncService, UploadScope},
        };
        let scope = server.client().probe().await.unwrap();
        let pool = db::init_pool(&root.path().join("library.sqlite")).unwrap();
        let (sender, _events) = DomainEventSender::new();
        let actor = start_db_actor(pool.clone(), sender);
        let key = server.api_key.clone();
        let service = SyncService::with_actor(pool.clone(), actor.clone())
            .with_credential_reader(move |_| Ok(key.clone()));
        let item = |path: &Path| {
            common::cache_local_thumbnail(path);
            NewMediaItem {
                uri: photo_viewer::core::file_uri::from_path(path),
                path: path.into(),
                folder_path: path.parent().unwrap().into(),
                mime_type: "image/jpeg".into(),
                media_subkind: "standard".into(),
                media_attributes: "{}".into(),
                width: Some(64),
                height: Some(48),
                video_duration_secs: None,
                taken_at: None,
                file_mtime: std::fs::metadata(path).unwrap().modified().unwrap().into(),
                file_size: std::fs::metadata(path).unwrap().len(),
                blake3_hash: String::new(),
            }
        };
        // A new photo is one asset slot. The initial cycle plus eight chained
        // ones move forty-five of forty-six photos, so only the fairness yield
        // can finish the last one.
        let items = (0..46)
            .map(|i| {
                item(&common::write_plain_jpeg(
                    &source,
                    &format!("photo-{i:02}.jpg"),
                ))
            })
            .collect();
        actor
            .execute_blocking(DbCommand::UpsertMediaBatch {
                source: ChangeSource::UserInteractive,
                items,
            })
            .unwrap();
        let job = service
            .store()
            .create_job(&NewSyncJob {
                provider_kind: ProviderKind::NativeSyncServer,
                endpoint: server.base_url.clone(),
                credential_ref: "continuation-key".into(),
                local_root: source.clone(),
                direction: SyncDirection::Bidirectional,
                upload_scope: UploadScope::All,
                upload_albums: vec![],
                native_zone: Some(NativeZone::new("PrimarySync")),
                native_pinned: Some(scope),
            })
            .unwrap();
        service.request_job_sync(job.id).unwrap();
        let deadline = Instant::now() + Duration::from_secs(120);
        // `has_unfinished_work` drops as the last record is confirmed, which can
        // happen before the worker releases its slot, so wait for both.
        while service.store().has_unfinished_work(job.id).unwrap()
            || service.has_pending_work(job.id).unwrap()
        {
            assert!(
                Instant::now() < deadline,
                "the bounded chain stranded deferred work: overview={:?}, logs: {}",
                service.store().overview(),
                logs.text()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(service.store().uplink_view(job.id).unwrap().len(), 0);
        let overview = service.store().overview().unwrap();
        assert_eq!(overview.synced_items, 46, "{overview:?}");
        assert_eq!(overview.total_items, 46, "{overview:?}");
        assert_eq!(
            logs.lines("云同步批次达到连续上限，让出调度后继续").len(),
            1
        );
    });
}
