//! 回收站后端包装：优先使用 gio 系统回收站，并提供应用自有 fallback。
//!
//! gio crate 0.19 仅绑定了 `File::trash()`；`restore_from_trash` 与
//! `delete_permanently` 在其公共 API 中未直接暴露。这里采用以下等价流程：
//!
//! * `move_to_system_trash(uri)`：gio `File::trash()` 处理原路径记录。
//! * `move_to_app_trash(uri)`：移动到 App 数据目录下的 freedesktop-style
//!   `Trash/files`，并写入 `Trash/info/*.trashinfo`。
//! * `resolve_trash_entry(original_path)`：扫描候选回收站根下的 `info/*.trashinfo`，
//!   按（percent-decode 后的）`Path=` 字段匹配原路径，得到 gio 实际使用的
//!   `files/` 文件名（含冲突后缀）。
//! * `restore_from_trash(uri)`：用解析出的实际文件名 `File::move_` 回原 `file://`
//!   路径，并清理对应的 `.trashinfo` 元数据文件。
//! * `delete_permanently(uri)`：同样定位实际回收站项，`File::delete()` 并清理
//!   `.trashinfo`。
//!
//! 所有错误最终归并为 `AppError::Gio(glib::Error)` 或 `AppError::Io(...)`。
//!
//! ## Flatpak 注意：实际回收站根是 HOST `~/.local/share/Trash`
//!
//! 沙箱内 `XDG_DATA_HOME` 指向 per-app 目录，但 gio 的 trash 后端跑在 HOST
//! 的 gvfs 守护进程里，文件实际落在 HOST `~/.local/share/Trash/`。因此
//! [`trash_roots`] 同时探测 HOST 回收站根与 per-app `XDG_DATA_HOME/Trash`。
//! `.trashinfo` 的 `Path=` 字段是 URL percent-encoded（如 `图片` →
//! `%E5%9B%BE%E7%89%87`），且 gio 冲突后缀可能从 `.0` 开始，所以解析时
//! **扫描所有 `.trashinfo`** 而不是按固定后缀猜文件名。
use crate::config;
use crate::core::backend::local::LocalBackend;
use crate::core::db::{self, DbPool};
use crate::core::error::{AppError, Result};
use crate::core::identity::MediaId;
use crate::core::media::{is_supported_media_path, NewMediaItem};
use crate::core::prefs::{self, TrashBackend};
use gtk::gio::prelude::*;
use gtk4 as gtk;
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// 待探测的回收站根目录，按优先级返回。
///
/// 见模块文档：Flatpak 下实际落点是 HOST `~/.local/share/Trash`，但同时保留
/// per-app `$XDG_DATA_HOME/Trash` 作为候选，兼容非沙箱或自定义 `XDG_DATA_HOME`。
pub fn trash_roots() -> Vec<PathBuf> {
    let mut roots = system_trash_roots();
    let app = app_trash_root();
    if !roots.contains(&app) {
        roots.push(app);
    }
    roots
}

/// System trash roots used by gio/gvfs and the freedesktop trash layout.
pub fn system_trash_roots() -> Vec<PathBuf> {
    trash_roots_from(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
}

/// App-owned fallback trash root.
pub fn app_trash_root() -> PathBuf {
    config::data_dir().join("Trash")
}

/// [`trash_roots`] 的纯函数核心，便于单测（不读环境变量/文件系统）。
fn trash_roots_from(xdg_data_home: Option<OsString>, home: Option<OsString>) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    // HOST 回收站根 —— gio/gvfs 后端的实际落点（Flatpak 下尤其重要）。
    if let Some(home) = home {
        roots.push(PathBuf::from(home).join(".local/share/Trash"));
    }
    // per-app / 自定义 XDG 回收站根。
    if let Some(data) = xdg_data_home.filter(|d| !d.is_empty()) {
        let r = PathBuf::from(data).join("Trash");
        if !roots.contains(&r) {
            roots.push(r);
        }
    }
    if roots.is_empty() {
        roots.push(PathBuf::from("/tmp/Trash"));
    }
    roots
}

/// 在候选回收站根的 `info/` 中，按 `Path=` 字段匹配 `original_path`，找出
/// **真实存在**的回收站条目（`files/` 副本也在）。
///
/// 返回 `(files 中的实际文件名, 命中的 trashinfo 文件路径)`；没有任何命中时返回
/// `None`。gio/gvfs 自行决定 `files/` 实际文件名（重名时追加 `.N` 等后缀），并把
/// 同样名字用作 `info/` 里的 `.trashinfo` 文件名，因此从命中 `.trashinfo` 的文件名
/// 可直接推回 `files/` 文件名，**无需猜测后缀**。
///
/// `Path=` 字段是 URL percent-encoded，比较前必须 [`percent_decode`]。
/// [`find_trash_entry`] 的可测试核心：显式传入候选回收站根。
fn find_trash_entry_in(original_path: &Path, trash_roots: &[PathBuf]) -> Option<(String, PathBuf)> {
    for root in trash_roots {
        let info_dir = root.join("info");
        let files_dir = root.join("files");
        let Ok(entries) = std::fs::read_dir(&info_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let info_path = entry.path();
            let Some(file_name) = info_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(actual) = file_name.strip_suffix(".trashinfo") else {
                continue;
            };
            let Ok(content) = std::fs::read_to_string(&info_path) else {
                continue;
            };
            let Some(path_line) = content.lines().find(|l| l.starts_with("Path=")) else {
                continue;
            };
            let recorded = percent_decode(&path_line["Path=".len()..]);
            if Path::new(&recorded) != original_path {
                continue;
            }
            // 防御性校验：files 里确实存在该条目。
            if files_dir.join(actual).exists() {
                return Some((actual.to_string(), info_path));
            }
        }
    }
    None
}

/// [`find_trash_entry`] 的带兜底包装，供缩略图/还原/永久删除使用：找不到真实条目
/// 时，用原始 basename + 第一个候选根构造一个（可能不存在的）路径，调用方拿到后
/// 会优雅失败并报错。
fn resolve_trash_entry_in_roots(
    original_path: &Path,
    trash_roots: &[PathBuf],
) -> Result<(String, PathBuf)> {
    if let Some(found) = find_trash_entry_in(original_path, trash_roots) {
        return Ok(found);
    }
    let root = trash_roots
        .first()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("/tmp/Trash"));
    let basename = original_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| AppError::Backend("orig path has no filename".into()))?;
    Ok((
        basename.to_string(),
        root.join("info").join(format!("{basename}.trashinfo")),
    ))
}

/// 解码 gio 写入 `.trashinfo` `Path=` 字段的 `%XX` percent-encoding
/// （RFC 3986 风格；非 ASCII 字节以 UTF-8 percent-encoding 表示）。
///
/// 非法/不完整的转义序列原样保留，因此纯 ASCII（未编码）路径恒等往返。
fn percent_decode(input: &str) -> String {
    let b = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(hi), Some(lo)) = (hex_digit(b[i + 1]), hex_digit(b[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn percent_encode_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    let mut out = String::new();
    for &b in path.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Return the current filesystem URI for an item already moved to trash.
///
/// `media_items.uri` intentionally keeps the original `file://` URI so restore
/// and permanent delete can find the matching `.trashinfo` entry. Thumbnail
/// decoding, however, needs the actual file now stored under the trash root's
/// `files/` directory (HOST `~/.local/share/Trash/files/` under Flatpak).
pub fn trashed_file_uri(uri: &str) -> Result<String> {
    trashed_file_uri_in_roots(uri, &trash_roots())
}

fn trashed_file_uri_in_roots(uri: &str, trash_roots: &[PathBuf]) -> Result<String> {
    let file = gtk::gio::File::for_uri(uri);
    let path = file
        .path()
        .ok_or_else(|| AppError::Backend(format!("uri {} has no local path", uri)))?;
    let (actual_name, info_path) = resolve_trash_entry_in_roots(&path, trash_roots)?;
    Ok(format!(
        "file://{}",
        files_dir_for(&info_path).join(actual_name).display()
    ))
}

/// 将文件移至系统回收站（gio 自动处理原路径记录）
pub fn move_to_trash(uri: &str) -> Result<()> {
    move_to_system_trash(uri)
}

/// Move a file to the system trash through gio.
pub fn move_to_system_trash(uri: &str) -> Result<()> {
    let file = gtk::gio::File::for_uri(uri);
    file.trash(gtk::gio::Cancellable::NONE)
        .map_err(AppError::Gio)?;
    Ok(())
}

/// Move a file to the app-owned fallback trash.
pub fn move_to_app_trash(uri: &str) -> Result<()> {
    let file = gtk::gio::File::for_uri(uri);
    let path = file
        .path()
        .ok_or_else(|| AppError::Backend(format!("uri {} has no local path", uri)))?;
    move_path_to_trash_root(&path, &app_trash_root())
}

/// Move a file using the currently configured backend.
pub fn move_to_configured_trash(uri: &str) -> Result<()> {
    move_to_backend(uri, prefs::trash_backend())
}

pub fn move_to_backend(uri: &str, backend: TrashBackend) -> Result<()> {
    match backend {
        TrashBackend::System => move_to_system_trash(uri),
        TrashBackend::App => move_to_app_trash(uri),
    }
}

fn move_path_to_trash_root(original_path: &Path, trash_root: &Path) -> Result<()> {
    if !original_path.is_file() {
        return Err(AppError::Backend(format!(
            "file does not exist: {}",
            original_path.display()
        )));
    }
    let info_dir = trash_root.join("info");
    let files_dir = trash_root.join("files");
    std::fs::create_dir_all(&info_dir)?;
    std::fs::create_dir_all(&files_dir)?;

    let basename = original_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| AppError::Backend("orig path has no filename".into()))?;
    let actual = unique_trash_name(&files_dir, &info_dir, basename);
    let target = files_dir.join(&actual);
    move_file(original_path, &target)?;

    let trashinfo_path = info_dir.join(format!("{actual}.trashinfo"));
    let trashinfo = trashinfo_for(original_path);
    if let Err(err) = std::fs::write(&trashinfo_path, trashinfo) {
        let _ = move_file(&target, original_path);
        return Err(AppError::Io(err));
    }
    Ok(())
}

fn trashinfo_for(original_path: &Path) -> String {
    format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        percent_encode_path(original_path),
        chrono::Local::now().format("%Y-%m-%dT%H:%M:%S")
    )
}

fn unique_trash_name(files_dir: &Path, info_dir: &Path, basename: &str) -> String {
    if !files_dir.join(basename).exists()
        && !info_dir.join(format!("{basename}.trashinfo")).exists()
    {
        return basename.to_string();
    }

    let path = Path::new(basename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(basename);
    let ext = path.extension().and_then(|e| e.to_str());
    for n in 0..=9999 {
        let candidate = match ext {
            Some(ext) if !ext.is_empty() => format!("{stem}.{n}.{ext}"),
            _ => format!("{stem}.{n}"),
        };
        if !files_dir.join(&candidate).exists()
            && !info_dir.join(format!("{candidate}.trashinfo")).exists()
        {
            return candidate;
        }
    }
    format!(
        "{basename}.{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    )
}

fn move_file(from: &Path, to: &Path) -> Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(err) if err.raw_os_error() == Some(libc::EXDEV) => {
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)?;
            Ok(())
        }
        Err(err) => Err(AppError::Io(err)),
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TrashMigrationStats {
    pub moved: usize,
    pub skipped: usize,
}

struct MigratedTrashEntry {
    old_file: PathBuf,
    old_info: PathBuf,
    old_info_content: String,
    new_file: PathBuf,
    new_info: PathBuf,
}

pub fn migrate_trash_backend(
    pool: &DbPool,
    from: TrashBackend,
    to: TrashBackend,
    excluded_ids: &[MediaId],
) -> Result<TrashMigrationStats> {
    if from == to {
        return Ok(TrashMigrationStats::default());
    }
    let source_roots = match from {
        TrashBackend::System => system_trash_roots(),
        TrashBackend::App => vec![app_trash_root()],
    };
    let target_root = match to {
        TrashBackend::System => system_trash_roots()
            .into_iter()
            .next()
            .unwrap_or_else(|| PathBuf::from("/tmp/Trash")),
        TrashBackend::App => app_trash_root(),
    };
    migrate_trash_entries_between_roots(pool, &source_roots, &target_root, excluded_ids)
}

pub fn switch_trash_backend(pool: &DbPool, target: TrashBackend) -> Result<TrashMigrationStats> {
    let current = prefs::trash_backend();
    if current == target {
        return Ok(TrashMigrationStats::default());
    }
    if target == TrashBackend::System {
        probe_system_trash()?;
    }
    let stats = migrate_trash_backend(pool, current, target, &[])?;
    prefs::set_trash_backend(target).map_err(AppError::Backend)?;
    Ok(stats)
}

pub fn probe_system_trash() -> Result<()> {
    let roots = config::media_roots();
    let probe_root = roots
        .iter()
        .find(|root| root.is_dir())
        .cloned()
        .unwrap_or_else(config::data_dir);
    probe_system_trash_in(&probe_root)
}

pub fn ensure_startup_trash_backend() {
    let configured = prefs::trash_backend();
    let probe = if configured == TrashBackend::System {
        probe_system_trash().map_err(|err| err.to_string())
    } else {
        Ok(())
    };
    let selected = startup_backend_after_probe(configured, probe);
    if selected != configured {
        if let Err(err) = prefs::set_trash_backend(selected) {
            tracing::warn!("failed to persist startup trash backend fallback: {err}");
        } else {
            tracing::warn!("system trash is unavailable; using app trash fallback");
        }
    }
}

fn startup_backend_after_probe(
    configured: TrashBackend,
    system_probe: std::result::Result<(), String>,
) -> TrashBackend {
    match (configured, system_probe) {
        (TrashBackend::System, Err(_)) => TrashBackend::App,
        (backend, _) => backend,
    }
}

fn probe_system_trash_in(root: &Path) -> Result<()> {
    std::fs::create_dir_all(root)?;
    let probe = root.join(format!(
        ".photo-viewer-trash-probe-{}-{}.tmp",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    std::fs::write(&probe, b"photo-viewer trash probe")?;
    let uri = format!("file://{}", probe.display());
    match move_to_system_trash(&uri) {
        Ok(()) => {
            if let Err(err) = delete_permanently_in_roots(&uri, &system_trash_roots()) {
                tracing::warn!("failed to clean system trash probe: {err}");
            }
            Ok(())
        }
        Err(err) => {
            let _ = std::fs::remove_file(&probe);
            Err(err)
        }
    }
}

fn migrate_trash_entries_between_roots(
    pool: &DbPool,
    source_roots: &[PathBuf],
    target_root: &Path,
    excluded_ids: &[MediaId],
) -> Result<TrashMigrationStats> {
    let excluded = excluded_ids
        .iter()
        .map(|id| id.get())
        .collect::<HashSet<_>>();
    let mut stats = TrashMigrationStats::default();
    let mut migrated = Vec::new();

    for row in db::list_trashed_media(pool)? {
        if excluded.contains(&row.id) {
            stats.skipped += 1;
            continue;
        }
        if find_trash_entry_in(&row.path, std::slice::from_ref(&target_root.to_path_buf()))
            .is_some()
        {
            stats.skipped += 1;
            continue;
        }
        if find_trash_entry_in(&row.path, source_roots).is_none() {
            stats.skipped += 1;
            continue;
        }

        match move_trash_entry_to_root(&row.path, source_roots, target_root) {
            Ok(entry) => {
                migrated.push(entry);
                stats.moved += 1;
            }
            Err(err) => {
                for entry in migrated.iter().rev() {
                    rollback_migrated_entry(entry);
                }
                return Err(err);
            }
        }
    }

    Ok(stats)
}

fn move_trash_entry_to_root(
    original_path: &Path,
    source_roots: &[PathBuf],
    target_root: &Path,
) -> Result<MigratedTrashEntry> {
    let (actual, old_info) = find_trash_entry_in(original_path, source_roots).ok_or_else(|| {
        AppError::Backend(format!(
            "no trash entry for {} in source backend",
            original_path.display()
        ))
    })?;
    let old_file = files_dir_for(&old_info).join(&actual);
    let old_info_content = std::fs::read_to_string(&old_info)?;

    let target_info_dir = target_root.join("info");
    let target_files_dir = target_root.join("files");
    std::fs::create_dir_all(&target_info_dir)?;
    std::fs::create_dir_all(&target_files_dir)?;
    let target_actual = unique_trash_name(&target_files_dir, &target_info_dir, &actual);
    let new_file = target_files_dir.join(&target_actual);
    let new_info = target_info_dir.join(format!("{target_actual}.trashinfo"));

    move_file(&old_file, &new_file)?;
    if let Err(err) = std::fs::write(&new_info, &old_info_content) {
        let _ = move_file(&new_file, &old_file);
        return Err(AppError::Io(err));
    }
    if let Err(err) = std::fs::remove_file(&old_info) {
        tracing::warn!(
            "failed to remove old trash metadata {} during migration: {err}",
            old_info.display()
        );
    }

    Ok(MigratedTrashEntry {
        old_file,
        old_info,
        old_info_content,
        new_file,
        new_info,
    })
}

fn rollback_migrated_entry(entry: &MigratedTrashEntry) {
    if let Some(parent) = entry.old_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Some(parent) = entry.old_info.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if entry.new_file.exists() {
        let _ = move_file(&entry.new_file, &entry.old_file);
    }
    let _ = std::fs::write(&entry.old_info, &entry.old_info_content);
    let _ = std::fs::remove_file(&entry.new_info);
}

/// 把媒体项移到系统回收站并标记 DB 行为 trashed —— **先标记后移动**。
///
/// 这是回收站流程的统一入口，顺序至关重要：
///
/// 1. 先 `db::mark_trashed`：DB 行的 `trashed_at` 立即置位。
/// 2. 再 [`move_to_trash`]：gio 移动文件，文件监听器随之收到原路径的 Remove 事件。
///
/// 监听器靠 `delete_media_by_path ... AND trashed_at IS NULL` 跳过已标记行；
/// 只有先标记，监听器处理 Remove 事件时行已是 trashed，才不会被硬删。
/// 若改成"先移后标"，gio 移动（慢：写 trashinfo + rename）与 `mark_trashed`
/// 之间的窗口会让监听器在 `trashed_at` 仍为 NULL 时把行删掉——多选删除时尤其
/// 频繁，表现为"删多张、回收站只剩一张"。
///
/// 若移动失败则 [`db::unmark_trashed`] 回滚，行保持 live，照片回到列表。
pub fn move_to_trash_marked(pool: &DbPool, id: i64, uri: &str) -> Result<()> {
    db::mark_trashed(pool, id)?;
    match move_to_configured_trash(uri) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = db::unmark_trashed(pool, id);
            Err(e)
        }
    }
}

/// 回收站对账结果统计。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileStats {
    /// 新增的 trashed 行：原属本图库（原路径在相册目录下）但 App 之前没有记录。
    pub inserted: usize,
    /// 已有 live 行被标记为 trashed：文件被外部删到回收站，DB 仍当它是 live。
    pub marked: usize,
    /// 删除的 trashed 行：DB 标了 trashed 但系统回收站里已没有该文件
    ///（外部清空/永久删除）。
    pub pruned: usize,
    /// 跳过：原路径不在相册目录下 / 原路径仍存在（已还原）/ 已是 trashed /
    /// `Trash/files` 副本缺失或无法解析（非图片等）。
    pub skipped: usize,
}

#[derive(Debug, Default)]
pub struct TrashReconcilePlan {
    pub(crate) new_items: Vec<NewMediaItem>,
    pub(crate) mark_ids: Vec<MediaId>,
    pub(crate) prune_ids: Vec<MediaId>,
    pub(crate) skipped: usize,
}

/// 启动时把 DB 的回收站状态与系统回收站做**完全对账**（双向收敛）。
///
/// App 的回收站视图 = DB 里 `trashed_at` 标记的行，**不是**系统回收站的实时镜像
///（见模块文档）。本函数让二者在启动时一致：
///
/// **补（add）**——遍历每个候选回收站根下的 `info/*.trashinfo`：
/// * 仅处理 `Path=`（percent-decode 后）落在 `pictures_root` 下的条目——非本图库
///   的文件（下载、文档等）一律忽略；
/// * 原路径仍存在 → 视为已还原，跳过，保持 live；
/// * 无对应 DB 行 → 从 `Trash/files` 副本提取元数据，按**原始** uri/path 插入新行
///   并标 `trashed_at`；有 live 行 → 标 `trashed_at`；已是 trashed → 跳过。
///
/// **删（prune）**——遍历 DB 已 trashed 的行：
/// * 原路径仍存在 → 已还原，交给启动扫描处理（不在此删）；
/// * 系统回收站里已找不到对应条目（`find_trash_entry` 为空）→ 说明被外部清空/永久
///   删除，删除该 DB 行，回收站视图不再残留打不开的死项。
///
/// 顺序依赖：必须**在相册扫描之后**运行——扫描会把"已还原（原位文件还在）"的
/// trashed 行重新 upsert 成 live（[`crate::core::backend::local::LocalBackend::upsert`]
/// 清 `trashed_at`），于是 prune 不会误删还原项。幂等：多次启动只做收敛。
pub fn reconcile_trash(pool: &DbPool, pictures_root: &Path) -> Result<ReconcileStats> {
    let plan = prepare_trash_reconcile(pool, pictures_root)?;
    commit_trash_reconcile(pool, plan)
}

pub fn prepare_trash_reconcile(pool: &DbPool, pictures_root: &Path) -> Result<TrashReconcilePlan> {
    prepare_trash_reconcile_in(pool, pictures_root, &trash_roots())
}

/// [`reconcile_trash`] 的可测试核心：显式传入候选回收站根，避免单测依赖真实
/// `~/.local/share/Trash` 与环境变量。
#[cfg(test)]
fn reconcile_trash_in(
    pool: &DbPool,
    pictures_root: &Path,
    trash_roots: &[PathBuf],
) -> Result<ReconcileStats> {
    let plan = prepare_trash_reconcile_in(pool, pictures_root, trash_roots)?;
    commit_trash_reconcile(pool, plan)
}

fn prepare_trash_reconcile_in(
    pool: &DbPool,
    pictures_root: &Path,
    trash_roots: &[PathBuf],
) -> Result<TrashReconcilePlan> {
    let mut plan = TrashReconcilePlan::default();
    let backend = LocalBackend::new(pool.clone());

    for root in trash_roots {
        let info_dir = root.join("info");
        let files_dir = root.join("files");
        let Ok(entries) = std::fs::read_dir(&info_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let info_path = entry.path();
            let Some(fname) = info_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(actual) = fname.strip_suffix(".trashinfo") else {
                continue;
            };
            let Ok(content) = std::fs::read_to_string(&info_path) else {
                continue;
            };
            let Some(path_line) = content.lines().find(|l| l.starts_with("Path=")) else {
                continue;
            };
            let original = percent_decode(&path_line["Path=".len()..]);
            let original_path = PathBuf::from(&original);

            // 只回收原路径在相册目录下的条目；其余（下载/文档等）忽略。
            if !original_path.starts_with(pictures_root) {
                plan.skipped += 1;
                continue;
            }
            // HOST 回收站包含文档、文本等非本 App 索引的文件。先按媒体扩展
            // 跳过，避免普通非媒体条目进入元数据解析并产生启动 warning。
            if !is_supported_media_path(&original_path) {
                plan.skipped += 1;
                continue;
            }
            // 原路径仍存在 → 已还原/还在原位，保持 live，不标 trashed。
            if original_path.exists() {
                plan.skipped += 1;
                continue;
            }
            let trash_file = files_dir.join(actual);
            if !trash_file.is_file() {
                plan.skipped += 1;
                continue;
            }

            let uri = format!("file://{}", original_path.display());
            match db::get_media_item_by_uri(pool, &uri)? {
                Some(existing) if existing.trashed_at.is_some() => {
                    plan.skipped += 1; // 已是 trashed，无需处理
                }
                Some(existing) => {
                    plan.mark_ids.push(MediaId::from(existing.id));
                }
                None => {
                    let folder = original_path.parent().unwrap_or_else(|| Path::new("/"));
                    match backend.process_file_at(&trash_file, &uri, &original_path, folder) {
                        Ok(item) => {
                            plan.new_items.push(item);
                        }
                        Err(e) => {
                            // 非图片 / 无法解码：跳过，不影响其余条目。
                            tracing::warn!("回收站对账：解析 {} 失败: {}", trash_file.display(), e);
                            plan.skipped += 1;
                        }
                    }
                }
            }
        }
    }

    // 删（prune）：DB 里 trashed、但系统回收站已无对应文件的行（外部清空/永久删除）。
    // 注意：原位文件仍存在的 trashed 行是"已还原"，启动扫描会把它重新 upsert 成
    // live，这里绝不删——双重保险。
    for row in db::list_trashed_media(pool)? {
        if row.path.exists() {
            continue; // 已还原，交给扫描
        }
        if find_trash_entry_in(&row.path, trash_roots).is_none() {
            plan.prune_ids.push(MediaId::from(row.id));
        }
    }

    Ok(plan)
}

pub(crate) fn commit_trash_reconcile(
    pool: &DbPool,
    plan: TrashReconcilePlan,
) -> Result<ReconcileStats> {
    let mut stats = ReconcileStats {
        skipped: plan.skipped,
        ..Default::default()
    };
    for id in plan.mark_ids {
        db::mark_trashed(pool, id.get())?;
        stats.marked += 1;
    }
    let inserted = db::upsert_media_items_batch(pool, &plan.new_items)?;
    for item in inserted {
        db::mark_trashed(pool, item.id)?;
        stats.inserted += 1;
    }
    for id in plan.prune_ids {
        db::delete_media_item(pool, id.get())?;
        stats.pruned += 1;
    }
    Ok(stats)
}

/// 由命中的 `.trashinfo` 路径推回同根的 `files/` 目录。
///
/// `resolve_trash_entry` 可能在任意一个候选回收站根命中（HOST 或 per-app），
/// 因此 `files/` 路径必须从命中的 trashinfo 推导，而不是另取某个固定根，
/// 否则跨根时会拼出错误路径。
fn files_dir_for(info_path: &Path) -> PathBuf {
    info_path
        .parent()
        .and_then(|p| p.parent())
        .map(|root| root.join("files"))
        .unwrap_or_else(|| PathBuf::from("/tmp/Trash/files"))
}

/// 从回收站还原到原路径
///
/// `uri` 必须是 `move_to_trash` 时传入的原文件 uri（`file://...`）。
pub fn restore_from_trash(uri: &str) -> Result<()> {
    restore_from_trash_in_roots(uri, &trash_roots())
}

fn restore_from_trash_in_roots(uri: &str, trash_roots: &[PathBuf]) -> Result<()> {
    prepare_restore_in_roots(uri, trash_roots)?.commit();
    Ok(())
}

/// Keep trash metadata until the corresponding database commit succeeds.
pub(crate) struct PreparedRestore {
    original: PathBuf,
    trashed: PathBuf,
    info: PathBuf,
    committed: bool,
}

impl PreparedRestore {
    pub(crate) fn commit(mut self) {
        self.committed = true;
        if let Err(error) = std::fs::remove_file(&self.info) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    "restored file, but failed to remove {}: {error}",
                    self.info.display()
                );
            }
        }
    }
}

impl Drop for PreparedRestore {
    fn drop(&mut self) {
        if !self.committed {
            if let Err(error) = gtk::gio::File::for_path(&self.original).move_(
                &gtk::gio::File::for_path(&self.trashed),
                gtk::gio::FileCopyFlags::NONE,
                gtk::gio::Cancellable::NONE,
                None,
            ) {
                tracing::error!(
                    "restore database commit failed; file remains at {}: {error}",
                    self.original.display()
                );
            }
        }
    }
}

pub(crate) fn prepare_restore(uri: &str) -> Result<PreparedRestore> {
    prepare_restore_in_roots(uri, &trash_roots())
}

/// Stage a trash file under a hidden sibling name. If the following database
/// delete fails, dropping this value restores the original trash entry. Once
/// committed, cleanup is best-effort; an undeletable staged file remains
/// recoverable on disk instead of leaving a DB row pointing at no file.
pub(crate) struct PreparedPermanentDelete {
    original: PathBuf,
    staged: PathBuf,
    info: PathBuf,
    committed: bool,
}

impl PreparedPermanentDelete {
    pub(crate) fn commit(mut self) {
        self.committed = true;
        if let Err(error) = std::fs::remove_file(&self.staged) {
            tracing::error!(
                "permanent delete committed, but staged file remains at {}: {error}",
                self.staged.display()
            );
            return;
        }
        if let Err(error) = std::fs::remove_file(&self.info) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("failed to remove {}: {error}", self.info.display());
            }
        }
    }
}

impl Drop for PreparedPermanentDelete {
    fn drop(&mut self) {
        if !self.committed {
            if let Err(error) = std::fs::rename(&self.staged, &self.original) {
                tracing::error!(
                    "delete database commit failed; trash file remains at {}: {error}",
                    self.staged.display()
                );
            }
        }
    }
}

pub(crate) fn prepare_permanent_delete(uri: &str) -> Result<PreparedPermanentDelete> {
    prepare_permanent_delete_in_roots(uri, &trash_roots())
}

fn prepare_permanent_delete_in_roots(
    uri: &str,
    trash_roots: &[PathBuf],
) -> Result<PreparedPermanentDelete> {
    let rest = uri
        .strip_prefix("file://")
        .ok_or_else(|| AppError::Backend("staged delete requires a local file URI".into()))?;
    let original_path = Path::new(rest);
    let (actual_name, info) = resolve_trash_entry_in_roots(original_path, trash_roots)?;
    let original = files_dir_for(&info).join(actual_name);
    let suffix = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let staged = original.with_extension(format!("photo-viewer-delete-{suffix}"));
    std::fs::rename(&original, &staged)?;
    Ok(PreparedPermanentDelete {
        original,
        staged,
        info,
        committed: false,
    })
}

fn prepare_restore_in_roots(uri: &str, trash_roots: &[PathBuf]) -> Result<PreparedRestore> {
    let file = gtk::gio::File::for_uri(uri);
    let path = file
        .path()
        .ok_or_else(|| AppError::Backend(format!("uri {} has no local path", uri)))?;

    let (actual_name, trashinfo_path) = resolve_trash_entry_in_roots(&path, trash_roots)?;
    let trash_child = gtk::gio::File::for_path(files_dir_for(&trashinfo_path).join(&actual_name));
    let target = gtk::gio::File::for_path(&path);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // No overwrite: a new file at the original path belongs to the user.
    trash_child
        .move_(
            &target,
            gtk::gio::FileCopyFlags::NONE,
            gtk::gio::Cancellable::NONE,
            None,
        )
        .map_err(AppError::Gio)?;

    Ok(PreparedRestore {
        original: path,
        trashed: files_dir_for(&trashinfo_path).join(actual_name),
        info: trashinfo_path,
        committed: false,
    })
}

/// 永久删除回收站中的文件
///
/// `uri` 接受两种形式：
/// * `file://...` —— 与 `move_to_trash` 时一致；函数会定位对应实际子项
///   （处理 basename 冲突后缀）并删除它（永久删除）。
/// * `trash:///...` —— 直接删除 trash 项；如 basename 含 `.trashinfo` 信息，
///   也一并清理对应元数据文件。
pub fn delete_permanently(uri: &str) -> Result<()> {
    delete_permanently_in_roots(uri, &trash_roots())
}

fn delete_permanently_in_roots(uri: &str, trash_roots: &[PathBuf]) -> Result<()> {
    if let Some(rest) = uri.strip_prefix("file://") {
        // 提取 basename，再解析实际回收站文件名
        let path = Path::new(rest);
        let (actual_name, trashinfo_path) = resolve_trash_entry_in_roots(path, trash_roots)?;
        let trash_child =
            gtk::gio::File::for_path(files_dir_for(&trashinfo_path).join(&actual_name));
        trash_child
            .delete(gtk::gio::Cancellable::NONE)
            .map_err(AppError::Gio)?;
        if trashinfo_path.exists() {
            let _ = std::fs::remove_file(&trashinfo_path);
        }
        Ok(())
    } else {
        let file = gtk::gio::File::for_uri(uri);
        file.delete(gtk::gio::Cancellable::NONE)
            .map_err(AppError::Gio)?;
        // 若传入的是 trash:///...，同步清理可能的 .trashinfo（探测候选根）
        if uri.starts_with("trash:///") {
            if let Some(base) = uri.strip_prefix("trash:///") {
                for root in trash_roots {
                    let candidate = root.join("info").join(format!("{}.trashinfo", base));
                    if candidate.exists() {
                        let _ = std::fs::remove_file(&candidate);
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
