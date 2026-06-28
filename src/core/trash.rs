//! gio 系统回收站包装
//!
//! gio crate 0.19 仅绑定了 `File::trash()`；`restore_from_trash` 与
//! `delete_permanently` 在其公共 API 中未直接暴露。这里采用以下等价流程：
//!
//! * `move_to_trash(uri)`：gio `File::trash()` 处理原路径记录。
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
use crate::core::backend::local::LocalBackend;
use crate::core::db::{self, DbPool};
use crate::core::error::{AppError, Result};
use gtk::gio::prelude::*;
use gtk4 as gtk;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// 待探测的回收站根目录，按优先级返回。
///
/// 见模块文档：Flatpak 下实际落点是 HOST `~/.local/share/Trash`，但同时保留
/// per-app `$XDG_DATA_HOME/Trash` 作为候选，兼容非沙箱或自定义 `XDG_DATA_HOME`。
pub fn trash_roots() -> Vec<PathBuf> {
    trash_roots_from(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
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
fn find_trash_entry(original_path: &Path) -> Option<(String, PathBuf)> {
    find_trash_entry_in(original_path, &trash_roots())
}

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
fn resolve_trash_entry(original_path: &Path) -> Result<(String, PathBuf)> {
    if let Some(found) = find_trash_entry(original_path) {
        return Ok(found);
    }
    let root = trash_roots()
        .into_iter()
        .next()
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
    let file = gtk::gio::File::for_uri(uri);
    let path = file
        .path()
        .ok_or_else(|| AppError::Backend(format!("uri {} has no local path", uri)))?;
    let (actual_name, info_path) = resolve_trash_entry(&path)?;
    Ok(format!(
        "file://{}",
        files_dir_for(&info_path).join(actual_name).display()
    ))
}

/// 将文件移至系统回收站（gio 自动处理原路径记录）
pub fn move_to_trash(uri: &str) -> Result<()> {
    let file = gtk::gio::File::for_uri(uri);
    file.trash(gtk::gio::Cancellable::NONE)
        .map_err(AppError::Gio)?;
    Ok(())
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
    match move_to_trash(uri) {
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
    reconcile_trash_in(pool, pictures_root, &trash_roots())
}

/// [`reconcile_trash`] 的可测试核心：显式传入候选回收站根，避免单测依赖真实
/// `~/.local/share/Trash` 与环境变量。
fn reconcile_trash_in(
    pool: &DbPool,
    pictures_root: &Path,
    trash_roots: &[PathBuf],
) -> Result<ReconcileStats> {
    let mut stats = ReconcileStats::default();
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
                stats.skipped += 1;
                continue;
            }
            // 原路径仍存在 → 已还原/还在原位，保持 live，不标 trashed。
            if original_path.exists() {
                stats.skipped += 1;
                continue;
            }
            let trash_file = files_dir.join(actual);
            if !trash_file.is_file() {
                stats.skipped += 1;
                continue;
            }

            let uri = format!("file://{}", original_path.display());
            match db::get_media_item_by_uri(pool, &uri)? {
                Some(existing) if existing.trashed_at.is_some() => {
                    stats.skipped += 1; // 已是 trashed，无需处理
                }
                Some(existing) => {
                    db::mark_trashed(pool, existing.id)?;
                    stats.marked += 1;
                }
                None => {
                    let folder = original_path.parent().unwrap_or_else(|| Path::new("/"));
                    match backend.process_file_at(&trash_file, &uri, &original_path, folder) {
                        Ok(item) => {
                            let id = db::insert_media_item(pool, &item)?;
                            db::mark_trashed(pool, id)?;
                            stats.inserted += 1;
                        }
                        Err(e) => {
                            // 非图片 / 无法解码：跳过，不影响其余条目。
                            tracing::warn!("回收站对账：解析 {} 失败: {}", trash_file.display(), e);
                            stats.skipped += 1;
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
            if let Err(e) = db::delete_media_item(pool, row.id) {
                tracing::warn!("回收站对账：删除过期行 {} 失败: {}", row.id, e);
            } else {
                stats.pruned += 1;
            }
        }
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
    let file = gtk::gio::File::for_uri(uri);
    let path = file
        .path()
        .ok_or_else(|| AppError::Backend(format!("uri {} has no local path", uri)))?;

    let (actual_name, trashinfo_path) = resolve_trash_entry(&path)?;
    let trash_child = gtk::gio::File::for_path(files_dir_for(&trashinfo_path).join(&actual_name));
    let target = gtk::gio::File::for_path(&path);

    // 先移动数据文件
    trash_child
        .move_(
            &target,
            gtk::gio::FileCopyFlags::OVERWRITE,
            gtk::gio::Cancellable::NONE,
            None,
        )
        .map_err(AppError::Gio)?;

    // 再清理对应的 .trashinfo（gio 不再负责）
    // 仅在 trashinfo 存在且 Path 字段（decode 后）匹配时才删除（避免误删）
    if trashinfo_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&trashinfo_path) {
            let matches = content
                .lines()
                .find(|l| l.starts_with("Path="))
                .map(|l| Path::new(&percent_decode(&l["Path=".len()..])) == path)
                .unwrap_or(false);
            if matches {
                let _ = std::fs::remove_file(&trashinfo_path);
            }
        }
    }

    Ok(())
}

/// 永久删除回收站中的文件
///
/// `uri` 接受两种形式：
/// * `file://...` —— 与 `move_to_trash` 时一致；函数会定位对应实际子项
///   （处理 basename 冲突后缀）并删除它（永久删除）。
/// * `trash:///...` —— 直接删除 trash 项；如 basename 含 `.trashinfo` 信息，
///   也一并清理对应元数据文件。
pub fn delete_permanently(uri: &str) -> Result<()> {
    if let Some(rest) = uri.strip_prefix("file://") {
        // 提取 basename，再解析实际回收站文件名
        let path = Path::new(rest);
        let (actual_name, trashinfo_path) = resolve_trash_entry(path)?;
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
                for root in trash_roots() {
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
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn percent_decode_handles_utf8_and_passthrough() {
        // gio 写入的非 ASCII 路径是 UTF-8 percent-encoding。
        assert_eq!(
            percent_decode("/home/luyao/%E5%9B%BE%E7%89%87/x.heic"),
            "/home/luyao/图片/x.heic"
        );
        // 纯 ASCII 未编码路径必须恒等往返（测试里手写的 trashinfo 没编码）。
        assert_eq!(
            percent_decode("/home/luyao/Pictures/a.jpg"),
            "/home/luyao/Pictures/a.jpg"
        );
        // 非法/不完整转义原样保留，不 panic。
        assert_eq!(percent_decode("100% done"), "100% done");
        assert_eq!(percent_decode("%ZZ"), "%ZZ");
        assert_eq!(percent_decode("%3"), "%3");
        // 大小写 hex 都接受。
        assert_eq!(percent_decode("%2f"), "/");
        assert_eq!(percent_decode("%2F"), "/");
    }

    #[test]
    fn trash_roots_includes_host_home_trash_alongside_per_app_xdg() {
        // Flatpak 情形：HOME 是真实家目录，XDG_DATA_HOME 指向 per-app 目录。
        // 必须同时给出 HOST 回收站根（gio 实际落点）和 per-app 根。
        let roots = trash_roots_from(
            Some(OsString::from(
                "/home/luyao/.var/app/org.gnome.PhotoViewer/data",
            )),
            Some(OsString::from("/home/luyao")),
        );
        assert_eq!(roots[0], PathBuf::from("/home/luyao/.local/share/Trash"));
        assert!(roots.contains(&PathBuf::from(
            "/home/luyao/.var/app/org.gnome.PhotoViewer/data/Trash"
        )));
    }

    #[test]
    fn trash_roots_host_only_when_no_xdg_override() {
        // 非 Flatpak、未自定义 XDG_DATA_HOME：只有 HOST 回收站根，且不重复。
        let roots = trash_roots_from(None, Some(OsString::from("/home/luyao")));
        assert_eq!(roots, vec![PathBuf::from("/home/luyao/.local/share/Trash")]);
    }

    #[test]
    fn files_dir_derived_from_matched_trashinfo_root() {
        // info_path 在哪个根，files_dir 就该在同一个根，避免跨根拼错路径。
        let info = PathBuf::from("/home/luyao/.local/share/Trash/info/x.jpg.trashinfo");
        assert_eq!(
            files_dir_for(&info),
            PathBuf::from("/home/luyao/.local/share/Trash/files")
        );
        let info2 = PathBuf::from("/app/data/Trash/info/x.jpg.trashinfo");
        assert_eq!(
            files_dir_for(&info2),
            PathBuf::from("/app/data/Trash/files")
        );
    }

    /// 选取 gio 可支持的真实文件系统路径（拒绝 tmpfs）。
    fn real_scratch_base() -> PathBuf {
        std::env::var_os("TMPDIR_REAL")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/var/tmp"))
    }

    /// 回收站流程必须让被删项在文件监听器的 Remove 事件下存活——这是
    /// "删多张、回收站只剩一张"的根因防护：`move_to_trash_marked` 先 `mark_trashed`
    /// 再移动，监听器随后按 Remove 事件调 `delete_media_by_path` 时行已是 trashed，
    /// 必须被跳过（`AND trashed_at IS NULL`）。
    #[test]
    fn move_to_trash_marked_survives_watcher_remove_event() {
        let base = real_scratch_base();
        let dir = tempfile::tempdir_in(&base).unwrap();
        let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

        let real_path = base.join(format!("pv-trash-flow-{}.jpg", std::process::id()));
        std::fs::write(&real_path, b"x").unwrap();
        let uri = format!("file://{}", real_path.display());
        let id = db::insert_media_item(
            &pool,
            &crate::core::media::NewMediaItem {
                uri: uri.clone(),
                path: real_path.clone(),
                folder_path: base.clone(),
                mime_type: "image/jpeg".into(),
                media_subkind: "standard".into(),
                media_attributes: "{}".into(),
                width: Some(1),
                height: Some(1),
                taken_at: None,
                file_mtime: chrono::Utc::now(),
                file_size: 1,
                blake3_hash: "h".into(),
            },
        )
        .unwrap();

        // 真实回收站流程：先标记后移动
        move_to_trash_marked(&pool, id, &uri).unwrap();
        assert!(!real_path.exists(), "file should have been moved to trash");

        // 模拟文件监听器收到原路径的 Remove 事件
        let changed = db::delete_media_by_path(&pool, &real_path).unwrap();
        assert_eq!(
            changed, 0,
            "watcher must not delete a row the trash flow marked trashed"
        );
        assert_eq!(
            db::list_trashed_media(&pool).unwrap().len(),
            1,
            "trashed row must survive the watcher's remove event"
        );

        // 清理 host trash 里本次产生的文件
        let _ = delete_permanently(&uri);
    }

    /// 移动失败时必须回滚 `mark_trashed`，否则行会变成"已标记回收站但文件还在"
    /// 的幽灵状态（回收站里出现打不开的项）。
    #[test]
    fn move_to_trash_marked_rolls_back_when_move_fails() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
        // 一个不存在的文件 uri —— gio::File::trash 会失败
        let path = dir
            .path()
            .join(format!("no-such-{}.jpg", std::process::id()));
        let uri = format!("file://{}", path.display());
        let id = db::insert_media_item(
            &pool,
            &crate::core::media::NewMediaItem {
                uri: uri.clone(),
                path: path.clone(),
                folder_path: dir.path().to_path_buf(),
                mime_type: "image/jpeg".into(),
                media_subkind: "standard".into(),
                media_attributes: "{}".into(),
                width: Some(1),
                height: Some(1),
                taken_at: None,
                file_mtime: chrono::Utc::now(),
                file_size: 1,
                blake3_hash: "h".into(),
            },
        )
        .unwrap();

        let result = move_to_trash_marked(&pool, id, &uri);
        assert!(result.is_err(), "trashing a nonexistent file should fail");
        let item = db::get_media_item(&pool, id).unwrap();
        assert!(
            item.trashed_at.is_none(),
            "failed move must roll back the trash marker so the row stays live"
        );
        assert_eq!(db::list_trashed_media(&pool).unwrap().len(), 0);
    }

    // ── reconcile_trash 对账 ──────────────────────────────────────────────

    /// 写一个真实（极小）JPEG 到 `path`，供 metadata::extract 成功解析。
    fn write_jpeg(path: &std::path::Path) {
        use image::{ImageBuffer, Rgb};
        let img = ImageBuffer::<Rgb<u8>, _>::from_fn(8, 8, |_, _| Rgb([10, 20, 30]));
        img.save(path).unwrap();
    }

    /// 在 `trash_root` 下构造一条回收站条目：`info/<actual>.trashinfo`（Path= 指向
    /// `original`）+ `files/<actual>`（真实 JPEG 副本）。
    fn plant_trash_entry(trash_root: &std::path::Path, actual: &str, original: &std::path::Path) {
        let info_dir = trash_root.join("info");
        let files_dir = trash_root.join("files");
        std::fs::create_dir_all(&info_dir).unwrap();
        std::fs::create_dir_all(&files_dir).unwrap();
        std::fs::write(
            info_dir.join(format!("{actual}.trashinfo")),
            format!(
                "[Trash Info]\nPath={}\nDeletionDate=2026-06-26T00:00:00\n",
                original.display()
            ),
        )
        .unwrap();
        write_jpeg(&files_dir.join(actual));
    }

    #[test]
    fn reconcile_inserts_orphan_whose_original_was_under_pictures() {
        let tmp = tempfile::tempdir().unwrap();
        let pictures = tmp.path().join("pictures");
        std::fs::create_dir_all(pictures.join("Camera")).unwrap();
        let trash_root = tmp.path().join("Trash");
        // 原路径在 pictures 下、原位不存在（已删）；回收站里有副本
        let original = pictures.join("Camera").join("orphan.jpg");
        plant_trash_entry(&trash_root, "orphan.jpg", &original);

        let pool = db::init_pool(&tmp.path().join("t.db")).unwrap();
        let stats =
            reconcile_trash_in(&pool, &pictures, std::slice::from_ref(&trash_root)).unwrap();

        assert_eq!(stats.inserted, 1);
        let trashed = db::list_trashed_media(&pool).unwrap();
        assert_eq!(
            trashed.len(),
            1,
            "orphan under pictures must be inserted as trashed"
        );
        assert_eq!(trashed[0].uri, format!("file://{}", original.display()));
        assert!(trashed[0].path.ends_with("orphan.jpg"));
    }

    #[test]
    fn reconcile_skips_orphan_whose_original_was_outside_pictures() {
        let tmp = tempfile::tempdir().unwrap();
        let pictures = tmp.path().join("pictures");
        std::fs::create_dir_all(&pictures).unwrap();
        let trash_root = tmp.path().join("Trash");
        // 原路径在 pictures 之外（如下载目录）——必须忽略，不进 DB
        let original = tmp.path().join("Downloads").join("elsewhere.jpg");
        plant_trash_entry(&trash_root, "elsewhere.jpg", &original);

        let pool = db::init_pool(&tmp.path().join("t.db")).unwrap();
        let stats =
            reconcile_trash_in(&pool, &pictures, std::slice::from_ref(&trash_root)).unwrap();

        assert_eq!(stats.inserted, 0);
        assert!(
            db::list_trashed_media(&pool).unwrap().is_empty(),
            "files not from the pictures library must not be added"
        );
    }

    #[test]
    fn reconcile_marks_existing_live_row_when_file_is_in_trash() {
        let tmp = tempfile::tempdir().unwrap();
        let pictures = tmp.path().join("pictures");
        std::fs::create_dir_all(pictures.join("Camera")).unwrap();
        let trash_root = tmp.path().join("Trash");
        let original = pictures.join("Camera").join("externally-deleted.jpg");
        plant_trash_entry(&trash_root, "externally-deleted.jpg", &original);

        let pool = db::init_pool(&tmp.path().join("t.db")).unwrap();
        // DB 里已有一条 live 行（历史索引过，后来文件被外部删到回收站）
        let live_id = db::insert_media_item(
            &pool,
            &crate::core::media::NewMediaItem {
                uri: format!("file://{}", original.display()),
                path: original.clone(),
                folder_path: pictures.join("Camera"),
                mime_type: "image/jpeg".into(),
                media_subkind: "standard".into(),
                media_attributes: "{}".into(),
                width: Some(1),
                height: Some(1),
                taken_at: None,
                file_mtime: chrono::Utc::now(),
                file_size: 1,
                blake3_hash: "h".into(),
            },
        )
        .unwrap();

        let stats = reconcile_trash_in(&pool, &pictures, &[trash_root]).unwrap();
        assert_eq!(stats.marked, 1);
        assert!(
            db::get_media_item(&pool, live_id)
                .unwrap()
                .trashed_at
                .is_some(),
            "live row whose file is in trash must be marked trashed"
        );
    }

    #[test]
    fn reconcile_is_idempotent_and_skips_already_trashed() {
        let tmp = tempfile::tempdir().unwrap();
        let pictures = tmp.path().join("pictures");
        std::fs::create_dir_all(pictures.join("Camera")).unwrap();
        let trash_root = tmp.path().join("Trash");
        let original = pictures.join("Camera").join("dup.jpg");
        plant_trash_entry(&trash_root, "dup.jpg", &original);

        let pool = db::init_pool(&tmp.path().join("t.db")).unwrap();
        let first =
            reconcile_trash_in(&pool, &pictures, std::slice::from_ref(&trash_root)).unwrap();
        assert_eq!(first.inserted, 1);
        // 第二次跑：行已是 trashed → 跳过，绝不重复插入
        let second = reconcile_trash_in(&pool, &pictures, &[trash_root]).unwrap();
        assert_eq!(second.inserted, 0);
        assert_eq!(db::list_trashed_media(&pool).unwrap().len(), 1);
    }

    #[test]
    fn reconcile_skips_when_original_path_still_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let pictures = tmp.path().join("pictures");
        std::fs::create_dir_all(pictures.join("Camera")).unwrap();
        let trash_root = tmp.path().join("Trash");
        let original = pictures.join("Camera").join("restored.jpg");
        plant_trash_entry(&trash_root, "restored.jpg", &original);
        // 原位文件还在（已还原）→ 必须保持 live，不标 trashed
        write_jpeg(&original);

        let pool = db::init_pool(&tmp.path().join("t.db")).unwrap();
        let stats = reconcile_trash_in(&pool, &pictures, &[trash_root]).unwrap();
        assert_eq!(stats.inserted, 0);
        assert!(
            db::list_trashed_media(&pool).unwrap().is_empty(),
            "a file still present at its original path must not be trashed"
        );
    }

    // ── reconcile_trash 对账：删（prune）方向 ──────────────────────────────

    /// 插一条 trashed 行，但系统回收站里已无对应文件（外部清空/永久删除）→ 必须删除，
    /// 否则回收站视图残留打不开缩略图的死项。
    #[test]
    fn reconcile_prunes_trashed_row_absent_from_system_trash() {
        let tmp = tempfile::tempdir().unwrap();
        let pictures = tmp.path().join("pictures");
        std::fs::create_dir_all(pictures.join("Camera")).unwrap();
        let pool = db::init_pool(&tmp.path().join("t.db")).unwrap();
        // 原位文件不存在、回收站里也没有该条目
        let original = pictures.join("Camera").join("emptied.jpg");
        let id = db::insert_media_item(
            &pool,
            &crate::core::media::NewMediaItem {
                uri: format!("file://{}", original.display()),
                path: original.clone(),
                folder_path: pictures.join("Camera"),
                mime_type: "image/jpeg".into(),
                media_subkind: "standard".into(),
                media_attributes: "{}".into(),
                width: Some(1),
                height: Some(1),
                taken_at: None,
                file_mtime: chrono::Utc::now(),
                file_size: 1,
                blake3_hash: "h".into(),
            },
        )
        .unwrap();
        db::mark_trashed(&pool, id).unwrap();
        assert_eq!(db::list_trashed_media(&pool).unwrap().len(), 1);

        let stats = reconcile_trash_in(&pool, &pictures, &[tmp.path().join("Trash")]).unwrap();
        assert_eq!(stats.pruned, 1);
        assert!(
            db::list_trashed_media(&pool).unwrap().is_empty(),
            "trashed row whose file is gone from the system trash must be pruned"
        );
    }

    #[test]
    fn reconcile_keeps_trashed_row_that_is_still_in_system_trash() {
        let tmp = tempfile::tempdir().unwrap();
        let pictures = tmp.path().join("pictures");
        std::fs::create_dir_all(pictures.join("Camera")).unwrap();
        let trash_root = tmp.path().join("Trash");
        let original = pictures.join("Camera").join("kept.jpg");
        plant_trash_entry(&trash_root, "kept.jpg", &original);

        let pool = db::init_pool(&tmp.path().join("t.db")).unwrap();
        let id = db::insert_media_item(
            &pool,
            &crate::core::media::NewMediaItem {
                uri: format!("file://{}", original.display()),
                path: original.clone(),
                folder_path: pictures.join("Camera"),
                mime_type: "image/jpeg".into(),
                media_subkind: "standard".into(),
                media_attributes: "{}".into(),
                width: Some(1),
                height: Some(1),
                taken_at: None,
                file_mtime: chrono::Utc::now(),
                file_size: 1,
                blake3_hash: "h".into(),
            },
        )
        .unwrap();
        db::mark_trashed(&pool, id).unwrap();

        let stats =
            reconcile_trash_in(&pool, &pictures, std::slice::from_ref(&trash_root)).unwrap();
        assert_eq!(stats.pruned, 0);
        assert!(
            db::get_media_item(&pool, id).is_ok(),
            "trashed row still present in the system trash must be kept"
        );
    }

    /// 已还原（原位文件还在）的 trashed 行：即便回收站里没条目了，也不能在这里删——
    /// 启动扫描会把它重新 upsert 成 live。reconcile 必须放过的。
    #[test]
    fn reconcile_does_not_prune_restored_row_whose_original_is_present() {
        let tmp = tempfile::tempdir().unwrap();
        let pictures = tmp.path().join("pictures");
        std::fs::create_dir_all(pictures.join("Camera")).unwrap();
        let pool = db::init_pool(&tmp.path().join("t.db")).unwrap();
        let original = pictures.join("Camera").join("restored-prune.jpg");
        write_jpeg(&original); // 原位文件在 → 已还原
        let id = db::insert_media_item(
            &pool,
            &crate::core::media::NewMediaItem {
                uri: format!("file://{}", original.display()),
                path: original.clone(),
                folder_path: pictures.join("Camera"),
                mime_type: "image/jpeg".into(),
                media_subkind: "standard".into(),
                media_attributes: "{}".into(),
                width: Some(1),
                height: Some(1),
                taken_at: None,
                file_mtime: chrono::Utc::now(),
                file_size: 1,
                blake3_hash: "h".into(),
            },
        )
        .unwrap();
        db::mark_trashed(&pool, id).unwrap();

        let stats = reconcile_trash_in(&pool, &pictures, &[tmp.path().join("Trash")]).unwrap();
        assert_eq!(stats.pruned, 0);
        assert!(
            db::get_media_item(&pool, id).is_ok(),
            "a restored row (original present) must not be pruned by reconcile"
        );
    }
}
