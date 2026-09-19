use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use percent_encoding::percent_decode_str;
use quick_xml::events::Event;
use quick_xml::Reader;
use reqwest::header::{CONTENT_LENGTH, ETAG, IF_MATCH, IF_NONE_MATCH};
use reqwest::{Method, StatusCode, Url};
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

use super::model::{Revision, RevisionStrength};
use super::provider::{
    ProviderCapabilities, ProviderError, ProviderResult, RemoteEntry, SyncProvider, WriteCondition,
};

const PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="utf-8" ?>
<d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:getcontentlength/>
<d:getlastmodified/><d:getetag/></d:prop></d:propfind>"#;
const MAX_DAV_XML_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone)]
pub struct WebDavProvider {
    client: reqwest::Client,
    base: Url,
    username: String,
    password: String,
}

impl WebDavProvider {
    pub fn new(endpoint: &str, username: String, password: String) -> ProviderResult<Self> {
        let mut base = Url::parse(endpoint)
            .map_err(|error| ProviderError::Protocol(format!("invalid endpoint URL: {error}")))?;
        if base.scheme() != "https" && base.host_str() != Some("localhost") {
            return Err(ProviderError::Protocol(
                "WebDAV endpoint must use HTTPS (HTTP is allowed only for localhost tests)".into(),
            ));
        }
        if base.query().is_some() || base.fragment().is_some() {
            return Err(ProviderError::Protocol(
                "WebDAV endpoint must not contain a query or fragment".into(),
            ));
        }
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(15 * 60))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(format!("PhotoViewer/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| ProviderError::Protocol(error.to_string()))?;
        Ok(Self {
            client,
            base,
            username,
            password,
        })
    }

    fn request(&self, method: Method, url: Url) -> reqwest::RequestBuilder {
        self.client
            .request(method, url)
            .basic_auth(&self.username, Some(&self.password))
    }

    fn object_url(&self, key: &str, collection: bool) -> ProviderResult<Url> {
        validate_key(key)?;
        let mut url = self.base.clone();
        {
            let mut segments = url.path_segments_mut().map_err(|_| {
                ProviderError::Protocol("WebDAV endpoint cannot be used as a path base".into())
            })?;
            segments.pop_if_empty();
            for segment in key.split('/').filter(|segment| !segment.is_empty()) {
                segments.push(segment);
            }
            if collection {
                segments.push("");
            }
        }
        Ok(url)
    }

    async fn propfind(&self, key: &str, depth: &str) -> ProviderResult<Vec<RemoteEntry>> {
        let method = Method::from_bytes(b"PROPFIND")
            .map_err(|error| ProviderError::Protocol(error.to_string()))?;
        let url = self.object_url(key, true)?;
        let response = self
            .request(method, url)
            .header("Depth", depth)
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(PROPFIND_BODY)
            .send()
            .await
            .map_err(map_transport)?;
        let status = response.status();
        if status == StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        if status != StatusCode::MULTI_STATUS && !status.is_success() {
            return Err(map_status(status, key));
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_DAV_XML_BYTES as u64)
        {
            return Err(ProviderError::Protocol(
                "DAV directory response exceeds the 32 MiB safety limit".into(),
            ));
        }
        let mut response = response;
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(map_transport)? {
            if body.len().saturating_add(chunk.len()) > MAX_DAV_XML_BYTES {
                return Err(ProviderError::Protocol(
                    "DAV directory response exceeds the 32 MiB safety limit".into(),
                ));
            }
            body.extend_from_slice(&chunk);
        }
        parse_multistatus(&body, &self.base)
    }
}

#[async_trait]
impl SyncProvider for WebDavProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            read: true,
            write_new: true,
            conditional_replace: true,
            conditional_delete: true,
            move_object: false,
            range_read: false,
            incremental_listing: false,
        }
    }

    async fn probe(&self) -> ProviderResult<ProviderCapabilities> {
        let response = self
            .request(Method::OPTIONS, self.base.clone())
            .send()
            .await
            .map_err(map_transport)?;
        if !response.status().is_success() {
            return Err(map_status(response.status(), ""));
        }
        // OPTIONS is only an accessibility probe. Per-operation conditional
        // behavior is still checked by the executor and HTTP preconditions.
        Ok(self.capabilities())
    }

    async fn list_children(&self, collection: &str) -> ProviderResult<Vec<RemoteEntry>> {
        let collection_key = collection.trim_matches('/');
        let mut entries = self.propfind(collection_key, "1").await?;
        entries.retain(|entry| entry.key.trim_matches('/') != collection_key);
        Ok(entries)
    }

    async fn stat(&self, key: &str) -> ProviderResult<Option<RemoteEntry>> {
        let normalized = key.trim_matches('/');
        let entries = self.propfind(normalized, "0").await?;
        Ok(entries
            .into_iter()
            .find(|entry| entry.key.trim_matches('/') == normalized))
    }

    async fn ensure_collection(&self, collection: &str) -> ProviderResult<()> {
        let mut current = String::new();
        for segment in collection
            .trim_matches('/')
            .split('/')
            .filter(|segment| !segment.is_empty())
        {
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(segment);
            let method = Method::from_bytes(b"MKCOL")
                .map_err(|error| ProviderError::Protocol(error.to_string()))?;
            let response = self
                .request(method, self.object_url(&current, true)?)
                .send()
                .await
                .map_err(map_transport)?;
            if response.status().is_success()
                || response.status() == StatusCode::METHOD_NOT_ALLOWED
                || response.status() == StatusCode::CONFLICT && self.stat(&current).await?.is_some()
            {
                continue;
            }
            return Err(map_status(response.status(), &current));
        }
        Ok(())
    }

    async fn download(
        &self,
        key: &str,
        expected: Option<&Revision>,
        destination: &Path,
    ) -> ProviderResult<RemoteEntry> {
        let mut request = self.request(Method::GET, self.object_url(key, false)?);
        if let Some(expected) = expected {
            if expected.strength == RevisionStrength::Strong {
                request = request.header(IF_MATCH, &expected.value);
            }
        }
        let response = request.send().await.map_err(map_transport)?;
        if !response.status().is_success() {
            return Err(map_status(response.status(), key));
        }
        let headers = response.headers().clone();
        let actual_revision = header_revision(&headers);
        if expected.is_some_and(|expected| actual_revision.as_ref() != Some(expected)) {
            return Err(ProviderError::PreconditionFailed(key.into()));
        }
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .await?;
        let mut stream = response.bytes_stream();
        let mut size = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(map_transport)?;
            size = size.saturating_add(chunk.len() as u64);
            file.write_all(&chunk).await?;
        }
        file.sync_all().await?;
        if let Some(declared) = headers
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
        {
            if declared != size {
                return Err(ProviderError::Protocol(format!(
                    "downloaded length {size} differs from Content-Length {declared}"
                )));
            }
        }
        Ok(RemoteEntry {
            key: key.into(),
            is_collection: false,
            size,
            modified_unix: None,
            revision: actual_revision,
        })
    }

    async fn upload(
        &self,
        key: &str,
        source: &Path,
        condition: WriteCondition,
    ) -> ProviderResult<RemoteEntry> {
        if let Some(parent) = Path::new(key).parent().and_then(Path::to_str) {
            self.ensure_collection(parent).await?;
        }
        let file = tokio::fs::File::open(source).await?;
        let size = file.metadata().await?.len();
        let body = reqwest::Body::wrap_stream(ReaderStream::new(file));
        let mut request = self
            .request(Method::PUT, self.object_url(key, false)?)
            .header(CONTENT_LENGTH, size)
            .body(body);
        request = match &condition {
            WriteCondition::CreateOnly => request.header(IF_NONE_MATCH, "*"),
            WriteCondition::ReplaceIf(revision)
                if revision.strength == RevisionStrength::Strong =>
            {
                request.header(IF_MATCH, &revision.value)
            }
            WriteCondition::ReplaceIf(_) => {
                return Err(ProviderError::Unsupported(
                    "weak ETag cannot authorize automatic replacement".into(),
                ))
            }
        };
        let response = request.send().await.map_err(map_transport)?;
        if !response.status().is_success() {
            return Err(map_status(response.status(), key));
        }
        let revision = header_revision(response.headers());
        if let Some(mut actual) = self.stat(key).await? {
            if actual.size == 0 && size != 0 {
                actual.size = size;
            }
            if actual.revision.is_none() {
                actual.revision = revision;
            }
            return Ok(actual);
        }
        Err(ProviderError::Protocol(
            "uploaded object could not be verified".into(),
        ))
    }

    async fn delete(&self, key: &str, expected: &Revision) -> ProviderResult<()> {
        if expected.strength != RevisionStrength::Strong {
            return Err(ProviderError::Unsupported(
                "weak ETag cannot authorize automatic deletion".into(),
            ));
        }
        let response = self
            .request(Method::DELETE, self.object_url(key, false)?)
            .header(IF_MATCH, &expected.value)
            .send()
            .await
            .map_err(map_transport)?;
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(map_status(response.status(), key))
        }
    }
}

#[derive(Default)]
struct ParsedResponse {
    href: Option<String>,
    status: Option<String>,
    etag: Option<String>,
    length: Option<u64>,
    modified: Option<String>,
    collection: bool,
}

fn parse_multistatus(body: &[u8], base: &Url) -> ProviderResult<Vec<RemoteEntry>> {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(true);
    let mut entries = Vec::new();
    let mut response: Option<ParsedResponse> = None;
    let mut field = Vec::<u8>::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                let name = start.local_name().as_ref().to_vec();
                if name.as_slice() == b"response" {
                    response = Some(ParsedResponse::default());
                }
                if name.as_slice() == b"collection" {
                    if let Some(response) = response.as_mut() {
                        response.collection = true;
                    }
                }
                field = name;
            }
            Ok(Event::Empty(empty)) => {
                if empty.local_name().as_ref() == b"collection" {
                    if let Some(response) = response.as_mut() {
                        response.collection = true;
                    }
                }
            }
            Ok(Event::Text(text)) => {
                let Some(response) = response.as_mut() else {
                    continue;
                };
                let value = text
                    .decode()
                    .map_err(|error| ProviderError::Protocol(error.to_string()))?
                    .into_owned();
                match field.as_slice() {
                    b"href" => response.href = Some(value),
                    b"status" => response.status = Some(value),
                    b"getetag" => response.etag = Some(value),
                    b"getcontentlength" => response.length = value.parse().ok(),
                    b"getlastmodified" => response.modified = Some(value),
                    _ => {}
                }
            }
            Ok(Event::End(end)) if end.local_name().as_ref() == b"response" => {
                if let Some(response) = response.take() {
                    if response
                        .status
                        .as_deref()
                        .is_some_and(|status| !status.contains(" 200 "))
                    {
                        continue;
                    }
                    let href = response.href.ok_or_else(|| {
                        ProviderError::Protocol("DAV response has no href".into())
                    })?;
                    let key = href_to_key(base, &href)?;
                    let revision = response
                        .etag
                        .map(|etag| Revision::etag(normalize_etag(&etag)));
                    entries.push(RemoteEntry {
                        key,
                        is_collection: response.collection,
                        size: response.length.unwrap_or(0),
                        modified_unix: None,
                        revision,
                    });
                }
                field.clear();
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(ProviderError::Protocol(error.to_string())),
        }
    }
    Ok(entries)
}

fn href_to_key(base: &Url, href: &str) -> ProviderResult<String> {
    let url = base
        .join(href)
        .map_err(|error| ProviderError::Protocol(format!("invalid DAV href: {error}")))?;
    if url.scheme() != base.scheme()
        || url.host_str() != base.host_str()
        || url.port_or_known_default() != base.port_or_known_default()
    {
        return Err(ProviderError::Protocol(
            "DAV href escaped the configured origin".into(),
        ));
    }
    let relative = url
        .path()
        .strip_prefix(base.path())
        .ok_or_else(|| ProviderError::Protocol("DAV href escaped the configured root".into()))?;
    let decoded = percent_decode_str(relative)
        .decode_utf8()
        .map_err(|error| ProviderError::Protocol(format!("invalid href encoding: {error}")))?;
    let key = decoded.trim_matches('/').to_string();
    validate_key(&key)?;
    Ok(key)
}

fn validate_key(key: &str) -> ProviderResult<()> {
    if key.starts_with('/')
        || key
            .split('/')
            .any(|segment| segment == "." || segment == ".." || segment.contains('\0'))
    {
        return Err(ProviderError::Protocol(
            "remote object key escapes the configured root".into(),
        ));
    }
    Ok(())
}

fn header_revision(headers: &reqwest::header::HeaderMap) -> Option<Revision> {
    headers
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .map(Revision::etag)
}

fn normalize_etag(value: &str) -> String {
    let value = value.trim();
    if value.starts_with('"') || value.starts_with("W/\"") {
        value.to_string()
    } else {
        format!("\"{value}\"")
    }
}

fn map_transport(error: reqwest::Error) -> ProviderError {
    if error.is_timeout() || error.is_connect() {
        ProviderError::Offline(error.to_string())
    } else {
        ProviderError::Protocol(error.to_string())
    }
}

fn map_status(status: StatusCode, key: &str) -> ProviderError {
    match status {
        StatusCode::UNAUTHORIZED => ProviderError::Authentication,
        StatusCode::FORBIDDEN => ProviderError::Permission,
        StatusCode::NOT_FOUND => ProviderError::NotFound(key.into()),
        StatusCode::PRECONDITION_FAILED | StatusCode::CONFLICT => {
            ProviderError::PreconditionFailed(key.into())
        }
        StatusCode::INSUFFICIENT_STORAGE => ProviderError::StorageFull,
        StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimited,
        _ => ProviderError::Protocol(format!("WebDAV returned HTTP {status} for {key}")),
    }
}

#[cfg(test)]
mod tests;
