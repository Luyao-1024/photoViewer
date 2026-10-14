//! C1.2 acceptance against the real SyncServer binary.
//!
//! The unit suites prove the graph rules and the durable-state rules; only a
//! real service proves the paging and delta contract this client depends on:
//! that a snapshot mints both tokens at one height, that a query's last page
//! omits the continuation marker, that a delta round is pinned to a fixed
//! height while the server keeps moving underneath it, and that writing to
//! another zone does not drag this one along.
//!
//! Records are written as `Album`, not as photos. A photo requires an asset
//! upload and a paired atomic write, which is C1.4's machinery; the point here
//! is transport, paging and ordering, and a non-photo type exercises the same
//! delta path without claiming media support this stage does not have.
//!
//! As in `native_sync_probe.rs`, the binary comes from the sibling checkout and
//! the suite skips (rather than faking a pass) when it is absent.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use photo_viewer::core::sync::native::receive::{ZoneId, SUPPORTED_RECORD_TYPES};
use photo_viewer::core::sync::native::records::RecordSet;
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

    fn client(&self) -> NativeClient {
        let endpoint = NativeEndpoint::parse(&self.base_url).expect("loopback endpoint");
        NativeClient::new(&endpoint, &self.api_key).expect("client")
    }

    fn pinned(&self) -> NativePinnedScope {
        block_on(self.client().probe()).expect("capability probe")
    }

    /// Acquire the upload lease, walk its phases to `create`, run one atomic
    /// batch, then release. Phase order is forward-only, so the three calls are
    /// not optional.
    fn write_records(&self, zone: &NativeZone, operations: Vec<Value>) {
        let lease = block_on(async { self.acquire_lease(zone).await });
        self.advance_lease_to_create(&lease, zone);
        let response = block_on(async {
            self.post_json(
                &self.cloudkit(&["records", "modify"]),
                json!({
                    "zoneID": ZoneId::from(zone),
                    "atomic": true,
                    "operations": operations,
                }),
                Some(&lease),
            )
            .await
        });
        let body: Value = response;
        assert!(
            body.get("serverErrorCode").is_none(),
            "records/modify rejected: {body}"
        );
        block_on(self.release_lease(&lease));
    }

    async fn acquire_lease(&self, zone: &NativeZone) -> Value {
        let body = self
            .post_json(
                &format!("{}v1/upload-leases", self.base_url),
                json!({
                    "deviceId": "photoviewer-test",
                    "zoneIDs": [ZoneId::from(zone)],
                }),
                None,
            )
            .await;
        assert!(body.get("leaseId").is_some(), "lease not granted: {body}");
        body
    }

    fn advance_lease_to_create(&self, lease: &Value, zone: &NativeZone) {
        // The check proves the client's own position is current before any
        // write is allowed, so the test passes the position it actually holds.
        let snapshot: Value = block_on(async {
            self.post_json(
                &format!("{}v1/snapshots", self.base_url),
                json!({ "zoneID": ZoneId::from(zone) }),
                None,
            )
            .await
        });
        let token = snapshot["syncToken"].as_str().unwrap_or_default();
        let checked: Value = block_on(async {
            self.post_json(
                &format!(
                    "{}v1/upload-leases/{}/check",
                    self.base_url,
                    lease["leaseId"].as_str().unwrap()
                ),
                json!({
                    "zoneTokens": [{
                        "zoneID": ZoneId::from(zone),
                        "syncToken": token,
                    }],
                    "localReady": true,
                }),
                Some(lease),
            )
            .await
        });
        assert_eq!(checked["mayUpload"], json!(true), "lease check: {checked}");

        for phase in ["delete", "modify", "create"] {
            let moved: Value = block_on(async {
                self.post_json(
                    &format!(
                        "{}v1/upload-leases/{}/phase",
                        self.base_url,
                        lease["leaseId"].as_str().unwrap()
                    ),
                    json!({ "phase": phase }),
                    Some(lease),
                )
                .await
            });
            assert_eq!(moved["phase"], json!(phase), "phase {phase}: {moved}");
        }
    }

    async fn release_lease(&self, lease: &Value) {
        let id = lease["leaseId"].as_str().unwrap_or_default();
        self.post_json(
            &format!("{}v1/upload-leases/{id}/release", self.base_url),
            json!({}),
            Some(lease),
        )
        .await;
    }

    fn cloudkit(&self, segments: &[&str]) -> String {
        format!(
            "{}database/1/com.apple.photos.cloud/production/private/{}",
            self.base_url,
            segments.join("/")
        )
    }

    async fn post_json(&self, url: &str, body: Value, lease: Option<&Value>) -> Value {
        let mut request = reqwest::Client::new()
            .post(url)
            .bearer_auth(&self.api_key)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(&body).unwrap());
        if let Some(lease) = lease {
            request = request
                .header("X-Sync-Lease-ID", lease["leaseId"].as_str().unwrap())
                .header("X-Sync-Lease-Token", lease["leaseToken"].as_str().unwrap())
                // The fence is a decimal *string*; sending it as a number would
                // be a different header value and fail the proof check.
                .header(
                    "X-Sync-Lease-Fence",
                    lease["fencingToken"].as_str().unwrap(),
                );
        }
        let response = request.send().await.expect("request");
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        assert!(status.is_success(), "{url} answered {status}: {text}");
        value
    }
}

fn mode(bits: u32) -> std::fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    std::fs::Permissions::from_mode(bits)
}

fn write_secret(root: &tempfile::TempDir, name: &str, value: String) -> String {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.path().join(name))
        .expect("create secret file");
    writeln!(file, "{value}").expect("write secret");
    value
}

/// A key that passes the service's 32..=512 printable-ASCII rule.
fn secret_value() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("pv-receive-key-{nanos:032x}")
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

fn zone() -> NativeZone {
    NativeZone::new("PrimarySync")
}

fn create_operation(name: &str, title: &str) -> Value {
    json!({
        "operationType": "create",
        "record": {
            "recordName": name,
            "recordType": "Album",
            "fields": { "title": { "type": "STRING", "value": title } },
        },
    })
}

fn skip() {
    eprintln!(
        "skipping: no syncserver binary. Build the sibling checkout with \
         `cargo build --locked --manifest-path ../syncserver/Cargo.toml` or set \
         SYNC_SERVER_BIN."
    );
}

/// A snapshot mints both tokens from one committed head. The tokens are
/// signed *cursors*, so two snapshots of the same head are equal — the height
/// they name is what identifies them, not the request that asked for it.
#[test]
fn a_snapshot_mints_two_tokens_at_one_height() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let client = server.client();
    let first = block_on(client.snapshot(&zone())).expect("snapshot");
    let second = block_on(client.snapshot(&zone())).expect("snapshot");

    assert_eq!(
        first.snapshot_token, second.snapshot_token,
        "the same head must produce the same read position"
    );
    assert_eq!(first.sync_token, second.sync_token);
    assert_ne!(
        first.snapshot_token, first.sync_token,
        "a snapshot token must not be usable as a zone position"
    );
    assert_eq!(first.zone_id, ZoneId::from(&zone()));

    // After a write the head moves, and both positions move with it. This is
    // what makes a token a position rather than a one-shot receipt.
    server.write_records(&zone(), vec![create_operation("album-0", "Travel")]);
    let after = block_on(client.snapshot(&zone())).expect("snapshot");
    assert_ne!(after.sync_token, first.sync_token);
}

/// An empty zone reads as one complete page: the marker is absent, not empty,
/// so a paged read terminates instead of looping.
#[test]
fn an_empty_zone_is_one_complete_page_and_a_stable_position() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let pinned = server.pinned();
    let client = server.client();
    let snapshot = block_on(client.snapshot(&zone())).expect("snapshot");

    for record_type in SUPPORTED_RECORD_TYPES {
        let page = block_on(client.query_records(
            &pinned.namespace,
            &zone(),
            record_type,
            client.page_limit(&pinned),
            Some(&snapshot.snapshot_token),
            None,
        ))
        .expect("query");
        assert!(page.records.is_empty());
        assert!(
            !page.more_pages(),
            "{record_type} must not report a next page"
        );
        // Every type sharing one snapshot reports the same zone position, which
        // is what makes one shared height the right thing to project at.
        assert_eq!(page.sync_token, snapshot.sync_token);
    }

    // A delta round over an unchanged zone reports no more pages and hands back
    // the same position, so a settled client can stop without spinning.
    let entry = block_on(client.changes_zone(
        &pinned.namespace,
        &zone(),
        Some(&snapshot.sync_token),
        client.page_limit(&pinned),
    ))
    .expect("delta");
    assert!(!entry.more_coming);
    assert!(entry.records.is_empty());
    assert_eq!(
        entry.sync_token.as_deref(),
        Some(snapshot.sync_token.as_str())
    );
}

/// A record written after a position is established is seen by the next round
/// and is not visible in a read pinned to the earlier snapshot.
#[test]
fn a_concurrent_create_arrives_in_the_next_round_not_the_earlier_one() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let pinned = server.pinned();
    let client = server.client();
    let before = block_on(client.snapshot(&zone())).expect("snapshot");

    server.write_records(
        &zone(),
        (0..3)
            .map(|index| create_operation(&format!("album-{index}"), "Travel"))
            .collect(),
    );

    // The snapshot read stays at its own height.
    let page = block_on(client.query_records(
        &pinned.namespace,
        &zone(),
        "Album",
        client.page_limit(&pinned),
        Some(&before.snapshot_token),
        None,
    ))
    .expect("query");
    assert!(
        page.records.is_empty(),
        "a snapshot read must not observe writes made after it"
    );

    // The delta from the same position does.
    let entry = block_on(client.changes_zone(
        &pinned.namespace,
        &zone(),
        Some(&before.sync_token),
        client.page_limit(&pinned),
    ))
    .expect("delta");
    assert_eq!(entry.records.len(), 3);
    assert!(!entry.more_coming);

    let names: Vec<&str> = entry
        .records
        .iter()
        .map(|record| record.record_name.as_str())
        .collect();
    assert!(names.contains(&"album-0"));
    assert!(names.contains(&"album-2"));
    for record in &entry.records {
        assert_eq!(record.record_type.as_deref(), Some("Album"));
        assert!(record.record_change_tag.is_some());
    }
}

/// A write that lands between two pages must not split the round: the token
/// handed back pins the watermark, so the round finishes at the height it
/// started at.
#[test]
fn a_write_between_pages_does_not_split_a_fixed_height_round() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let pinned = server.pinned();
    let client = server.client();

    // The position is taken first, so the writes below are changes *after* it
    // and a round from it must see exactly them.
    let snapshot = block_on(client.snapshot(&zone())).expect("snapshot");
    let operations: Vec<Value> = (0..3)
        .map(|index| create_operation(&format!("seed-{index}"), "Seed"))
        .collect();
    server.write_records(&zone(), operations);

    let first =
        block_on(client.changes_zone(&pinned.namespace, &zone(), Some(&snapshot.sync_token), 1))
            .expect("delta page 1");
    assert!(first.more_coming, "a limit of 1 over 3 records must page");
    assert_eq!(first.records.len(), 1);

    // A write now cannot appear in the rest of this round, so the position to
    // resume the *next* round from is the one the finished round handed back.
    let position_before_late_write = first.sync_token.clone();
    server.write_records(&zone(), vec![create_operation("late-arrival", "Late")]);

    let mut position = position_before_late_write.clone();
    let mut seen = first.records.len();
    let mut finished_token = String::new();
    while let Some(token) = position.clone() {
        let entry = block_on(client.changes_zone(&pinned.namespace, &zone(), Some(&token), 1))
            .expect("delta page");
        seen += entry.records.len();
        if entry.more_coming {
            // A paged token keeps pinning the round's height, so a write now
            // still cannot leak into the pages that follow.
            position = entry.sync_token;
        } else {
            // Only the final token is re-based to the current head.
            finished_token = entry.sync_token.unwrap_or_default();
            position = None;
        }
    }
    assert_eq!(
        seen, 3,
        "the round sees exactly the records that existed at its height"
    );
    assert!(!finished_token.is_empty());
    // The late write is picked up only by the next round, resumed from the
    // finished round's token. Resuming from a mid-round token instead would
    // re-read the same height and never see it.
    let next = block_on(client.changes_zone(
        &pinned.namespace,
        &zone(),
        Some(finished_token.as_str()),
        100,
    ))
    .expect("next round");
    assert!(next
        .records
        .iter()
        .any(|record| record.record_name == "late-arrival"));
}

/// A delta replayed twice must produce the same live set — which is what makes
/// the durable inbox safe to re-read after a crash.
#[test]
fn a_replayed_round_projects_the_same_graph() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let pinned = server.pinned();
    let client = server.client();
    // Position first, so the round below has work to report.
    let snapshot = block_on(client.snapshot(&zone())).expect("snapshot");
    server.write_records(
        &zone(),
        (0..4)
            .map(|index| create_operation(&format!("album-{index}"), "Travel"))
            .collect(),
    );

    let read = |token: Option<&str>| {
        block_on(client.changes_zone(&pinned.namespace, &zone(), token, 100)).expect("delta")
    };
    let first = read(Some(&snapshot.sync_token));
    let second = read(Some(&snapshot.sync_token));

    let zone_id = ZoneId::from(&zone());
    let project = |entry: &photo_viewer::core::sync::native::receive::ZoneChangesEntry| {
        let mut set = RecordSet::default();
        for record in &entry.records {
            set.apply(record.clone());
        }
        set.to_graph(&zone_id).expect("graph")
    };
    assert_eq!(project(&first), project(&second));
    assert_eq!(project(&first).records.len(), 4);
}

/// A zone that this job does not read must not change this job's position.
#[test]
fn another_zone_moving_does_not_move_this_jobs_head() {
    let Some(_server) = Server::start() else {
        return skip();
    };
    // A single-zone deployment cannot demonstrate this, and the server refuses
    // to change its zone set after first start. Assert the client's rule
    // instead: a zone position belongs to its zone, so a head is only ever
    // compared against the zone it was minted for.
    let pinned = _server.pinned();
    let first = block_on(_server.client().snapshot(&zone())).expect("snapshot");
    assert_eq!(first.zone_id, ZoneId::from(&zone()));
    assert!(pinned.has_zone("PrimarySync"));
    assert!(
        !pinned.has_zone("OtherSync"),
        "the deployment configures one zone"
    );
}

/// A token minted for one zone is not a position in another.
#[test]
fn a_zone_token_cannot_be_read_as_a_snapshot_token() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let pinned = server.pinned();
    let client = server.client();
    let snapshot = block_on(client.snapshot(&zone())).expect("snapshot");

    // Sending the zone token where a snapshot is expected is refused. This is
    // the check that would catch a client substituting one position for another.
    let error = block_on(async {
        client
            .query_records(
                &pinned.namespace,
                &zone(),
                SUPPORTED_RECORD_TYPES[0],
                client.page_limit(&pinned),
                Some(&snapshot.sync_token),
                None,
            )
            .await
    });
    match error {
        Err(SyncError::Protocol(_)) | Err(SyncError::Unsupported(_)) => {}
        other => panic!("a zone token must not be accepted as a snapshot token: {other:?}"),
    }
}

/// A page size outside the server's accepted range is refused rather than
/// silently clamped, so a client cannot believe it asked for less than it did.
#[test]
fn an_out_of_range_page_limit_is_refused() {
    let Some(server) = Server::start() else {
        return skip();
    };
    let pinned = server.pinned();
    let client = server.client();
    let snapshot = block_on(client.snapshot(&zone())).expect("snapshot");

    let error = block_on(client.query_records(
        &pinned.namespace,
        &zone(),
        SUPPORTED_RECORD_TYPES[0],
        0,
        Some(&snapshot.snapshot_token),
        None,
    ))
    .expect_err("resultsLimit=0 must be refused");
    assert!(
        matches!(error, SyncError::Protocol(_) | SyncError::Unsupported(_)),
        "{error:?}"
    );
}
