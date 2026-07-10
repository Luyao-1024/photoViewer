use super::*;
use std::ffi::OsString;
use std::io::Write;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct CapturedLog(Arc<Mutex<Vec<u8>>>);

struct CapturedLogWriter(Arc<Mutex<Vec<u8>>>);

impl Write for CapturedLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLog {
    type Writer = CapturedLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        CapturedLogWriter(self.0.clone())
    }
}

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
            "/home/luyao/.var/app/io.github.luyao_1024.photoviewer/data",
        )),
        Some(OsString::from("/home/luyao")),
    );
    assert_eq!(roots[0], PathBuf::from("/home/luyao/.local/share/Trash"));
    assert!(roots.contains(&PathBuf::from(
        "/home/luyao/.var/app/io.github.luyao_1024.photoviewer/data/Trash"
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
            video_duration_secs: None,
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
            video_duration_secs: None,
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

#[test]
fn app_trash_root_entry_can_resolve_restore_and_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let trash_root = tmp.path().join("AppTrash");
    let original = tmp.path().join("pictures").join("app-trash.jpg");
    std::fs::create_dir_all(original.parent().unwrap()).unwrap();
    std::fs::write(&original, b"app trash data").unwrap();
    let uri = format!("file://{}", original.display());

    move_path_to_trash_root(&original, &trash_root).unwrap();
    assert!(!original.exists(), "move should remove the original file");

    let trashed = trashed_file_uri_in_roots(&uri, std::slice::from_ref(&trash_root)).unwrap();
    assert!(
        std::path::Path::new(trashed.strip_prefix("file://").unwrap()).exists(),
        "trashed file URI should point at the app trash files directory"
    );

    restore_from_trash_in_roots(&uri, std::slice::from_ref(&trash_root)).unwrap();
    assert_eq!(std::fs::read(&original).unwrap(), b"app trash data");
    assert!(
        find_trash_entry_in(&original, std::slice::from_ref(&trash_root)).is_none(),
        "restore should remove the app trashinfo entry"
    );

    move_path_to_trash_root(&original, &trash_root).unwrap();
    delete_permanently_in_roots(&uri, std::slice::from_ref(&trash_root)).unwrap();
    assert!(
        find_trash_entry_in(&original, &[trash_root]).is_none(),
        "permanent delete should remove app trash files and metadata"
    );
}

#[test]
fn migrate_trashed_entries_moves_metadata_between_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&tmp.path().join("test.db")).unwrap();
    let pictures = tmp.path().join("pictures");
    std::fs::create_dir_all(&pictures).unwrap();
    let original = pictures.join("migrated.jpg");
    let system_root = tmp.path().join("SystemTrash");
    let app_root = tmp.path().join("AppTrash");
    plant_trash_entry(&system_root, "migrated.jpg", &original);

    let id = db::insert_media_item(
        &pool,
        &crate::core::media::NewMediaItem {
            uri: format!("file://{}", original.display()),
            path: original.clone(),
            folder_path: pictures,
            mime_type: "image/jpeg".into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: Some(1),
            height: Some(1),
            video_duration_secs: None,
            taken_at: None,
            file_mtime: chrono::Utc::now(),
            file_size: 1,
            blake3_hash: "h".into(),
        },
    )
    .unwrap();
    db::mark_trashed(&pool, id).unwrap();

    let stats = migrate_trash_entries_between_roots(
        &pool,
        std::slice::from_ref(&system_root),
        &app_root,
        &[],
    )
    .unwrap();

    assert_eq!(stats.moved, 1);
    assert!(
        find_trash_entry_in(&original, &[system_root]).is_none(),
        "migration should remove the old system trash entry"
    );
    assert!(
        find_trash_entry_in(&original, &[app_root]).is_some(),
        "migration should create the app trash entry"
    );
}

#[test]
fn migrate_trashed_entries_skips_missing_source_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&tmp.path().join("test.db")).unwrap();
    let pictures = tmp.path().join("pictures");
    std::fs::create_dir_all(&pictures).unwrap();
    let original = pictures.join("missing-source.jpg");
    let system_root = tmp.path().join("SystemTrash");
    let app_root = tmp.path().join("AppTrash");

    let id = db::insert_media_item(
        &pool,
        &crate::core::media::NewMediaItem {
            uri: format!("file://{}", original.display()),
            path: original.clone(),
            folder_path: pictures,
            mime_type: "image/jpeg".into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: Some(1),
            height: Some(1),
            video_duration_secs: None,
            taken_at: None,
            file_mtime: chrono::Utc::now(),
            file_size: 1,
            blake3_hash: "h".into(),
        },
    )
    .unwrap();
    db::mark_trashed(&pool, id).unwrap();

    let stats = migrate_trash_entries_between_roots(
        &pool,
        std::slice::from_ref(&system_root),
        &app_root,
        &[],
    )
    .unwrap();

    assert_eq!(stats.moved, 0);
    assert_eq!(stats.skipped, 1);
    assert!(
        find_trash_entry_in(&original, &[app_root]).is_none(),
        "missing source entries cannot be copied into the target backend"
    );
}

#[test]
fn startup_backend_falls_back_to_app_only_when_system_is_configured_and_unavailable() {
    assert_eq!(
        startup_backend_after_probe(TrashBackend::System, Ok(())),
        TrashBackend::System
    );
    assert_eq!(
        startup_backend_after_probe(
            TrashBackend::System,
            Err("trash portal unavailable".to_string())
        ),
        TrashBackend::App
    );
    assert_eq!(
        startup_backend_after_probe(TrashBackend::App, Ok(())),
        TrashBackend::App,
        "startup should not automatically migrate an explicit app-trash preference back to system"
    );
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
fn reconcile_skips_unsupported_trash_file_without_decode_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let pictures = tmp.path().join("pictures");
    std::fs::create_dir_all(pictures.join("Camera")).unwrap();
    let trash_root = tmp.path().join("Trash");
    let info_dir = trash_root.join("info");
    let files_dir = trash_root.join("files");
    std::fs::create_dir_all(&info_dir).unwrap();
    std::fs::create_dir_all(&files_dir).unwrap();

    let original = pictures.join("Camera").join("notes.txt");
    std::fs::write(
        info_dir.join("notes.txt.trashinfo"),
        format!(
            "[Trash Info]\nPath={}\nDeletionDate=2026-07-09T00:00:00\n",
            original.display()
        ),
    )
    .unwrap();
    std::fs::write(files_dir.join("notes.txt"), b"not an indexed media file").unwrap();

    let pool = db::init_pool(&tmp.path().join("t.db")).unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_max_level(tracing::Level::WARN)
        .with_writer(CapturedLog(captured.clone()))
        .finish();

    let stats = tracing::subscriber::with_default(subscriber, || {
        reconcile_trash_in(&pool, &pictures, &[trash_root]).unwrap()
    });

    assert_eq!(stats.inserted, 0);
    assert_eq!(stats.skipped, 1);
    assert!(db::list_trashed_media(&pool).unwrap().is_empty());
    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(
        !logs.contains("回收站对账：解析"),
        "unsupported trash entries should be skipped before decode, got logs: {logs}"
    );
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
    let stats = reconcile_trash_in(&pool, &pictures, std::slice::from_ref(&trash_root)).unwrap();

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
    let stats = reconcile_trash_in(&pool, &pictures, std::slice::from_ref(&trash_root)).unwrap();

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
            video_duration_secs: None,
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
    let first = reconcile_trash_in(&pool, &pictures, std::slice::from_ref(&trash_root)).unwrap();
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
            video_duration_secs: None,
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
            video_duration_secs: None,
            taken_at: None,
            file_mtime: chrono::Utc::now(),
            file_size: 1,
            blake3_hash: "h".into(),
        },
    )
    .unwrap();
    db::mark_trashed(&pool, id).unwrap();

    let stats = reconcile_trash_in(&pool, &pictures, std::slice::from_ref(&trash_root)).unwrap();
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
            video_duration_secs: None,
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
