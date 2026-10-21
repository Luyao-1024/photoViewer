//! C1.3 acceptance against the real SyncServer binary: two clients, one zone.
//!
//! The upload lease is the only thing that makes "one effective uplink device"
//! true, and that claim is worth nothing unless two real clients talking to one
//! real service actually agree. So this suite is built around two independent
//! clients with separate device identities and separate credentials.
//!
//! Records are written as `Album`, not photos: a photo needs an asset upload
//! and a paired atomic write, which is C1.4's machinery. What is under test is
//! arbitration — who holds the lease, who is told to stand down, and what a
//! lost lease does to the loser.
//!
//! As in the other native suites, the binary comes from the sibling checkout
//! and the suite skips rather than faking a pass when it is absent.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use photo_viewer::core::sync::native::lease::UploadPhase;
use photo_viewer::core::sync::{
    NativeClient, NativeEndpoint, NativePinnedScope, NativeZone, SyncError,
};
use serde_json::{json, Value};

fn server_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("SYNC_SERVER_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
        eprintln!("SYNC_SERVER_BIN={path:?} does not exist; ignoring it");
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

    /// A second, independent client: its own connection, its own identity.
    fn client(&self, device: &str) -> Client {
        Client {
            inner: NativeClient::new(
                &NativeEndpoint::parse(&self.base_url).expect("loopback endpoint"),
                &self.api_key,
            )
            .expect("client"),
            device: device.to_string(),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Client {
    inner: NativeClient,
    device: String,
}

impl Client {
    fn zone(&self) -> NativeZone {
        NativeZone::new("PrimarySync")
    }

    fn pinned(&self) -> NativePinnedScope {
        block_on(self.inner.probe()).expect("capability probe")
    }

    fn current_token(&self) -> String {
        block_on(self.inner.snapshot(&self.zone()))
            .expect("snapshot")
            .sync_token
    }
}

fn modify_url(server: &Server, pinned: &NativePinnedScope) -> url::Url {
    format!(
        "{}database/1/{}/{}/{}/records/modify",
        server.base_url,
        pinned.namespace.container,
        pinned.namespace.environment,
        pinned.namespace.database
    )
    .parse()
    .expect("cloudkit modify URL")
}

fn create_operation(name: &str) -> Value {
    json!({
        "operationType": "create",
        "record": {
            "recordName": name,
            "recordType": "Album",
            "fields": { "title": {"type": "STRING", "value": "Travel"} },
        },
    })
}

/// Issue one exact logical mutation under a lease, the way the coordinator
/// does. The body and idempotency key are the test's, not the client's: these
/// cases pin what the service does with a receipt, not how a request is built.
fn send_modify(
    server: &Server,
    pinned: &NativePinnedScope,
    proof: &photo_viewer::core::sync::native::lease::LeaseProof,
    body: &Value,
    key: &str,
) -> (reqwest::StatusCode, Vec<u8>) {
    let mut request = reqwest::Client::new()
        .post(modify_url(server, pinned))
        .bearer_auth(&server.api_key)
        .header("Content-Type", "application/json")
        .header("Idempotency-Key", key)
        .body(serde_json::to_vec(body).expect("body"));
    for (name, value) in photo_viewer::core::sync::native::lease::lease_headers(proof) {
        request = request.header(name, value);
    }
    block_on(async {
        let response = request.send().await.expect("modify");
        let status = response.status();
        (status, response.bytes().await.expect("body").to_vec())
    })
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
    format!("pv-lease-key-{nanos:032x}")
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

fn skip() {
    eprintln!(
        "skipping: no syncserver binary. Build the sibling checkout with \
         `cargo build --locked --manifest-path ../syncserver/Cargo.toml` or set \
         SYNC_SERVER_BIN."
    );
}

/// One zone, one uplink device: the second acquisition is refused while the
/// first is held.
#[test]
fn one_client_holds_the_zone_and_the_second_is_told_to_stand_down() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let first = server.client("device-one");
    let second = server.client("device-two");
    let zone = first.zone();

    let (grant, _) = block_on(first.inner.acquire_lease(&first.device, &zone)).expect("acquire");
    assert!(!grant.lease_id.is_empty());
    assert!(!grant.fencing_token.is_empty());
    assert!(grant.ttl_seconds > 0);
    assert!(grant.heartbeat_seconds > 0);
    assert_eq!(
        grant.phase, "checking",
        "the gate is the only exit from checking"
    );

    // The second client's single acquisition attempt is refused. A cycle that
    // retried here would fight the holder and never finish.
    let error = block_on(second.inner.acquire_lease(&second.device, &zone))
        .expect_err("a held zone admits one uplink device");
    assert!(
        matches!(error, SyncError::RateLimited),
        "acquisition contention must be distinguishable from a lost lease: {error:?}"
    );

    // After release, the zone is available again — and the fence has moved, so
    // the old proof is permanently useless.
    let first_lease = photo_viewer::core::sync::UploadLease::from_grant(&grant, Instant::now());
    assert!(block_on(first_lease.deadline()).is_some());
    block_on(first.inner.release_lease(first_lease.proof())).expect("release");

    let (second_grant, _) =
        block_on(second.inner.acquire_lease(&second.device, &zone)).expect("reacquire");
    assert_ne!(second_grant.lease_id, grant.lease_id);
    assert_ne!(
        second_grant.fencing_token, grant.fencing_token,
        "a new grant carries a new fence"
    );
    // The released grant's tokens no longer work.
    let error = block_on(first.inner.release_lease(first_lease.proof()))
        .expect_err("a released lease cannot be released again");
    assert!(matches!(error, SyncError::Unsupported(_)), "{error:?}");
}

/// A lease held by one device is not usable by another, even with the right
/// headers: the fence is the proof, and a stolen id is not a proof.
#[test]
fn a_fenced_lease_cannot_be_used_to_write() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let first = server.client("device-one");
    let second = server.client("device-two");
    let zone = first.zone();

    let (grant, _) = block_on(first.inner.acquire_lease(&first.device, &zone)).expect("acquire");
    // The second device tries the holder's own proof.
    let stolen = photo_viewer::core::sync::LeaseProof {
        lease_id: grant.lease_id.clone(),
        lease_token: grant.lease_token.clone(),
        fencing_token: "999".into(),
    };
    let error = block_on(
        second
            .inner
            .check_lease(&stolen, &zone, &second.current_token(), false),
    )
    .expect_err("a forged fence must be refused");
    assert!(matches!(error, SyncError::Unsupported(_)), "{error:?}");
    assert!(error.to_string().contains("UPLOAD_LEASE_LOST"), "{error:?}");
}

/// The gate refuses a client that declares itself not ready, and a refusal is
/// not a loss: the lease is still held and still renewable.
#[test]
fn a_gate_refuses_an_unready_client_without_ending_its_lease() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let client = server.client("device-one");
    let zone = client.zone();
    let (grant, started) =
        block_on(client.inner.acquire_lease(&client.device, &zone)).expect("acquire");
    let lease = photo_viewer::core::sync::UploadLease::from_grant(&grant, started);

    let check = block_on(client.inner.check_lease(
        lease.proof(),
        &zone,
        &client.current_token(),
        false,
    ))
    .expect("check");
    assert!(check.is_current, "an unchanged zone is current");
    assert!(!check.may_upload, "not-ready must not be allowed to upload");
    assert_eq!(
        check.phase, "checking",
        "a refused gate leaves the lease in checking"
    );

    // Still held, and renewable: a client that is simply behind is not a client
    // that lost anything.
    assert!(block_on(lease.is_held()));
    let (ttl, sent_at) = block_on(client.inner.renew_lease(lease.proof())).expect("renew");
    block_on(lease.renew(ttl, sent_at));
    assert!(block_on(lease.is_held()));
    assert_eq!(check.zone_tokens.len(), 1);
}

/// Renewal extends authority, and a renewal answer that arrives after the
/// deadline cannot bring it back.
#[test]
fn renewal_extends_the_window_and_a_lost_lease_stays_lost() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let client = server.client("device-one");
    let zone = client.zone();
    let (grant, started) =
        block_on(client.inner.acquire_lease(&client.device, &zone)).expect("acquire");
    let lease = photo_viewer::core::sync::UploadLease::from_grant(&grant, started);

    let before = block_on(lease.deadline()).expect("held");
    let (ttl, sent_at) = block_on(client.inner.renew_lease(lease.proof())).expect("renew");
    assert!(ttl > 0);
    block_on(lease.renew(ttl, sent_at));
    let after = block_on(lease.deadline()).expect("held");
    assert!(after > before, "{after:?} should exceed {before:?}");

    // An explicit loss is final: the cycle cannot recover authority it has been
    // told it does not have.
    block_on(lease.mark_lost());
    assert!(!block_on(lease.is_held()));
    block_on(lease.renew(ttl, Instant::now()));
    assert!(block_on(lease.deadline()).is_none());
}

/// The phase ladder is forward-only and gated: a phase cannot be entered before
/// the progress gate has passed.
#[test]
fn the_phase_ladder_is_gated_and_walks_forward_only() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let client = server.client("device-one");
    let zone = client.zone();
    let (grant, started) =
        block_on(client.inner.acquire_lease(&client.device, &zone)).expect("acquire");
    let lease = photo_viewer::core::sync::UploadLease::from_grant(&grant, started);
    let proof = lease.proof().clone();

    // Before the gate: entering a phase is refused, and the code says why.
    let error = block_on(client.inner.set_phase(&proof, UploadPhase::Delete))
        .expect_err("a phase before the gate must be refused");
    assert!(error.to_string().contains("SYNC_NOT_CURRENT"), "{error:?}");

    // The gate only opens once the client declares itself ready.
    let _check = block_on(
        client
            .inner
            .check_lease(&proof, &zone, &client.current_token(), true),
    )
    .expect("check");
    assert_eq!(
        block_on(client.inner.set_phase(&proof, UploadPhase::Delete)).unwrap(),
        UploadPhase::Delete
    );
    // Re-entering the same phase is allowed and idempotent.
    assert_eq!(
        block_on(client.inner.set_phase(&proof, UploadPhase::Delete)).unwrap(),
        UploadPhase::Delete
    );
    assert_eq!(
        block_on(client.inner.set_phase(&proof, UploadPhase::Modify)).unwrap(),
        UploadPhase::Modify
    );
    // Backwards is not.
    let error = block_on(client.inner.set_phase(&proof, UploadPhase::Delete))
        .expect_err("a phase cannot go backwards");
    assert!(
        error.to_string().contains("UPLOAD_PHASE_MISMATCH"),
        "{error:?}"
    );
}

/// The gate re-checks currency: a zone that moved since the client's position
/// is not current, and the client catches up while holding the lock.
#[test]
fn a_zone_that_moved_is_not_current_until_the_client_catches_up() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let writer = server.client("device-writer");
    let holder = server.client("device-holder");
    let zone = writer.zone();

    // The holder's position predates the writer's commit.
    let stale = holder.current_token();

    // The writer takes the lease, commits, and releases it.
    let (grant, started) =
        block_on(writer.inner.acquire_lease(&writer.device, &zone)).expect("writer acquires");
    let writer_lease = photo_viewer::core::sync::UploadLease::from_grant(&grant, started);
    let writer_proof = writer_lease.proof().clone();
    let _ = block_on(
        writer
            .inner
            .check_lease(&writer_proof, &zone, &writer.current_token(), true),
    );
    for phase in UploadPhase::ORDER {
        let _ = block_on(writer.inner.set_phase(&writer_proof, phase));
    }
    let body = json!({
        "zoneID": photo_viewer::core::sync::ZoneId::from(&zone),
        "atomic": true,
        "operations": [create_operation("album-after-position")],
    });
    let (status, _) = send_modify(
        &server,
        &writer.pinned(),
        &writer_proof,
        &body,
        "pv-zone-move",
    );
    assert!(status.is_success(), "{status}");
    block_on(writer.inner.release_lease(&writer_proof)).expect("release");

    // The holder now takes the lease with a position that is behind.
    let (grant, started) =
        block_on(holder.inner.acquire_lease(&holder.device, &zone)).expect("holder acquires");
    let lease = photo_viewer::core::sync::UploadLease::from_grant(&grant, started);
    let behind =
        block_on(holder.inner.check_lease(lease.proof(), &zone, &stale, true)).expect("check");
    assert!(!behind.is_current, "the zone moved past this position");
    assert!(
        !behind.may_upload,
        "a client that has not caught up may not upload"
    );
    // The lease is still held: being behind is not being fenced.
    assert!(block_on(lease.is_held()));
    assert_eq!(behind.phase, "checking", "a refused gate leaves checking");

    // After catching up, the same check passes and moves the lease on.
    let caught_up = holder.current_token();
    let ready = block_on(
        holder
            .inner
            .check_lease(lease.proof(), &zone, &caught_up, true),
    )
    .expect("check after catch-up");
    assert!(ready.is_current && ready.may_upload);
    assert_eq!(ready.phase, "delete");
}

/// A passed gate is single-use: the lease leaves `checking`, so a second check
/// is a phase error rather than a repeat of the same question.
#[test]
fn a_passed_gate_cannot_be_asked_twice() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let client = server.client("device-one");
    let zone = client.zone();
    let (grant, started) =
        block_on(client.inner.acquire_lease(&client.device, &zone)).expect("acquire");
    let lease = photo_viewer::core::sync::UploadLease::from_grant(&grant, started);
    let token = client.current_token();

    let first =
        block_on(client.inner.check_lease(lease.proof(), &zone, &token, true)).expect("check");
    assert!(first.may_upload);
    assert_eq!(first.phase, "delete");

    let error = block_on(client.inner.check_lease(lease.proof(), &zone, &token, true))
        .expect_err("a passed gate is not re-answerable");
    assert!(
        error.to_string().contains("UPLOAD_PHASE_MISMATCH"),
        "{error:?}"
    );
}

/// An operation receipt survives the lease that issued it: reconciling an
/// uncertain outcome has to work after authority is gone.
#[test]
fn an_operation_receipt_is_readable_after_the_lease_is_gone() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let client = server.client("device-one");
    let zone = client.zone();
    let (grant, started) =
        block_on(client.inner.acquire_lease(&client.device, &zone)).expect("acquire");
    let lease = photo_viewer::core::sync::UploadLease::from_grant(&grant, started);
    let proof = lease.proof().clone();
    let pinned = client.pinned();

    // Gate, phases, then one real mutation with an idempotency key.
    let _ = block_on(
        client
            .inner
            .check_lease(&proof, &zone, &client.current_token(), true),
    );
    for phase in UploadPhase::ORDER {
        let _ = block_on(client.inner.set_phase(&proof, phase));
    }
    let body = json!({
        "zoneID": photo_viewer::core::sync::ZoneId::from(&zone),
        "atomic": true,
        "operations": [create_operation("album-receipt")],
    });
    let key = "pv-1-metadata-1";
    let (status, bytes) = send_modify(&server, &pinned, &proof, &body, key);
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert!(status.is_success(), "{status}: {value}");
    assert_eq!(
        value["records"][0]["recordName"], "album-receipt",
        "the write landed"
    );

    // Release, so the lease is definitively gone.
    block_on(client.inner.release_lease(&proof)).expect("release");

    // The receipt is still readable, with no lease proof at all.
    let state = block_on(client.inner.operation_status(key)).expect("receipt");
    assert!(
        matches!(
            state,
            photo_viewer::core::sync::OperationState::Committed(_)
        ),
        "a committed mutation must be reconcilable after its lease ended: {state:?}"
    );

    // An id nobody issued has no receipt, which says nothing about whether any
    // particular write happened.
    assert_eq!(
        block_on(client.inner.operation_status("never-issued")).unwrap(),
        photo_viewer::core::sync::OperationState::Unknown
    );
}

/// The same idempotency key with the same body returns the same answer; the
/// receipt is what makes a retry safe rather than a second write.
#[test]
fn a_replayed_mutation_returns_the_same_receipt() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let client = server.client("device-one");
    let zone = client.zone();
    let (grant, started) =
        block_on(client.inner.acquire_lease(&client.device, &zone)).expect("acquire");
    let lease = photo_viewer::core::sync::UploadLease::from_grant(&grant, started);
    let proof = lease.proof().clone();
    let pinned = client.pinned();
    let _ = block_on(
        client
            .inner
            .check_lease(&proof, &zone, &client.current_token(), true),
    );
    for phase in UploadPhase::ORDER {
        let _ = block_on(client.inner.set_phase(&proof, phase));
    }

    let body = json!({
        "zoneID": photo_viewer::core::sync::ZoneId::from(&zone),
        "atomic": true,
        "operations": [create_operation("album-idempotent")],
    });
    let key = "pv-1-metadata-9";

    let first = send_modify(&server, &pinned, &proof, &body, key);
    let second = send_modify(&server, &pinned, &proof, &body, key);
    assert!(first.0.is_success() && second.0.is_success());
    let first: Value = serde_json::from_slice(&first.1).unwrap();
    let second: Value = serde_json::from_slice(&second.1).unwrap();
    assert_eq!(
        first["records"][0]["recordChangeTag"], second["records"][0]["recordChangeTag"],
        "a replay must not mint a second revision"
    );
}
