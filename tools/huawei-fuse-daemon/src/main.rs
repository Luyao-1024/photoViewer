use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry,
    ReplyOpen, Request,
};
use futures_util::{SinkExt, StreamExt};
use reqwest::blocking::Client;
use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, COOKIE, REFERER, USER_AGENT,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};
use tokio::process::{Child, Command};
use tokio_tungstenite::tungstenite::Message;

const ROOT_INO: u64 = 1;
const ALBUM_INO_BASE: u64 = 10_000;
const FILE_INO_BASE: u64 = 1_000_000;
const FILE_INO_STRIDE: u64 = 100_000;
const TTL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
struct Config {
    mountpoint: PathBuf,
    cache_dir: PathBuf,
    profile_dir: PathBuf,
    chrome: PathBuf,
    debug_port: u16,
    count: usize,
    headless: bool,
    auth_timeout: Duration,
    mode: Mode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Mount,
    Login,
}

#[derive(Debug, Deserialize)]
struct JsonVersion {
    #[serde(rename = "webSocketDebuggerUrl")]
    web_socket_debugger_url: String,
}

#[derive(Debug, Deserialize)]
struct JsonTarget {
    #[serde(rename = "type")]
    target_type: String,
    url: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    web_socket_debugger_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CookieEntry {
    name: String,
    value: String,
    domain: String,
}

#[derive(Debug, Deserialize)]
struct SimpleFileResponse {
    #[serde(rename = "resultCode")]
    result_code: Option<Value>,
    #[serde(rename = "resultDesc")]
    result_desc: Option<String>,
    #[serde(rename = "fileList", default)]
    file_list: Vec<SimpleFile>,
    #[serde(rename = "hasMore")]
    has_more: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct AlbumInfoResponse {
    code: i64,
    info: Option<String>,
    #[serde(rename = "albumList", default)]
    album_list: Vec<AlbumInfo>,
}

#[derive(Debug, Deserialize, Clone)]
struct AlbumInfo {
    #[serde(rename = "albumId")]
    album_id: String,
    #[serde(rename = "albumName")]
    album_name: String,
    #[serde(rename = "photoNum")]
    photo_num: usize,
    lpath: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct SimpleFile {
    #[serde(rename = "albumId", default)]
    album_id: String,
    #[serde(rename = "fileType", default)]
    file_type: String,
    #[serde(rename = "uniqueId", default)]
    unique_id: String,
}

#[derive(Debug, Deserialize)]
struct SingleUrlResponse {
    code: i64,
    info: Option<String>,
    #[serde(rename = "urlList", default)]
    url_list: Vec<UrlInfo>,
}

#[derive(Debug, Deserialize, Clone)]
struct UrlInfo {
    #[serde(rename = "fileType")]
    file_type: String,
    #[serde(rename = "uniqueId")]
    unique_id: String,
    url: String,
    sha256: Option<String>,
}

#[derive(Debug, Clone)]
struct RemoteFile {
    ino: u64,
    name: String,
    unique_id: String,
    album_id: String,
}

#[derive(Debug, Clone)]
struct AlbumDir {
    ino: u64,
    name: String,
    album_id: String,
    photo_num: usize,
    files: Option<Vec<RemoteFile>>,
}

struct HuaweiFs {
    http: Client,
    headers: HeaderMap,
    cache_dir: PathBuf,
    debug_port: u16,
    page_size: usize,
    albums: Vec<AlbumDir>,
    album_by_ino: HashMap<u64, usize>,
    album_by_name: HashMap<String, u64>,
    by_ino: HashMap<u64, RemoteFile>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::parse()?;
    std::fs::create_dir_all(&config.cache_dir)?;
    std::fs::create_dir_all(&config.profile_dir)?;

    match config.mode {
        Mode::Login => run_login(config).await,
        Mode::Mount => run_mount(config).await,
    }
}

async fn run_login(config: Config) -> Result<()> {
    let mut chrome = launch_chrome(&config, false).await?;
    println!(
        "login browser started with profile {}",
        config.profile_dir.display()
    );
    println!("finish Huawei Cloud login in the browser window; this command will return after auth is valid");
    let http = Client::builder().timeout(Duration::from_secs(20)).build()?;
    loop {
        if let Ok(cookie) = browser_cookie_header_with_timeout(config.debug_port).await {
            if let Ok(headers) = base_headers(&cookie) {
                if fetch_albums_resilient(&http, &headers, config.debug_port)
                    .await
                    .is_ok()
                {
                    println!("Huawei Cloud auth is valid");
                    let _ = chrome.kill().await;
                    return Ok(());
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn run_mount(config: Config) -> Result<()> {
    std::fs::create_dir_all(&config.mountpoint)?;
    let mut chrome = launch_chrome(&config, config.headless).await?;
    if !config.headless {
        println!(
            "visible Chromium started; waiting up to {} seconds for Huawei Cloud auth",
            config.auth_timeout.as_secs()
        );
    }
    let http = Client::builder()
        .timeout(Duration::from_secs(90))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()?;

    let started = SystemTime::now();
    let mut last_error = anyhow!("Huawei Cloud auth did not complete");
    let (headers, files) = loop {
        let attempt = async {
            let cookie = browser_cookie_header_with_timeout(config.debug_port)
                .await
                .context("read Huawei Cloud cookies from Chromium profile")?;
            if cookie.is_empty() {
                bail_auth(&config)?;
            }
            let headers = base_headers(&cookie)?;
            let albums = fetch_albums_resilient(&http, &headers, config.debug_port).await?;
            Ok::<_, anyhow::Error>((headers, albums))
        }
        .await;
        match attempt {
            Ok(pair) => break pair,
            Err(err) => last_error = err,
        }
        if SystemTime::now()
            .duration_since(started)
            .unwrap_or_default()
            > config.auth_timeout
        {
            let _ = chrome.kill().await;
            bail!(
                "Huawei Cloud auth is unavailable or expired: {last_error:#}\nrun: huawei-fuse-daemon --login --profile {}",
                config.profile_dir.display()
            );
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    };
    println!("indexed {} Huawei Cloud albums", files.len());
    println!("mounting {}", config.mountpoint.display());

    let fs = HuaweiFs::new(
        http,
        headers,
        config.cache_dir.clone(),
        config.debug_port,
        config.count,
        files,
    );
    let options = vec![
        MountOption::RO,
        MountOption::FSName("huawei-cloud".to_string()),
    ];
    let result = fuser::mount2(fs, &config.mountpoint, &options).context("mount fuse filesystem");
    let _ = chrome.kill().await;
    result
}

impl Config {
    fn parse() -> Result<Self> {
        let data_dir = data_dir();
        let cache_base = cache_dir();
        let mut config = Self {
            mountpoint: data_dir.join("mounts/huawei-cloud"),
            cache_dir: cache_base.join("remotes/huawei-cloud"),
            profile_dir: data_dir.join("auth/huawei-cloud-profile"),
            chrome: find_chrome().ok_or_else(|| {
                anyhow!("Chromium executable not found; pass --chrome /path/to/chromium")
            })?,
            debug_port: 9333,
            count: 200,
            headless: true,
            auth_timeout: Duration::from_secs(300),
            mode: Mode::Mount,
        };

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--login" => config.mode = Mode::Login,
                "--visible" => config.headless = false,
                "--auth-timeout" => {
                    let seconds: u64 = args
                        .next()
                        .ok_or_else(|| anyhow!("--auth-timeout requires seconds"))?
                        .parse()
                        .context("parse --auth-timeout")?;
                    config.auth_timeout = Duration::from_secs(seconds);
                }
                "--mount" => config.mountpoint = next_path(&mut args, "--mount")?,
                "--cache" => config.cache_dir = next_path(&mut args, "--cache")?,
                "--profile" => config.profile_dir = next_path(&mut args, "--profile")?,
                "--chrome" => config.chrome = next_path(&mut args, "--chrome")?,
                "--port" => {
                    config.debug_port = args
                        .next()
                        .ok_or_else(|| anyhow!("--port requires a value"))?
                        .parse()
                        .context("parse --port")?;
                }
                "--count" => {
                    config.count = args
                        .next()
                        .ok_or_else(|| anyhow!("--count requires a value"))?
                        .parse()
                        .context("parse --count")?;
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                other => bail!("unknown argument: {other}"),
            }
        }
        Ok(config)
    }
}

fn next_path(args: &mut impl Iterator<Item = String>, name: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(
        args.next()
            .ok_or_else(|| anyhow!("{name} requires a value"))?,
    ))
}

fn print_help() {
    println!(
        "Usage: huawei-fuse-daemon [--login] [--visible] [--auth-timeout SECONDS] [--mount PATH] [--cache PATH] [--profile PATH] [--chrome PATH] [--port PORT] [--count N]\n\n\
         Default mount:   ~/.local/share/photoViewer/mounts/huawei-cloud\n\
         Default cache:   ~/.cache/photoViewer/remotes/huawei-cloud\n\
         Default profile: ~/.local/share/photoViewer/auth/huawei-cloud-profile\n\n\
         Run --login once when the profile has no valid Huawei Cloud session.\n\
         Mount mode waits up to 300 seconds for auth by default.\n\
         --count controls the per-album API page size."
    );
}

impl HuaweiFs {
    fn new(
        http: Client,
        headers: HeaderMap,
        cache_dir: PathBuf,
        debug_port: u16,
        page_size: usize,
        albums: Vec<AlbumDir>,
    ) -> Self {
        let album_by_ino = albums
            .iter()
            .enumerate()
            .map(|(idx, album)| (album.ino, idx))
            .collect();
        let album_by_name = albums
            .iter()
            .map(|album| (album.name.clone(), album.ino))
            .collect();
        Self {
            http,
            headers,
            cache_dir,
            debug_port,
            page_size,
            albums,
            album_by_ino,
            album_by_name,
            by_ino: HashMap::new(),
        }
    }

    fn file_attr(&self, file: &RemoteFile) -> FileAttr {
        let size = self
            .cache_path(file)
            .metadata()
            .map(|m| m.len())
            .unwrap_or(16 * 1024 * 1024);
        FileAttr {
            ino: file.ino,
            size,
            blocks: size.div_ceil(512),
            atime: SystemTime::now(),
            mtime: SystemTime::now(),
            ctime: SystemTime::now(),
            crtime: SystemTime::now(),
            kind: FileType::RegularFile,
            perm: 0o444,
            nlink: 1,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    fn cache_path(&self, file: &RemoteFile) -> PathBuf {
        self.cache_dir.join(format!("{}.bin", file.unique_id))
    }

    fn ensure_cached(&self, file: &RemoteFile) -> Result<Vec<u8>> {
        let path = self.cache_path(file);
        if let Ok(bytes) = std::fs::read(&path) {
            return Ok(bytes);
        }

        let url_info = self.fetch_single_url(file)?;
        let media_url = format!(
            "https://cloud.huawei.com:443/proxy/v1/download/%2Fv2%2FcloudPhoto%2Fcallback%2Fv1%2Fmedia%2FDB3zWvEEAFYA{}",
            file.unique_id
        );
        let http_result = self
            .http
            .get(&media_url)
            .headers(self.headers.clone())
            .send()?
            .error_for_status()
            .and_then(|r| r.bytes())
            .map(|b| b.to_vec());
        let bytes = match http_result {
            Ok(bytes) => bytes,
            Err(err) => {
                eprintln!(
                    "direct HTTP media download failed for {}, falling back to browser context: {err}",
                    file.name
                );
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(cdp_fetch_bytes(self.debug_port, &media_url))?
            }
        };
        if let Some(expected) = url_info.sha256.as_deref() {
            let actual = sha256_hex(&bytes);
            if actual != expected {
                eprintln!(
                    "warning: sha256 mismatch for {}: expected {}, got {}",
                    file.name, expected, actual
                );
            }
        }
        std::fs::write(&path, &bytes)?;
        Ok(bytes)
    }

    fn fetch_single_url(&self, file: &RemoteFile) -> Result<UrlInfo> {
        let response: SingleUrlResponse = post_json_blocking(
            &self.http,
            &self.headers,
            "https://cloud.huawei.com/album/getSingleUrl",
            json!({
                "fileList": [{"uniqueId": file.unique_id, "albumId": file.album_id}],
                "type": "1",
                "thumbType": "imgszexqu",
                "thumbHeight": 350,
                "thumbWidth": 350,
                "traceId": trace_id("04101")
            }),
        )?;
        if response.code != 0 {
            bail!("getSingleUrl failed: {} {:?}", response.code, response.info);
        }
        let info = response
            .url_list
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("empty getSingleUrl urlList"))?;
        let _ = (&info.file_type, &info.unique_id, &info.url);
        Ok(info)
    }

    fn album_attr(&self, album: &AlbumDir) -> FileAttr {
        let mut attr = dir_attr(album.ino);
        attr.size = album.photo_num as u64;
        attr
    }

    fn ensure_album_loaded(&mut self, album_idx: usize) -> Result<()> {
        if self.albums[album_idx].files.is_some() {
            return Ok(());
        }

        let album_id = self.albums[album_idx].album_id.clone();
        let files = fetch_album_files(
            &self.http,
            &self.headers,
            &album_id,
            self.page_size.max(1),
            album_idx,
            self.debug_port,
        )?;
        for file in &files {
            self.by_ino.insert(file.ino, file.clone());
        }
        self.albums[album_idx].files = Some(files);
        Ok(())
    }

    fn lookup_file_in_album(&mut self, album_idx: usize, name: &str) -> Result<Option<RemoteFile>> {
        self.ensure_album_loaded(album_idx)?;
        Ok(self.albums[album_idx]
            .files
            .as_ref()
            .and_then(|files| files.iter().find(|file| file.name == name).cloned()))
    }
}

impl Filesystem for HuaweiFs {
    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        if ino == ROOT_INO {
            reply.attr(&TTL, &dir_attr(ROOT_INO));
        } else if let Some(album_idx) = self.album_by_ino.get(&ino).copied() {
            reply.attr(&TTL, &self.album_attr(&self.albums[album_idx]));
        } else if let Some(file) = self.by_ino.get(&ino) {
            reply.attr(&TTL, &self.file_attr(file));
        } else {
            reply.error(libc::ENOENT);
        }
    }

    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let Some(name) = name.to_str() else {
            reply.error(libc::ENOENT);
            return;
        };
        if parent == ROOT_INO {
            if let Some(ino) = self.album_by_name.get(name).copied() {
                if let Some(album_idx) = self.album_by_ino.get(&ino).copied() {
                    reply.entry(&TTL, &self.album_attr(&self.albums[album_idx]), 0);
                    return;
                }
            }
            reply.error(libc::ENOENT);
        } else if let Some(album_idx) = self.album_by_ino.get(&parent).copied() {
            match self.lookup_file_in_album(album_idx, name) {
                Ok(Some(file)) => reply.entry(&TTL, &self.file_attr(&file), 0),
                Ok(None) => reply.error(libc::ENOENT),
                Err(err) => {
                    eprintln!(
                        "lookup in album {} failed: {err:#}",
                        self.albums[album_idx].name
                    );
                    reply.error(libc::EIO);
                }
            }
        } else {
            reply.error(libc::ENOENT);
        }
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let mut entries: Vec<(u64, FileType, String)> = Vec::new();
        if ino == ROOT_INO {
            entries.push((ROOT_INO, FileType::Directory, ".".into()));
            entries.push((ROOT_INO, FileType::Directory, "..".into()));
            entries.extend(
                self.albums
                    .iter()
                    .map(|album| (album.ino, FileType::Directory, album.name.clone())),
            );
        } else if let Some(album_idx) = self.album_by_ino.get(&ino).copied() {
            if let Err(err) = self.ensure_album_loaded(album_idx) {
                eprintln!("load album {} failed: {err:#}", self.albums[album_idx].name);
                reply.error(libc::EIO);
                return;
            }
            entries.push((ino, FileType::Directory, ".".into()));
            entries.push((ROOT_INO, FileType::Directory, "..".into()));
            entries.extend(
                self.albums[album_idx]
                    .files
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|f| (f.ino, FileType::RegularFile, f.name.clone())),
            );
        } else {
            reply.error(libc::ENOENT);
            return;
        }

        for (idx, entry) in entries.into_iter().enumerate().skip(offset as usize) {
            if reply.add(entry.0, (idx + 1) as i64, entry.1, entry.2) {
                break;
            }
        }
        reply.ok();
    }

    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        if flags & libc::O_ACCMODE != libc::O_RDONLY {
            reply.error(libc::EACCES);
        } else if self.by_ino.contains_key(&ino) {
            reply.opened(0, 0);
        } else {
            reply.error(libc::ENOENT);
        }
    }

    fn read(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        let Some(file) = self.by_ino.get(&ino) else {
            reply.error(libc::ENOENT);
            return;
        };
        match self.ensure_cached(file) {
            Ok(bytes) => {
                let start = offset.max(0) as usize;
                if start >= bytes.len() {
                    reply.data(&[]);
                    return;
                }
                let end = (start + size as usize).min(bytes.len());
                reply.data(&bytes[start..end]);
            }
            Err(err) => {
                eprintln!("read {} failed: {err:#}", file.name);
                reply.error(libc::EIO);
            }
        }
    }
}

fn fetch_albums(http: &Client, headers: &HeaderMap) -> Result<Vec<AlbumDir>> {
    let response: AlbumInfoResponse = post_json_blocking(
        http,
        headers,
        "https://cloud.huawei.com/album/queryAlbumInfo",
        query_album_info_body(),
    )?;
    album_dirs_from_response(response)
}

async fn fetch_albums_resilient(
    http: &Client,
    headers: &HeaderMap,
    debug_port: u16,
) -> Result<Vec<AlbumDir>> {
    match fetch_albums(http, headers) {
        Ok(albums) => Ok(albums),
        Err(err) => {
            eprintln!("direct queryAlbumInfo failed, falling back to browser context: {err}");
            let text = cdp_post_json_text(
                debug_port,
                "https://cloud.huawei.com/album/queryAlbumInfo",
                query_album_info_body(),
            )
            .await?;
            match serde_json::from_str::<AlbumInfoResponse>(&text) {
                Ok(response) => album_dirs_from_response(response),
                Err(err) => {
                    eprintln!(
                        "browser queryAlbumInfo returned non-json: {} ({err})",
                        preview(&text)
                    );
                    let captured = cdp_capture_response_body(
                        debug_port,
                        "https://cloud.huawei.com/home#/album/timePhoto",
                        "/album/queryAlbumInfo",
                        Duration::from_secs(20),
                    )
                    .await?;
                    let response: AlbumInfoResponse = serde_json::from_str(&captured)
                        .with_context(|| {
                            format!(
                                "captured queryAlbumInfo returned non-json: {}",
                                preview(&captured)
                            )
                        })?;
                    album_dirs_from_response(response)
                }
            }
        }
    }
}

fn query_album_info_body() -> Value {
    json!({
        "isHash": false,
        "language": "en-us",
        "traceId": trace_id("04113")
    })
}

fn album_dirs_from_response(response: AlbumInfoResponse) -> Result<Vec<AlbumDir>> {
    if response.code != 0 {
        bail!(
            "queryAlbumInfo failed: {} {:?}",
            response.code,
            response.info
        );
    }

    let mut used_names = HashMap::<String, usize>::new();
    let mut albums = Vec::new();
    for item in response.album_list {
        if item.album_id.is_empty() {
            continue;
        }
        let idx = albums.len();
        let short_id = short_id(&item.album_id);
        let raw_name = if item.album_name.trim().is_empty() {
            item.lpath.as_deref().unwrap_or("album")
        } else {
            &item.album_name
        };
        let name = unique_name(sanitize_name(raw_name, "album"), &short_id, &mut used_names);
        albums.push(AlbumDir {
            ino: ALBUM_INO_BASE + idx as u64,
            name,
            album_id: item.album_id,
            photo_num: item.photo_num,
            files: None,
        });
    }
    Ok(albums)
}

fn fetch_album_files(
    http: &Client,
    headers: &HeaderMap,
    album_id: &str,
    page_size: usize,
    album_idx: usize,
    debug_port: u16,
) -> Result<Vec<RemoteFile>> {
    let mut current_num = 0usize;
    let mut files = Vec::new();
    let mut used_names = HashMap::<String, usize>::new();

    loop {
        let body = get_simple_file_body(album_id, current_num, page_size);
        let response: SimpleFileResponse = match post_json_blocking(
            http,
            headers,
            "https://cloud.huawei.com/album/getSimpleFile",
            body.clone(),
        ) {
            Ok(response) => response,
            Err(err) => {
                eprintln!(
                        "direct getSimpleFile failed for album {}, falling back to browser context: {err}",
                        album_id
                    );
                let rt = tokio::runtime::Runtime::new()?;
                let text = rt.block_on(cdp_post_json_text(
                    debug_port,
                    "https://cloud.huawei.com/album/getSimpleFile",
                    body,
                ))?;
                serde_json::from_str(&text).with_context(|| {
                    format!(
                        "browser getSimpleFile returned non-json: {}",
                        preview(&text)
                    )
                })?
            }
        };
        if !is_success_code(response.result_code.as_ref()) {
            bail!(
                "getSimpleFile failed for album {}: {:?} {:?}",
                album_id,
                response.result_code,
                response.result_desc
            );
        }

        let has_more = response.has_more;
        let mut accepted_on_page = 0usize;
        let page_len = response.file_list.len();
        for item in response
            .file_list
            .into_iter()
            .filter(|f| f.file_type == "1")
        {
            if item.unique_id.is_empty() {
                continue;
            }
            let idx = files.len();
            let album_id_for_file = if item.album_id.is_empty() {
                album_id.to_string()
            } else {
                item.album_id
            };
            let short_id = short_id(&item.unique_id);
            let base = format!("{short_id}_{}", idx + 1);
            files.push(RemoteFile {
                ino: FILE_INO_BASE + album_idx as u64 * FILE_INO_STRIDE + idx as u64,
                name: format!("{}.jpg", unique_name(base, &short_id, &mut used_names)),
                unique_id: item.unique_id,
                album_id: album_id_for_file,
            });
            accepted_on_page += 1;
        }

        current_num += page_len;
        if has_more != Some(true) || page_len == 0 {
            break;
        }
        if accepted_on_page == 0 && page_len < page_size {
            break;
        }
    }

    Ok(files)
}

fn get_simple_file_body(album_id: &str, current_num: usize, page_size: usize) -> Value {
    json!({
        "albumId": album_id,
        "currentNum": current_num,
        "count": page_size,
        "type": Value::Null,
        "traceId": trace_id("04118")
    })
}

fn sanitize_name(raw: &str, fallback: &str) -> String {
    let cleaned = raw
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | '\0' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string();
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned
    }
}

fn unique_name(base: String, stable_suffix: &str, used: &mut HashMap<String, usize>) -> String {
    let next_count = used.get(&base).copied().unwrap_or(0) + 1;
    used.insert(base.clone(), next_count);
    if next_count == 1 {
        return base;
    }

    let candidate = format!("{base}_{stable_suffix}");
    if !used.contains_key(&candidate) {
        used.insert(candidate.clone(), 1);
        return candidate;
    }

    format!("{candidate}_{next_count}")
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect::<String>()
}

fn post_json_blocking<T: for<'de> Deserialize<'de>>(
    http: &Client,
    headers: &HeaderMap,
    url: &str,
    body: Value,
) -> Result<T> {
    let response = http.post(url).headers(headers.clone()).json(&body).send()?;
    let status = response.status();
    let text = response.text()?;
    if !status.is_success() {
        bail!("POST {url} returned {status}: {}", preview(&text));
    }
    serde_json::from_str(&text)
        .with_context(|| format!("POST {url} returned non-json: {}", preview(&text)))
}

async fn launch_chrome(config: &Config, headless: bool) -> Result<Child> {
    if cdp_ready(config.debug_port).await {
        return Err(anyhow!(
            "debug port {} is already in use; pass --port or stop the existing Chromium",
            config.debug_port
        ));
    }

    let mut command = Command::new(&config.chrome);
    command
        .arg(format!("--remote-debugging-port={}", config.debug_port))
        .arg(format!("--user-data-dir={}", config.profile_dir.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--no-sandbox")
        .arg("--disable-background-networking")
        .arg("https://cloud.huawei.com/home#/album/timePhoto")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if headless {
        command.arg("--headless=new");
        command.arg("--disable-gpu");
    }

    let child = command.spawn().context("launch Chromium")?;
    for _ in 0..80 {
        if cdp_ready(config.debug_port).await {
            return Ok(child);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    bail!("Chromium remote debugging endpoint did not start")
}

async fn cdp_ready(port: u16) -> bool {
    reqwest::get(format!("http://127.0.0.1:{port}/json/version"))
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

async fn browser_cookie_header(port: u16) -> Result<String> {
    let version: JsonVersion = reqwest::get(format!("http://127.0.0.1:{port}/json/version"))
        .await?
        .error_for_status()?
        .json()
        .await?;
    let (mut ws, _) = tokio_tungstenite::connect_async(version.web_socket_debugger_url).await?;
    ws.send(Message::Text(
        json!({"id": 1, "method": "Storage.getCookies"}).to_string(),
    ))
    .await?;
    while let Some(msg) = ws.next().await {
        let msg = msg?;
        let Message::Text(text) = msg else { continue };
        let value: Value = serde_json::from_str(&text)?;
        if value.get("id").and_then(Value::as_i64) != Some(1) {
            continue;
        }
        let cookies: Vec<CookieEntry> = serde_json::from_value(
            value
                .get("result")
                .and_then(|v| v.get("cookies"))
                .cloned()
                .ok_or_else(|| anyhow!("CDP Storage.getCookies returned no cookies"))?,
        )?;
        return Ok(cookies
            .into_iter()
            .filter(|c| c.domain.contains("huawei.com") || c.domain.contains("dbankcloud.cn"))
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; "));
    }
    bail!("CDP websocket closed before cookies were returned")
}

async fn browser_cookie_header_with_timeout(port: u16) -> Result<String> {
    tokio::time::timeout(Duration::from_secs(10), browser_cookie_header(port))
        .await
        .context("CDP cookie request timed out")?
}

async fn cdp_post_json_text(port: u16, url: &str, body: Value) -> Result<String> {
    let ws_url = huawei_page_ws_url(port).await?;
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url).await?;
    let url_json = serde_json::to_string(url)?;
    let body_json = serde_json::to_string(&body)?;
    let expression = format!(
        r#"(async () => {{
            const response = await fetch({url_json}, {{
                method: "POST",
                credentials: "include",
                headers: {{
                    "accept": "application/json, text/plain, */*",
                    "content-type": "application/json;charset=UTF-8",
                    "x-hw-client-mode": "WEB_WAP",
                    "x-hw-os-brand": "Linux",
                    "x-hw-trace-id": "04101_02_1782531976_72444352"
                }},
                body: JSON.stringify({body_json})
            }});
            return {{
                status: response.status,
                contentType: response.headers.get("content-type") || "",
                body: await response.text()
            }};
        }})()"#
    );
    ws.send(Message::Text(
        json!({
            "id": 88,
            "method": "Runtime.evaluate",
            "params": {
                "expression": expression,
                "awaitPromise": true,
                "returnByValue": true
            }
        })
        .to_string(),
    ))
    .await?;
    while let Some(msg) = ws.next().await {
        let msg = msg?;
        let Message::Text(text) = msg else { continue };
        let value: Value = serde_json::from_str(&text)?;
        if value.get("id").and_then(Value::as_i64) != Some(88) {
            continue;
        }
        if let Some(exception) = value.get("result").and_then(|v| v.get("exceptionDetails")) {
            bail!("CDP POST exception: {exception}");
        }
        let result = value
            .get("result")
            .and_then(|v| v.get("result"))
            .and_then(|v| v.get("value"))
            .ok_or_else(|| anyhow!("CDP POST returned no value: {value}"))?;
        let status = result
            .get("status")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("CDP POST returned no status: {result}"))?;
        let body = result
            .get("body")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("CDP POST returned no body"))?;
        if !(200..300).contains(&status) {
            bail!("CDP POST returned HTTP {status}: {}", preview(body));
        }
        return Ok(body.to_string());
    }
    bail!("CDP websocket closed before POST completed")
}

async fn cdp_capture_response_body(
    port: u16,
    _navigate_url: &str,
    url_part: &str,
    wait: Duration,
) -> Result<String> {
    tokio::time::timeout(wait, async {
        let ws_url = huawei_page_ws_url(port).await?;
        let (mut ws, _) = tokio_tungstenite::connect_async(ws_url).await?;
        ws.send(Message::Text(
            json!({"id": 101, "method": "Network.enable"}).to_string(),
        ))
        .await?;
        ws.send(Message::Text(
            json!({"id": 102, "method": "Page.reload", "params": {"ignoreCache": true}})
                .to_string(),
        ))
        .await?;

        while let Some(msg) = ws.next().await {
            let msg = msg?;
            let Message::Text(text) = msg else { continue };
            let value: Value = serde_json::from_str(&text)?;
            if value.get("method").and_then(Value::as_str) != Some("Network.responseReceived") {
                continue;
            }
            let Some(params) = value.get("params") else {
                continue;
            };
            let Some(response_url) = params
                .get("response")
                .and_then(|r| r.get("url"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            if !response_url.contains(url_part) {
                continue;
            }
            let request_id = params
                .get("requestId")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("Network.responseReceived had no requestId"))?
                .to_string();
            ws.send(Message::Text(
                json!({
                    "id": 103,
                    "method": "Network.getResponseBody",
                    "params": {"requestId": request_id}
                })
                .to_string(),
            ))
            .await?;
            while let Some(body_msg) = ws.next().await {
                let body_msg = body_msg?;
                let Message::Text(body_text) = body_msg else {
                    continue;
                };
                let body_value: Value = serde_json::from_str(&body_text)?;
                if body_value.get("id").and_then(Value::as_i64) != Some(103) {
                    continue;
                }
                if let Some(error) = body_value.get("error") {
                    bail!("Network.getResponseBody failed: {error}");
                }
                let result = body_value
                    .get("result")
                    .ok_or_else(|| anyhow!("Network.getResponseBody returned no result"))?;
                let body = result
                    .get("body")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("Network.getResponseBody returned no body"))?;
                if result
                    .get("base64Encoded")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    let bytes = base64::engine::general_purpose::STANDARD.decode(body)?;
                    return Ok(String::from_utf8(bytes)?);
                }
                return Ok(body.to_string());
            }
        }
        bail!("CDP websocket closed before matching response was captured")
    })
    .await
    .context("timed out waiting for page network response")?
}

async fn cdp_fetch_bytes(port: u16, url: &str) -> Result<Vec<u8>> {
    let ws_url = huawei_page_ws_url(port).await?;
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url).await?;
    let url_json = serde_json::to_string(url)?;
    let expression = format!(
        r#"(async () => {{
            const response = await fetch({url_json}, {{ credentials: "include" }});
            const buffer = new Uint8Array(await response.arrayBuffer());
            let binary = "";
            for (let i = 0; i < buffer.length; i += 32768) {{
                binary += String.fromCharCode(...buffer.subarray(i, i + 32768));
            }}
            return {{
                status: response.status,
                contentType: response.headers.get("content-type") || "",
                body: btoa(binary)
            }};
        }})()"#
    );
    ws.send(Message::Text(
        json!({
            "id": 77,
            "method": "Runtime.evaluate",
            "params": {
                "expression": expression,
                "awaitPromise": true,
                "returnByValue": true
            }
        })
        .to_string(),
    ))
    .await?;
    while let Some(msg) = ws.next().await {
        let msg = msg?;
        let Message::Text(text) = msg else { continue };
        let value: Value = serde_json::from_str(&text)?;
        if value.get("id").and_then(Value::as_i64) != Some(77) {
            continue;
        }
        if let Some(exception) = value.get("result").and_then(|v| v.get("exceptionDetails")) {
            bail!("CDP fetch exception: {exception}");
        }
        let result = value
            .get("result")
            .and_then(|v| v.get("result"))
            .and_then(|v| v.get("value"))
            .ok_or_else(|| anyhow!("CDP fetch returned no value: {value}"))?;
        let status = result
            .get("status")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("CDP fetch returned no status: {result}"))?;
        if !(200..300).contains(&status) {
            bail!("CDP fetch returned HTTP {status}");
        }
        let body = result
            .get("body")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("CDP fetch returned no body"))?;
        return Ok(base64::engine::general_purpose::STANDARD.decode(body)?);
    }
    bail!("CDP websocket closed before fetch completed")
}

async fn huawei_page_ws_url(port: u16) -> Result<String> {
    let targets: Vec<JsonTarget> = reqwest::get(format!("http://127.0.0.1:{port}/json/list"))
        .await?
        .error_for_status()?
        .json()
        .await?;
    targets
        .into_iter()
        .find(|t| {
            t.target_type == "page"
                && t.url.contains("cloud.huawei.com")
                && t.web_socket_debugger_url.is_some()
        })
        .and_then(|t| t.web_socket_debugger_url)
        .ok_or_else(|| anyhow!("no Huawei Cloud page target available for CDP"))
}

fn base_headers(cookie: &str) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36"));
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/json, text/plain, */*"),
    );
    headers.insert(
        ACCEPT_LANGUAGE,
        HeaderValue::from_static("en-US,en;q=0.9,zh-CN;q=0.8,zh;q=0.7"),
    );
    headers.insert(
        REFERER,
        HeaderValue::from_static("https://cloud.huawei.com/home"),
    );
    headers.insert(
        "origin",
        HeaderValue::from_static("https://cloud.huawei.com"),
    );
    headers.insert("x-hw-client-mode", HeaderValue::from_static("WEB_WAP"));
    headers.insert("x-hw-os-brand", HeaderValue::from_static("Linux"));
    headers.insert(
        "x-hw-trace-id",
        HeaderValue::from_static("04101_02_1782531976_72444352"),
    );
    headers.insert(COOKIE, HeaderValue::from_str(cookie)?);
    Ok(headers)
}

fn dir_attr(ino: u64) -> FileAttr {
    FileAttr {
        ino,
        size: 0,
        blocks: 0,
        atime: SystemTime::now(),
        mtime: SystemTime::now(),
        ctime: SystemTime::now(),
        crtime: SystemTime::now(),
        kind: FileType::Directory,
        perm: 0o555,
        nlink: 2,
        uid: unsafe { libc::getuid() },
        gid: unsafe { libc::getgid() },
        rdev: 0,
        blksize: 4096,
        flags: 0,
    }
}

fn find_chrome() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CHROME").map(PathBuf::from) {
        if path.exists() {
            return Some(path);
        }
    }
    for name in ["chromium", "google-chrome", "chrome"] {
        if let Some(path) = find_in_path(name) {
            return Some(path);
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    for path in [
        home.join(".local/bin/chromium"),
        home.join(".local/opt/chromium"),
        home.join(".local/opt/chromium/chrome"),
    ] {
        if path.exists() {
            return Some(path);
        }
    }
    None
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| std::env::temp_dir().join("photoViewer-home"))
}

fn data_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home_dir().join(".local/share"));
    base.join("photoViewer")
}

fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home_dir().join(".cache"));
    base.join("photoViewer")
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn trace_id(prefix: &str) -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{prefix}_02_{now}_00000001")
}

fn is_success_code(code: Option<&Value>) -> bool {
    match code {
        None => true,
        Some(Value::Number(n)) => n.as_i64() == Some(0),
        Some(Value::String(s)) => s == "0",
        _ => false,
    }
}

fn preview(text: &str) -> String {
    text.chars()
        .take(500)
        .collect::<String>()
        .replace('\n', "\\n")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn bail_auth(config: &Config) -> Result<()> {
    bail!(
        "Huawei Cloud profile has no usable login cookies: {}\nrun: huawei-fuse-daemon --login --profile {}",
        config.profile_dir.display(),
        config.profile_dir.display()
    )
}

#[allow(dead_code)]
fn _assert_path(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_name_keeps_album_text_but_removes_path_separators() {
        assert_eq!(sanitize_name("旅行/杭州\\西湖", "album"), "旅行_杭州_西湖");
        assert_eq!(sanitize_name("...", "album"), "album");
        assert_eq!(sanitize_name("  Camera  ", "album"), "Camera");
    }

    #[test]
    fn unique_name_keeps_first_name_and_suffixes_duplicates() {
        let mut used = HashMap::new();
        assert_eq!(
            unique_name("Camera".into(), "abc12345", &mut used),
            "Camera"
        );
        assert_eq!(
            unique_name("Camera".into(), "def67890", &mut used),
            "Camera_def67890"
        );
    }

    #[test]
    fn short_id_handles_short_and_unicode_ids() {
        assert_eq!(short_id("abcdef123456"), "abcdef12");
        assert_eq!(short_id("相册一二三四五六七八九"), "相册一二三四五六");
    }
}
