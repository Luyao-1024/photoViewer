use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;

#[test]
fn parses_namespaced_multistatus_and_decodes_paths() {
    let xml = br#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:">
      <d:response><d:href>/dav/root/%E7%9B%B8%E5%86%8C/a%20b.jpg</d:href>
      <d:propstat><d:prop><d:getcontentlength>42</d:getcontentlength>
      <d:getetag>&quot;abc&quot;</d:getetag><d:resourcetype/></d:prop>
      <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
    </d:multistatus>"#;
    let base = Url::parse("https://example.com/dav/root/").unwrap();
    let entries = parse_multistatus(xml, &base).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].key, "相册/a b.jpg");
    assert_eq!(entries[0].size, 42);
    assert_eq!(entries[0].revision.as_ref().unwrap().value, "\"abc\"");
}

#[test]
fn keeps_collection_when_a_later_propstat_is_not_found() {
    let xml = br#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:">
      <D:response><D:href>/dav/root/%E6%9C%AC%E5%9C%B0/</D:href>
      <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype>
      <D:getlastmodified>Sat, 19 Sep 2026 06:34:11 GMT</D:getlastmodified></D:prop>
      <D:status>HTTP/1.1 200 OK</D:status></D:propstat>
      <D:propstat><D:prop><D:getcontentlength></D:getcontentlength>
      <D:getetag></D:getetag></D:prop>
      <D:status>HTTP/1.1 404 Not Found</D:status></D:propstat></D:response>
    </D:multistatus>"#;
    let base = Url::parse("https://example.com/dav/root/").unwrap();
    let entries = parse_multistatus(xml, &base).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].key, "本地");
    assert!(entries[0].is_collection);
    assert_eq!(entries[0].size, 0);
    assert!(entries[0].revision.is_none());
}

#[test]
fn rejects_cross_origin_and_root_escape_hrefs() {
    let base = Url::parse("https://example.com/dav/root/").unwrap();
    assert!(href_to_key(&base, "https://evil.example/a.jpg").is_err());
    assert!(href_to_key(&base, "/dav/other/a.jpg").is_err());
}

#[test]
fn encodes_remote_key_segments_without_losing_special_characters() {
    let provider = WebDavProvider::new(
        "https://example.com/dav/root/",
        "user".into(),
        "secret".into(),
    )
    .unwrap();
    let url = provider.object_url("相册/a #?.jpg", false).unwrap();
    assert_eq!(
        url.as_str(),
        "https://example.com/dav/root/%E7%9B%B8%E5%86%8C/a%20%23%3F.jpg"
    );
}

fn mock_server(responses: Vec<Vec<u8>>) -> (String, mpsc::Receiver<Vec<u8>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let read = stream.read(&mut buffer).unwrap_or(0);
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                let Some(header_end) = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|index| index + 4)
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':').and_then(|(name, value)| {
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                    })
                    .unwrap_or(0);
                if request.len() >= header_end + length {
                    break;
                }
            }
            let _ = sender.send(request);
            stream.write_all(&response).unwrap();
        }
    });
    (format!("http://localhost:{port}/"), receiver)
}

fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

#[tokio::test]
async fn sends_create_precondition_and_verifies_uploaded_object() {
    let multistatus = br#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:">
      <d:response><d:href>/a%20b.jpg</d:href><d:propstat><d:prop>
      <d:getcontentlength>3</d:getcontentlength><d:getetag>&quot;new&quot;</d:getetag>
      <d:resourcetype/></d:prop><d:status>HTTP/1.1 200 OK</d:status>
      </d:propstat></d:response></d:multistatus>"#;
    let (endpoint, requests) = mock_server(vec![
        response("405 Method Not Allowed", "", b""),
        response("201 Created", "ETag: \"new\"\r\n", b""),
        response(
            "207 Multi-Status",
            "Content-Type: application/xml\r\n",
            multistatus,
        ),
    ]);
    let provider = WebDavProvider::new(&endpoint, "alice".into(), "secret".into()).unwrap();
    let temp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(temp.path(), b"abc").unwrap();

    let uploaded = provider
        .upload("a b.jpg", temp.path(), WriteCondition::CreateOnly)
        .await
        .unwrap();
    assert_eq!(uploaded.revision.unwrap().value, "\"new\"");
    let lock = String::from_utf8_lossy(&requests.recv().unwrap()).to_ascii_lowercase();
    assert!(lock.starts_with("lock /a%20b.jpg http/1.1"));
    let put = String::from_utf8_lossy(&requests.recv().unwrap()).to_ascii_lowercase();
    assert!(put.starts_with("put /a%20b.jpg http/1.1"));
    assert!(put.contains("if-none-match: *"));
    assert!(put.contains("authorization: basic "));
    let stat = String::from_utf8_lossy(&requests.recv().unwrap()).to_ascii_lowercase();
    assert!(stat.starts_with("propfind /a%20b.jpg/ http/1.1"));
    assert!(stat.contains("depth: 0"));
}

#[tokio::test]
async fn lock_null_resource_makes_create_only_atomic_without_if_none_match() {
    let multistatus = br#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:">
      <d:response><d:href>/new.jpg</d:href><d:propstat><d:prop>
      <d:getcontentlength>3</d:getcontentlength><d:getetag>&quot;new&quot;</d:getetag>
      <d:resourcetype/></d:prop><d:status>HTTP/1.1 200 OK</d:status>
      </d:propstat></d:response></d:multistatus>"#;
    let (endpoint, requests) = mock_server(vec![
        response("201 Created", "Lock-Token: <token-1>\r\n", b""),
        response("201 Created", "ETag: \"new\"\r\n", b""),
        response(
            "207 Multi-Status",
            "Content-Type: application/xml\r\n",
            multistatus,
        ),
        response("204 No Content", "", b""),
    ]);
    let provider = WebDavProvider::new(&endpoint, "alice".into(), "secret".into()).unwrap();
    let temp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(temp.path(), b"abc").unwrap();

    provider
        .upload("new.jpg", temp.path(), WriteCondition::CreateOnly)
        .await
        .unwrap();

    let lock = String::from_utf8_lossy(&requests.recv().unwrap()).to_ascii_lowercase();
    assert!(lock.starts_with("lock /new.jpg http/1.1"));
    let put = String::from_utf8_lossy(&requests.recv().unwrap()).to_ascii_lowercase();
    assert!(put.contains("if: (<token-1>)"));
    assert!(!put.contains("if-none-match:"));
    let stat = String::from_utf8_lossy(&requests.recv().unwrap()).to_ascii_lowercase();
    assert!(stat.starts_with("propfind /new.jpg/ http/1.1"));
    let unlock = String::from_utf8_lossy(&requests.recv().unwrap()).to_ascii_lowercase();
    assert!(unlock.starts_with("unlock /new.jpg http/1.1"));
    assert!(unlock.contains("lock-token: <token-1>"));
}

#[tokio::test]
async fn existing_locked_resource_rejects_create_only_before_put() {
    let (endpoint, requests) = mock_server(vec![
        response("200 OK", "Lock-Token: <token-2>\r\n", b""),
        response("204 No Content", "", b""),
    ]);
    let provider = WebDavProvider::new(&endpoint, "alice".into(), "secret".into()).unwrap();
    let temp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(temp.path(), b"abc").unwrap();

    let error = provider
        .upload("existing.jpg", temp.path(), WriteCondition::CreateOnly)
        .await
        .unwrap_err();
    assert!(matches!(error, ProviderError::PreconditionFailed(_)));
    let lock = String::from_utf8_lossy(&requests.recv().unwrap()).to_ascii_lowercase();
    assert!(lock.starts_with("lock /existing.jpg http/1.1"));
    let unlock = String::from_utf8_lossy(&requests.recv().unwrap()).to_ascii_lowercase();
    assert!(unlock.starts_with("unlock /existing.jpg http/1.1"));
    assert!(requests.try_recv().is_err());
}

#[tokio::test]
async fn classifies_common_webdav_failures() {
    for (status, expected) in [
        ("401 Unauthorized", "authentication"),
        ("403 Forbidden", "permission"),
        ("429 Too Many Requests", "rate"),
        ("507 Insufficient Storage", "storage"),
    ] {
        let (endpoint, _) = mock_server(vec![response(status, "", b"")]);
        let provider = WebDavProvider::new(&endpoint, "alice".into(), "secret".into()).unwrap();
        let error = provider.probe().await.unwrap_err();
        match (expected, error) {
            ("authentication", ProviderError::Authentication)
            | ("permission", ProviderError::Permission)
            | ("rate", ProviderError::RateLimited)
            | ("storage", ProviderError::StorageFull) => {}
            (_, error) => panic!("unexpected error for {status}: {error}"),
        }
    }

    let (endpoint, requests) = mock_server(vec![
        response("405 Method Not Allowed", "", b""),
        response("412 Precondition Failed", "", b""),
    ]);
    let provider = WebDavProvider::new(&endpoint, "alice".into(), "secret".into()).unwrap();
    let temp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(temp.path(), b"abc").unwrap();
    let error = provider
        .upload(
            "a.jpg",
            temp.path(),
            WriteCondition::ReplaceIf(Revision::etag("\"old\"")),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ProviderError::PreconditionFailed(_)));
    let lock = String::from_utf8_lossy(&requests.recv().unwrap()).to_ascii_lowercase();
    assert!(lock.starts_with("lock /a.jpg http/1.1"));
    let put = String::from_utf8_lossy(&requests.recv().unwrap()).to_ascii_lowercase();
    assert!(put.contains("if-match: \"old\""));
}

#[tokio::test]
async fn rejects_truncated_downloads() {
    let mut truncated =
        b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 10\r\nETag: \"v1\"\r\n\r\nabc"
            .to_vec();
    // Keep the raw response intentionally shorter than Content-Length.
    truncated.shrink_to_fit();
    let (endpoint, _) = mock_server(vec![truncated]);
    let provider = WebDavProvider::new(&endpoint, "alice".into(), "secret".into()).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("partial.jpg");
    let error = provider
        .download("a.jpg", Some(&Revision::etag("\"v1\"")), &destination)
        .await
        .unwrap_err();
    assert!(matches!(error, ProviderError::Protocol(_)));
}

#[tokio::test]
async fn rejects_successful_download_with_a_different_response_etag() {
    let (endpoint, requests) =
        mock_server(vec![response("200 OK", "ETag: \"v2\"\r\n", b"changed")]);
    let provider = WebDavProvider::new(&endpoint, "alice".into(), "secret".into()).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("changed.jpg");
    let error = provider
        .download("a.jpg", Some(&Revision::etag("\"v1\"")), &destination)
        .await
        .unwrap_err();
    assert!(matches!(error, ProviderError::PreconditionFailed(_)));
    assert!(!destination.exists());
    let get = String::from_utf8_lossy(&requests.recv().unwrap()).to_ascii_lowercase();
    assert!(get.contains("if-match: \"v1\""));
}
