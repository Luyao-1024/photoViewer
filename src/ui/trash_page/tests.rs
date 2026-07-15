use super::*;
use chrono::Utc;
use std::path::PathBuf;

fn loader_for(pool: DbPool) -> Arc<ThumbnailLoader> {
    let dir = tempfile::tempdir().unwrap().keep();
    Arc::new(ThumbnailLoader::new(pool, dir.join("cache")))
}

fn media_item(id: i64) -> MediaItem {
    MediaItem {
        id,
        uri: format!("file:///tmp/{id}.jpg"),
        path: PathBuf::from(format!("/tmp/{id}.jpg")),
        folder_path: PathBuf::from("/tmp"),
        mime_type: "image/jpeg".to_string(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(1),
        height: Some(1),
        video_duration_secs: None,
        taken_at: None,
        file_mtime: Utc::now(),
        file_size: 1,
        blake3_hash: "hash".to_string(),
        is_favorite: false,
        trashed_at: Some(Utc::now()),
    }
}

#[test]
fn selected_indices_map_to_media_item_ids_not_indices() {
    let items = vec![media_item(42), media_item(99), media_item(123)];

    assert_eq!(selected_ids_for_indices(&items, [0, 2]), vec![42, 123]);
}

#[gtk::test]
fn trash_uses_query_backed_virtual_grid() {
    let _ = gtk::init();
    let page = TrashPage::default();
    assert!(page
        .imp()
        .content_stack
        .get()
        .child_by_name("empty")
        .is_some());
    assert!(page.imp().grid.borrow().is_none());
}

#[gtk::test]
fn trash_virtual_grid_keeps_multi_select_enabled() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let page = TrashPage::new(pool.clone(), loader_for(pool));
    let grid = page.imp().grid.borrow().as_ref().cloned().unwrap();
    assert!(grid.is_multi_select_mode());
    assert_eq!(grid.mode(), crate::core::section_model::GroupBy::Day);
}

/// 选取 gio 可支持的真实文件系统路径（拒绝 tmpfs）。
fn real_scratch() -> std::path::PathBuf {
    std::env::var_os("TMPDIR_REAL")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
        .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"))
}

fn pump_until<F: Fn() -> bool>(ctx: &glib::MainContext, max_iters: usize, done: F) {
    for _ in 0..max_iters {
        while ctx.pending() {
            ctx.iteration(false);
        }
        if done() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    while ctx.pending() {
        ctx.iteration(false);
    }
}

/// Insert a single trashed item and wait until the virtual query exposes it.
fn page_with_one_trashed_item() -> (TrashPage, i64, std::path::PathBuf) {
    let _ = gtk::init();
    let ctx = glib::MainContext::default();

    let dir = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();

    let real_dir = real_scratch();
    let real_path = real_dir.join(format!(
        "photo-viewer-trash-restore-test-{}.jpg",
        std::process::id()
    ));
    std::fs::write(&real_path, b"data").unwrap();

    let item = crate::core::media::NewMediaItem {
        uri: format!("file://{}", real_path.display()),
        path: real_path.clone(),
        folder_path: real_dir.clone(),
        mime_type: "image/jpeg".to_string(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(1),
        height: Some(1),
        video_duration_secs: None,
        taken_at: None,
        file_mtime: chrono::Utc::now(),
        file_size: 4,
        blake3_hash: "h".to_string(),
    };
    let uri = item.uri.clone();
    let id = db::insert_media_item(&pool, &item).unwrap();
    trash::move_to_trash(&uri).unwrap();
    db::mark_trashed(&pool, id).unwrap();

    let page = TrashPage::new(pool.clone(), loader_for(pool.clone()));
    let grid = page.imp().grid.borrow().as_ref().cloned().unwrap();
    pump_until(&ctx, 100, || grid.logical_media_count() == 1);
    assert_eq!(
        grid.logical_media_count(),
        1,
        "TrashPage::new should load the one trashed item into VirtualMediaGrid"
    );
    (page, id, real_path)
}

/// Restoring must remove the item from the virtual Trash query immediately.
#[gtk::test]
fn restore_btn_refreshes_virtual_grid_after_restoring_items() {
    let (page, id, real_path) = page_with_one_trashed_item();
    let ctx = glib::MainContext::default();
    let grid = page.imp().grid.borrow().as_ref().cloned().unwrap();

    *page.imp().trashed_ids.borrow_mut() = vec![id];
    page.imp().restore_btn.get().emit_clicked();

    pump_until(&ctx, 200, || grid.logical_media_count() == 0);
    assert_eq!(
        grid.logical_media_count(),
        0,
        "Virtual grid should be empty after restoring the only item"
    );

    let _ = std::fs::remove_file(&real_path);
}

#[gtk::test]
fn header_bar_uses_glass_header() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let page = TrashPage::new(pool.clone(), loader_for(pool));
    let header_classes: Vec<String> = page
        .imp()
        .header_bar
        .get()
        .css_classes()
        .iter()
        .map(|class| class.to_string())
        .collect();

    assert!(
        header_classes.iter().any(|class| class == "glass-header"),
        "TrashPage header should carry glass-header, got {header_classes:?}",
    );
}

#[gtk::test]
fn restore_btn_reinserts_item_into_shared_media_list() {
    let _ = gtk::init();
    let ctx = glib::MainContext::default();

    let dir = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let real_dir = real_scratch();
    let real_path = real_dir.join(format!(
        "photo-viewer-trash-shared-restore-{}.jpg",
        std::process::id()
    ));
    std::fs::write(&real_path, b"data").unwrap();

    let item = crate::core::media::NewMediaItem {
        uri: format!("file://{}", real_path.display()),
        path: real_path.clone(),
        folder_path: real_dir.clone(),
        mime_type: "image/jpeg".to_string(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(1),
        height: Some(1),
        video_duration_secs: None,
        taken_at: None,
        file_mtime: chrono::Utc::now(),
        file_size: 4,
        blake3_hash: "shared-restore".to_string(),
    };
    let uri = item.uri.clone();
    let id = db::insert_media_item(&pool, &item).unwrap();
    trash::move_to_trash(&uri).unwrap();
    db::mark_trashed(&pool, id).unwrap();

    let shared = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let page = TrashPage::with_media_list(pool.clone(), loader_for(pool.clone()), shared.clone());
    let grid = page.imp().grid.borrow().as_ref().cloned().unwrap();
    pump_until(&ctx, 100, || grid.logical_media_count() == 1);
    assert_eq!(grid.logical_media_count(), 1);

    *page.imp().trashed_ids.borrow_mut() = vec![id];
    page.imp().restore_btn.get().emit_clicked();

    pump_until(&ctx, 200, || shared.n_items() == 1);
    assert_eq!(
        shared.n_items(),
        1,
        "restoring from Trash should reinsert the item into the shared Photos model"
    );
    assert_eq!(
        crate::ui::media_list::media_item_at(&shared, 0).map(|item| item.id),
        Some(id)
    );

    let _ = std::fs::remove_file(&real_path);
}

/// Partial permanent delete must retain only the remaining virtual item.
#[gtk::test]
fn delete_btn_refreshes_virtual_grid_after_partial_delete() {
    let _ = gtk::init();
    let ctx = glib::MainContext::default();

    let dir = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let real_dir = real_scratch();

    let path_a = real_dir.join(format!(
        "photo-viewer-trash-del-a-{}.jpg",
        std::process::id()
    ));
    let path_b = real_dir.join(format!(
        "photo-viewer-trash-del-b-{}.jpg",
        std::process::id()
    ));
    std::fs::write(&path_a, b"a").unwrap();
    std::fs::write(&path_b, b"b").unwrap();

    let mk = |p: &std::path::Path, h: &str| -> i64 {
        let item = crate::core::media::NewMediaItem {
            uri: format!("file://{}", p.display()),
            path: p.to_path_buf(),
            folder_path: real_dir.clone(),
            mime_type: "image/jpeg".to_string(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: Some(1),
            height: Some(1),
            video_duration_secs: None,
            taken_at: None,
            file_mtime: chrono::Utc::now(),
            file_size: 1,
            blake3_hash: h.to_string(),
        };
        let id = db::insert_media_item(&pool, &item).unwrap();
        db::mark_trashed(&pool, id).unwrap();
        id
    };
    let id_a = mk(&path_a, "a");
    let _id_b = mk(&path_b, "b");

    let page = TrashPage::new(pool.clone(), loader_for(pool.clone()));
    let grid = page.imp().grid.borrow().as_ref().cloned().unwrap();
    pump_until(&ctx, 100, || grid.logical_media_count() == 2);
    assert_eq!(
        grid.logical_media_count(),
        2,
        "Both trashed items should be loaded into VirtualMediaGrid"
    );

    // 只删 A
    *page.imp().trashed_ids.borrow_mut() = vec![id_a];
    page.imp().delete_btn.get().emit_clicked();

    pump_until(&ctx, 200, || grid.logical_media_count() == 1);
    assert_eq!(
        grid.logical_media_count(),
        1,
        "Virtual grid should retain only the remaining trashed item after partial delete"
    );

    let _ = std::fs::remove_file(&path_a);
    let _ = std::fs::remove_file(&path_b);
}
