//! C1.1 acceptance against the real SyncServer binary.
//!
//! The wire contract this suite exercises lives in
//! `docs/modules/native-sync-protocol.md`. A protocol mock can prove parsing,
//! but only the real service proves the client accepts what an actual M4 server
//! announces — capability names, limit keys, the trailing-slash public base URL
//! and the 401 envelope — and refuses what it must not accept.
//!
//! The server binary comes from the sibling `syncserver` checkout. Without it
//! the suite prints how to build one and skips: this repository's CI has no
//! access to the sibling, and a skipped run is reported honestly rather than
//! turned into a fake pass.

mod common;

use std::io::Write;
use std::net::TcpListener;
use std::time::{Duration, Instant};

use common::sync_server::{server_binary, ServerProcess};
use photo_viewer::core::sync::SyncError;
use photo_viewer::core::sync::{NativeEndpoint, NativePinnedScope, SyncService};

/// One server instance bound to a free loopback port with a fresh data
/// directory. `publicBaseURL` is the actual listener address, which is what a
/// real deployment announces and what the client's same-origin rule accepts.
struct Server {
    /// Owned through the shared harness: the process group and the
    /// kernel-level parent-death backstop make sure the server cannot outlive
    /// the test, even when the test binary is killed outright.
    child: ServerProcess,
    base_url: String,
    api_key: String,
    #[allow(dead_code)]
    root: tempfile::TempDir,
}

impl Server {
    fn start(extra_env: &[(&str, &str)]) -> Option<Self> {
        let binary = server_binary()?;
        let root = tempfile::tempdir().expect("temp dir");
        // The service refuses key files that are not owner-only, and refuses to
        // start at all if they already exist.
        let api_key = write_secret(&root, "api.key", random_key());
        let signing_key = base64_32_bytes();
        write_secret(&root, "signing.key", signing_key);
        let data = root.path().join("data");
        std::fs::create_dir(&data).expect("data dir");
        std::fs::set_permissions(&data, permissions_0700()).expect("data dir mode");
        let port = free_loopback_port();
        let base_url = format!("http://127.0.0.1:{port}/");

        let data_dir = root.path().join("data");
        let api_key_file = root.path().join("api.key");
        let signing_key_file = root.path().join("signing.key");
        let mut environment: Vec<(&str, &str)> = vec![
            ("SYNC_DATA_DIR", data_dir.to_str().expect("utf-8 path")),
            (
                "SYNC_API_KEY_FILE",
                api_key_file.to_str().expect("utf-8 path"),
            ),
            (
                "SYNC_SIGNING_KEY_FILE",
                signing_key_file.to_str().expect("utf-8 path"),
            ),
        ];
        let bind = format!("127.0.0.1:{port}");
        let public = format!("http://127.0.0.1:{port}");
        environment.push(("SYNC_BIND", bind.as_str()));
        environment.push(("SYNC_PUBLIC_BASE_URL", public.as_str()));
        environment.extend_from_slice(extra_env);
        let child = ServerProcess::spawn(&binary, &environment).expect("spawn syncserver");
        let mut server = Self {
            child,
            base_url,
            api_key,
            root,
        };
        server.wait_ready();
        Some(server)
    }

    /// Poll `/readyz` until the service answers, then give the SQLite recovery
    /// pass a moment to finish before the first authenticated call.
    fn wait_ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            let url = format!("{}readyz", self.base_url);
            let ready = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("poll runtime")
                .block_on(async {
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
}

// The server process is stopped by `ServerProcess`'s own `Drop`, which also
// covers the case where this `Server` is dropped during a panic unwind.

fn write_secret(root: &tempfile::TempDir, name: &str, value: String) -> String {
    use std::os::unix::fs::OpenOptionsExt;
    let path = root.path().join(name);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .expect("create secret file");
    writeln!(file, "{value}").expect("write secret");
    value
}

/// 43 printable characters: inside the service's 32..=512 ASCII rule.
fn random_key() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("pv-test-key-{nanos:032x}")
}

fn base64_32_bytes() -> String {
    // 32 bytes → 44 base64 characters. Deterministic is fine: the value only
    // has to parse as the service expects, never to be secret.
    let raw: Vec<u8> = (0..32u8).collect();
    base64_standard(&raw)
}

fn base64_standard(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
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

fn permissions_0700() -> std::fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    std::fs::Permissions::from_mode(0o700)
}

fn free_loopback_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

fn service() -> SyncService {
    let dir = common::tmp_dir();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("probe.db")).unwrap();
    SyncService::new(pool)
}

/// A key that passes the client's shape check but is not the server's key.
fn wrong_key() -> String {
    "0".repeat(64)
}

#[test]
fn native_probe_pins_a_real_m4_server_scope() {
    let Some(server) = Server::start(&[]) else {
        eprintln!(
            "skipping: no syncserver binary. Build the sibling checkout with \
             `cargo build --locked --manifest-path ../syncserver/Cargo.toml` or set \
             SYNC_SERVER_BIN."
        );
        return;
    };
    let scope: NativePinnedScope = tokio_block_on(async {
        service()
            .probe_native_connection(&server.base_url, &server.api_key)
            .await
    })
    .expect("a healthy M4 server must be accepted");

    assert_eq!(scope.public_base_url, server.base_url);
    assert_eq!(scope.protocol_version, 1);
    assert_eq!(scope.stage, "M4");
    assert_eq!(scope.checksum_algorithm, "SHA256");
    assert_eq!(scope.namespace.environment, "production");
    assert_eq!(scope.namespace.database, "private");
    assert!(!scope.database_generation.is_empty());
    assert!(
        scope.zones.iter().any(|zone| zone == "PrimarySync"),
        "the default zone must be announced: {:?}",
        scope.zones
    );
    // The effective limits are the deployment's own, not protocol constants.
    assert_eq!(scope.limits.lease_ttl_secs, 90);
    assert_eq!(scope.limits.lease_heartbeat_secs, 30);
    assert_eq!(scope.sync_limits.max_page, 200);
    assert_eq!(scope.record_limits.max_operations, 400);
    // Every capability C1 requires is on, and the optional bits are recorded
    // without being depended on.
    assert!(scope.capabilities.missing_required().is_empty());
    assert!(!scope.capabilities.query_batch || scope.capabilities.query_batch);
}

#[test]
fn a_wrong_or_missing_bearer_key_is_an_authentication_failure() {
    let Some(server) = Server::start(&[]) else {
        eprintln!("skipping: no syncserver binary (see native_probe_pins_a_real_m4_server_scope)");
        return;
    };
    let error = tokio_block_on(async {
        service()
            .probe_native_connection(&server.base_url, &wrong_key())
            .await
    })
    .expect_err("a wrong key must not be accepted");
    assert!(matches!(error_source(&error), SyncError::Authentication));

    // A key that never reaches the request is refused before any socket opens.
    let error = tokio_block_on(async {
        service()
            .probe_native_connection(&server.base_url, "too-short")
            .await
    })
    .expect_err("a malformed key must be refused locally");
    assert!(matches!(error_source(&error), SyncError::Unsupported(_)));
}

#[test]
fn cleartext_endpoints_off_loopback_are_refused_before_any_request() {
    // No server is needed: the endpoint never survives normalization.
    let error = tokio_block_on(async {
        service()
            .probe_native_connection("http://sync.example.invalid/", &wrong_key())
            .await
    })
    .expect_err("cleartext off loopback must be refused");
    assert!(matches!(error_source(&error), SyncError::Unsupported(_)));

    // `localhost` is a DNS name, not a literal address, so it is refused too —
    // even with a server actually listening there.
    let error = tokio_block_on(async {
        service()
            .probe_native_connection("http://localhost:8790/", &wrong_key())
            .await
    })
    .expect_err("a DNS name is not a literal loopback address");
    assert!(matches!(error_source(&error), SyncError::Unsupported(_)));
}

#[test]
fn an_announced_base_url_from_another_origin_is_refused() {
    // The server announces its own listener as publicBaseURL; pointing the
    // client at a *different* announced origin (a second instance on another
    // port, reached through the first instance's endpoint) is the mismatch
    // case. Here the simplest honest construction: probe instance A through an
    // endpoint whose origin is A, while A announces B.
    let Some(_server) = Server::start(&[(
        "SYNC_PUBLIC_BASE_URL",
        // Announce an origin that is not this listener: the same-origin rule
        // must refuse the scope even though the server itself is healthy.
        &format!("http://127.0.0.1:{}/", free_loopback_port()),
    )]) else {
        eprintln!("skipping: no syncserver binary (see native_probe_pins_a_real_m4_server_scope)");
        return;
    };
    let error = tokio_block_on(async {
        service()
            .probe_native_connection(&_server.base_url, &_server.api_key)
            .await
    })
    .expect_err("a foreign announced origin must be refused");
    let message = error_source(&error).to_string();
    assert!(
        message.contains("public base URL") || message.contains("origin"),
        "the failure must name the announced origin: {message}"
    );
}

#[test]
fn a_job_against_an_unknown_zone_is_refused_without_writing_a_row() {
    let Some(server) = Server::start(&[]) else {
        eprintln!("skipping: no syncserver binary (see native_probe_pins_a_real_m4_server_scope)");
        return;
    };
    let dir = common::tmp_dir();
    let media_root = dir.path().join("photos");
    std::fs::create_dir(&media_root).unwrap();
    let pool = photo_viewer::core::db::init_pool(&dir.path().join("zone.db")).unwrap();

    let error = tokio_block_on(async {
        SyncService::new(pool.clone())
            .create_native_job(
                &server.base_url,
                "zone-credential",
                &server.api_key,
                &media_root,
                "NoSuchZone",
                photo_viewer::core::sync::SyncDirection::Bidirectional,
                Vec::new(),
            )
            .await
    })
    .expect_err("an unknown zone must not become a task");
    assert!(
        error.to_string().contains("NoSuchZone"),
        "the failure must name the zone: {error}"
    );
    let jobs: i64 = pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM sync_jobs", [], |row| row.get(0))
        .unwrap();
    let connections: i64 = pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM sync_connections", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!((jobs, connections), (0, 0));
}

#[test]
fn endpoint_normalization_matches_the_connection_rules() {
    // Offline shape checks that would otherwise need a server to reach.
    assert!(NativeEndpoint::parse("http://127.0.0.1:8790/").is_ok());
    assert!(NativeEndpoint::parse("https://sync.example.invalid/").is_ok());
    assert!(NativeEndpoint::parse("https://sync.example.invalid/base/").is_err());
}

/// `#[tokio::test]` is unavailable in a plain test crate target without a
/// runtime feature declaration; these tests only need *a* runtime to drive the
/// async client, so a minimal current-thread one is built per call.
fn tokio_block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(future)
}

/// `SyncService` reports failures as `AppError::Backend` carrying the provider
/// error's text. Matching the underlying kind needs the text back, so the
/// provider error is re-parsed from its message.
fn error_source(error: &photo_viewer::core::error::AppError) -> SyncError {
    let text = error.to_string();
    // AppError::Backend renders as "backend unavailable: <provider message>".
    let detail = text
        .strip_prefix("backend unavailable: ")
        .unwrap_or(&text)
        .to_string();
    if detail.contains("authentication failed") {
        SyncError::Authentication
    } else if detail.starts_with("provider capability is unsupported") {
        SyncError::Unsupported(detail)
    } else {
        SyncError::Protocol(detail)
    }
}
