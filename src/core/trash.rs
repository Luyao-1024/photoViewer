//! gio 系统回收站包装
//!
//! gio crate 0.19 仅绑定了 `File::trash()`；`restore_from_trash` 与
//! `delete_permanently` 在其公共 API 中未直接暴露。这里采用以下等价流程：
//!
//! * `move_to_trash(uri)`：gio `File::trash()` 处理原路径记录。
//! * `restore_from_trash(uri)`：通过 `~/.local/share/Trash/info/{basename}.trashinfo`
//!   找出 gio 实际使用的回收站内文件名（gio 在重名时会在 basename 后追加
//!   `.N` 后缀），再 `File::move_` 到原 file:// 路径，并清理对应的 `.trashinfo`
//!   元数据文件。
//! * `delete_permanently(uri)`：同样的方式定位实际回收站项，调用 `File::delete()`
//!   并清理 `.trashinfo` 元数据文件。
//!
//! 所有错误最终归并为 `AppError::Gio(glib::Error)` 或 `AppError::Io(...)`。
use crate::core::error::{AppError, Result};
use gtk::gio::prelude::*;
use gtk4 as gtk;
use std::path::{Path, PathBuf};

/// XDG trash 根目录（`$XDG_DATA_HOME/Trash` 或 `$HOME/.local/share/Trash`）
fn trash_root() -> PathBuf {
    if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        let p = PathBuf::from(data);
        if !p.as_os_str().is_empty() {
            return p.join("Trash");
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".local/share/Trash");
    }
    PathBuf::from("/tmp/Trash")
}

/// 在 `~/.local/share/Trash/info/` 中解析出对应 `original_path` 的回收站实际文件名。
///
/// gio 在将 `foo.jpg` 移到回收站时，如果同名文件已存在会改名为 `foo.2.jpg`、
/// `foo.3.jpg`... 并在 `info/foo.jpg.trashinfo`、`info/foo.2.jpg.trashinfo` 中
/// 记录原路径。`Path=` 字段是原文件的绝对路径。
///
/// 返回 `(files_dir 中的实际文件名, info_dir 中的 trashinfo 文件路径)`。
fn resolve_trash_entry(original_path: &Path, original_basename: &str) -> Result<(String, PathBuf)> {
    let info_dir = trash_root().join("info");
    let files_dir = trash_root().join("files");

    // 候选 trashinfo 路径：
    // 1. 直接以原始 basename 命名的（无冲突场景）
    // 2. basename.N.trashinfo（N=2..max_suffix），gio 重名时的递增后缀
    let mut candidates: Vec<PathBuf> = Vec::new();
    candidates.push(info_dir.join(format!("{}.trashinfo", original_basename)));
    let max_suffix = 1000usize;
    for n in 2..=max_suffix {
        candidates.push(info_dir.join(format!("{}.{}.trashinfo", original_basename, n)));
    }

    for info_path in &candidates {
        let Ok(content) = std::fs::read_to_string(info_path) else {
            continue;
        };
        if let Some(path_line) = content.lines().find(|l| l.starts_with("Path=")) {
            let recorded = &path_line["Path=".len()..];
            if Path::new(recorded) == original_path {
                // 从 info 文件名推回 files 中的实际文件名
                // e.g. "foo.jpg.trashinfo" -> "foo.jpg"
                //      "foo.2.jpg.trashinfo" -> "foo.2.jpg"
                let file_name_os = info_path
                    .file_name()
                    .ok_or_else(|| AppError::Backend("trashinfo missing filename".into()))?;
                let file_name = file_name_os
                    .to_str()
                    .ok_or_else(|| AppError::Backend("trashinfo filename not utf8".into()))?;
                let actual = file_name
                    .strip_suffix(".trashinfo")
                    .ok_or_else(|| AppError::Backend("trashinfo missing .trashinfo suffix".into()))?
                    .to_string();
                // 校验 files 目录里也确实存在该条目（防御性检查）
                let in_files = files_dir.join(&actual);
                if in_files.exists() {
                    return Ok((actual, info_path.clone()));
                }
                // 找不到 files 时继续尝试（gio 信息可能与其他 trash 实现不一致）
            }
        }
    }

    // 兜底：使用原始 basename（无冲突路径，gio 不创建 .trashinfo 时的兼容情况）
    Ok((
        original_basename.to_string(),
        info_dir.join(format!("{}.trashinfo", original_basename)),
    ))
}

/// 将文件移至系统回收站（gio 自动处理原路径记录）
pub fn move_to_trash(uri: &str) -> Result<()> {
    let file = gtk::gio::File::for_uri(uri);
    file.trash(gtk::gio::Cancellable::NONE)
        .map_err(AppError::Gio)?;
    Ok(())
}

/// 从回收站还原到原路径
///
/// `uri` 必须是 `move_to_trash` 时传入的原文件 uri（`file://...`）。
pub fn restore_from_trash(uri: &str) -> Result<()> {
    let file = gtk::gio::File::for_uri(uri);
    let path = file
        .path()
        .ok_or_else(|| AppError::Backend(format!("uri {} has no local path", uri)))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| AppError::Backend("orig path has no filename".into()))?
        .to_string();

    let (actual_name, trashinfo_path) = resolve_trash_entry(&path, &name)?;
    let trash_root_uri = "trash:///";
    let trash_child = gtk::gio::File::for_uri(trash_root_uri).child(&actual_name);
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
    // 仅在 trashinfo 存在且 Path 字段匹配时才删除（避免误删）
    if trashinfo_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&trashinfo_path) {
            let matches = content
                .lines()
                .find(|l| l.starts_with("Path="))
                .map(|l| Path::new(&l["Path=".len()..]) == path)
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
/// * `file://...` —— 与 `move_to_trash` 时一致；函数会定位 trash:/// 中对应
///   的实际子项（处理 basename 冲突后缀）并删除它（永久删除）。
/// * `trash:///...` —— 直接删除 trash 项；如 basename 含 `.trashinfo` 信息，
///   也一并清理对应元数据文件。
pub fn delete_permanently(uri: &str) -> Result<()> {
    if let Some(rest) = uri.strip_prefix("file://") {
        // 提取 basename，再解析实际回收站文件名
        let path = Path::new(rest);
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| AppError::Backend("uri has no filename".into()))?
            .to_string();
        let (actual_name, trashinfo_path) = resolve_trash_entry(path, &name)?;
        let trash_child = gtk::gio::File::for_uri("trash:///").child(&actual_name);
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
        // 若传入的是 trash:///...，同步清理可能的 .trashinfo
        if uri.starts_with("trash:///") {
            if let Some(base) = uri.strip_prefix("trash:///") {
                let candidate = trash_root()
                    .join("info")
                    .join(format!("{}.trashinfo", base));
                if candidate.exists() {
                    let _ = std::fs::remove_file(&candidate);
                }
            }
        }
        Ok(())
    }
}
