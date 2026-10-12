use super::*;

fn fixture(temp: &tempfile::TempDir, selected: bool) -> (SyncService, SyncJob, Arc<FakeProvider>) {
    let local_root = temp.path().join("photos");
    std::fs::create_dir(&local_root).unwrap();
    std::fs::write(local_root.join("a.jpg"), b"AAAA").unwrap();
    let pool = crate::core::db::init_pool(&temp.path().join("photos.db")).unwrap();
    let (sender, _) = crate::core::events::DomainEventSender::new();
    let actor = crate::core::db_actor::start_db_actor(pool.clone(), sender);
    let service = SyncService {
        store: SyncStore::with_actor(pool.clone(), actor.clone()),
        pool,
        actor: Some(actor),
        staging_root: temp.path().join("staging"),
    };
    let job = service
        .store
        .create_job(&super::super::super::store::NewSyncJob {
            endpoint: "https://dav.example.test/".into(),
            username: "hash-test".into(),
            credential_ref: "hash-test".into(),
            etag_is_sha256: true,
            local_root,
            remote_root: "Photos".into(),
            direction: SyncDirection::Bidirectional,
            upload_scope: if selected {
                UploadScope::All
            } else {
                UploadScope::SelectedAlbums
            },
            upload_albums: vec![],
        })
        .unwrap();
    let provider = Arc::new(FakeProvider::default());
    provider.provide_sha256.store(true, Ordering::Relaxed);
    (service, job, provider)
}

#[tokio::test]
async fn sha256_matching_content_establishes_baseline_without_download() {
    let temp = tempfile::tempdir().unwrap();
    let (service, job, provider) = fixture(&temp, true);
    provider.insert("Photos/a.jpg", b"AAAA".to_vec());
    let summary = service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert_eq!(summary.verified, 1);
    assert_eq!(provider.download_calls.load(Ordering::Relaxed), 0);
    assert_eq!(provider.upload_calls.load(Ordering::Relaxed), 0);
    let entry = service.store.entries(job.id).unwrap().pop().unwrap();
    assert_eq!(entry.baseline, entry.local);
    assert_eq!(entry.baseline, entry.remote);
    assert_eq!(entry.state, "synced");
    assert!(!service.staging_root.exists());
    let again = service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert_eq!(again.unchanged, 1);
    assert_eq!(provider.download_calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn sha256_same_size_different_content_preserves_both_and_records_conflict() {
    let temp = tempfile::tempdir().unwrap();
    let (service, job, provider) = fixture(&temp, true);
    provider.insert("Photos/a.jpg", b"BBBB".to_vec());
    let summary = service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert_eq!(summary.conflicts, 1);
    assert_eq!(summary.verified, 0);
    assert_eq!(provider.download_calls.load(Ordering::Relaxed), 0);
    assert_eq!(provider.upload_calls.load(Ordering::Relaxed), 0);
    assert_eq!(
        std::fs::read(job.local_root.join("a.jpg")).unwrap(),
        b"AAAA"
    );
    assert_eq!(provider.objects.lock().unwrap()["Photos/a.jpg"], b"BBBB");
    assert!(service.store.entries(job.id).unwrap()[0].baseline.is_none());
}

#[tokio::test]
async fn sha256_unselected_mismatch_downloads_only_the_required_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let (service, job, provider) = fixture(&temp, false);
    provider.insert("Photos/a.jpg", b"BBBB".to_vec());
    let summary = service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert_eq!(summary.downloaded, 1);
    assert_eq!(provider.download_calls.load(Ordering::Relaxed), 1);
    assert_eq!(provider.upload_calls.load(Ordering::Relaxed), 0);
    assert_eq!(
        std::fs::read(job.local_root.join("a.jpg")).unwrap(),
        b"BBBB"
    );
}

#[tokio::test]
async fn sha256_generic_provider_retains_full_content_verification() {
    let temp = tempfile::tempdir().unwrap();
    let (service, job, provider) = fixture(&temp, true);
    provider.provide_sha256.store(false, Ordering::Relaxed);
    provider.insert("Photos/a.jpg", b"AAAA".to_vec());
    let summary = service
        .run_with_provider(&job, provider.clone())
        .await
        .unwrap();
    assert_eq!(summary.verified, 1);
    assert_eq!(provider.download_calls.load(Ordering::Relaxed), 1);
}

#[test]
fn sha256_rejects_a_local_file_changed_after_scan_even_with_restored_mtime() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("a.jpg");
    std::fs::write(&path, b"AAAA").unwrap();
    let entry = local::scan(temp.path()).unwrap().remove("a.jpg").unwrap();
    let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::fs::write(&path, b"BBBB").unwrap();
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(mtime))
        .unwrap();
    assert!(local::content_sha256(&entry)
        .unwrap_err()
        .to_string()
        .contains("file changed"));
}

#[test]
fn sha256_configuration_coalesces_and_global_reset_keeps_the_latest_choice() {
    let temp = tempfile::tempdir().unwrap();
    let (service, job, _) = fixture(&temp, true);
    let store = &service.store;
    assert!(!store
        .queue_edit(
            job.id,
            super::super::super::store::JobEdit::EtagIsSha256(true)
        )
        .unwrap());
    assert!(store
        .queue_edit(
            job.id,
            super::super::super::store::JobEdit::EtagIsSha256(false)
        )
        .unwrap());
    assert!(!store.desired_job(job.id).unwrap().unwrap().etag_is_sha256);
    assert!(store.get_job(job.id).unwrap().unwrap().etag_is_sha256);
    store.begin_reset().unwrap();
    let token = store.reset_intent().unwrap().unwrap().token;
    assert!(store.finish_reset(&token).unwrap());
    assert!(!store.get_job(job.id).unwrap().unwrap().etag_is_sha256);
    assert!(store.pending_change(job.id).unwrap().is_none());
}

#[tokio::test]
async fn sha256_webdav_verifies_large_existing_file_using_only_metadata_requests() {
    use crate::core::sync::{NewSyncJob, SyncDirection, SyncService, UploadScope};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("photos");
    std::fs::create_dir(&root).unwrap();
    let path = root.join("large.mp4");
    std::fs::File::create(&path)
        .unwrap()
        .set_len(256 * 1024 * 1024 + 1)
        .unwrap();
    let entry = local::scan(&root).unwrap().remove("large.mp4").unwrap();
    let sha256 = local::content_sha256(&entry).unwrap();
    let xml = format!(
        r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/Photos/large.mp4</d:href>
        <d:propstat><d:prop><d:getcontentlength>{}</d:getcontentlength><d:getetag>&quot;{}&quot;</d:getetag>
        <d:resourcetype/></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#,
        entry.fingerprint.size, sha256
    );
    let (endpoint, requests) = metadata_mock_server(vec![
        http_response("200 OK", "", b""),
        http_response("405 Method Not Allowed", "", b""),
        http_response(
            "207 Multi-Status",
            "Content-Type: application/xml\r\n",
            xml.as_bytes(),
        ),
    ]);
    let pool = crate::core::db::init_pool(&temp.path().join("library.db")).unwrap();
    let (sender, _) = crate::core::events::DomainEventSender::new();
    let actor = crate::core::db_actor::start_db_actor(pool.clone(), sender);
    let service = SyncService::with_actor(pool, actor);
    let job = service
        .store()
        .create_job(&NewSyncJob {
            endpoint: endpoint.clone(),
            username: "test".into(),
            credential_ref: "metadata-test".into(),
            etag_is_sha256: true,
            local_root: root,
            remote_root: "Photos".into(),
            direction: SyncDirection::Bidirectional,
            upload_scope: UploadScope::All,
            upload_albums: vec![],
        })
        .unwrap();
    let provider = std::sync::Arc::new(
        WebDavProvider::new(&endpoint, "test".into(), "test".into())
            .unwrap()
            .with_sha256_etags(true),
    );
    let summary = service.run_with_provider(&job, provider).await.unwrap();
    assert_eq!(summary.verified, 1);
    assert_eq!(summary.downloaded, 0);
    assert_eq!(summary.conflicts, 0);
    let methods: Vec<_> = requests
        .iter()
        .map(|r| {
            String::from_utf8(r)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(methods, ["OPTIONS", "MKCOL", "PROPFIND"]);
    assert!(service.store().entries(job.id).unwrap()[0]
        .baseline
        .is_some());
}

fn http_response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

fn metadata_mock_server(responses: Vec<Vec<u8>>) -> (String, std::sync::mpsc::Receiver<Vec<u8>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for response in responses {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = vec![];
            let mut buffer = [0; 4096];
            loop {
                let read = socket.read(&mut buffer).unwrap();
                assert_ne!(read, 0);
                request.extend_from_slice(&buffer[..read]);
                if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            if name.eq_ignore_ascii_case("content-length") {
                                value.trim().parse::<usize>().ok()
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            sender.send(request).unwrap();
            socket.write_all(&response).unwrap();
        }
    });
    (format!("http://localhost:{port}/dav/"), receiver)
}

#[tokio::test]
async fn sha256_misconfigured_server_fails_before_any_download_or_baseline() {
    let temp = tempfile::tempdir().unwrap();
    let (service, job, _) = fixture(&temp, true);
    let xml = br#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/Photos/a.jpg</d:href>
        <d:propstat><d:prop><d:getcontentlength>4</d:getcontentlength><d:getetag>&quot;metadata-only&quot;</d:getetag>
        <d:resourcetype/></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
    let (endpoint, requests) = metadata_mock_server(vec![
        http_response("200 OK", "", b""),
        http_response("405 Method Not Allowed", "", b""),
        http_response("207 Multi-Status", "Content-Type: application/xml\r\n", xml),
    ]);
    let provider = Arc::new(
        WebDavProvider::new(&endpoint, "test".into(), "test".into())
            .unwrap()
            .with_sha256_etags(true),
    );
    let error = service
        .run_with_provider(&job, provider)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("--etag-hash sha256"));
    assert!(service.store.entries(job.id).unwrap().is_empty());
    assert!(!service.staging_root.exists());
    let methods: Vec<_> = requests
        .iter()
        .map(|r| {
            String::from_utf8(r)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(methods, ["OPTIONS", "MKCOL", "PROPFIND"]);
}
