//! gio 系统回收站包装
//!
//! gio crate 0.19 仅绑定了 `File::trash()`；`restore_from_trash` 与
//! `delete_permanently` 在其公共 API 中未直接暴露。这里采用以下等价流程：
//!
//! * `restore_from_trash(uri)`：根据 uri 的 basename 拼出 `trash:///NAME`
//!   子项，然后 `move_` 到原始 file:// 路径。
//! * `delete_permanently(uri)`：`File::delete()`。若 uri 指向 trash:/// 项，
//!   gio 走 gvfsd-trash 永久删除路径；否则等同于普通文件删除。
//!
//! 所有错误最终归并为 `AppError::Gio(glib::Error)`。
use crate::core::error::{AppError, Result};
use gtk::gio::prelude::*;
use gtk4 as gtk;

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
        .ok_or_else(|| AppError::Backend("orig path has no filename".into()))?;

    let trash_root = gtk::gio::File::for_uri("trash:///");
    let trash_child = trash_root.child(name);
    let target = gtk::gio::File::for_path(&path);
    trash_child
        .move_(
            &target,
            gtk::gio::FileCopyFlags::OVERWRITE,
            gtk::gio::Cancellable::NONE,
            None,
        )
        .map_err(AppError::Gio)?;
    Ok(())
}

/// 永久删除回收站中的文件
///
/// `uri` 接受两种形式：
/// * `file://...` —— 与 `move_to_trash` 时一致；函数会先 trashing，然后
///   定位 trash:/// 中同名子项并删除它（永久删除）。
/// * `trash:///...` —— 直接删除 trash 项。
pub fn delete_permanently(uri: &str) -> Result<()> {
    if let Some(rest) = uri.strip_prefix("file://") {
        // 提取 basename 作为 trash:/// 子项名
        let name = std::path::Path::new(rest)
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| AppError::Backend("uri has no filename".into()))?;
        let trash_child = gtk::gio::File::for_uri("trash:///").child(name);
        trash_child
            .delete(gtk::gio::Cancellable::NONE)
            .map_err(AppError::Gio)?;
        Ok(())
    } else {
        let file = gtk::gio::File::for_uri(uri);
        file.delete(gtk::gio::Cancellable::NONE)
            .map_err(AppError::Gio)?;
        Ok(())
    }
}