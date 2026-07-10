use super::*;
use chrono::Utc;
use std::path::PathBuf;

fn empty_loader() -> Arc<ThumbnailLoader> {
    let dir = tempfile::tempdir().unwrap().keep();
    let pool = db::init_pool(&dir.join("test.db")).unwrap();
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
fn trash_flow_box_matches_photo_grid_day_view_style() {
    let _ = gtk::init();
    let page = TrashPage::default();
    let flow = page.imp().flow_box.get();

    // grid_viewport now wraps a crossfade Stack (holding the grid content +
    // empty-state page); the flow_box still lives inside the grid_content Box
    // that is the Stack's "content" child.
    assert!(page
        .imp()
        .grid_viewport
        .get()
        .child()
        .and_then(|child| child.downcast::<gtk::Stack>().ok())
        .is_some());
    assert!(flow
        .parent()
        .and_then(|parent| parent.downcast::<gtk::Box>().ok())
        .is_some());
    assert!(flow.has_css_class("thumb-grid"));
    assert!(!flow.has_css_class("trash-grid"));
    assert!(flow.is_homogeneous());
    assert_eq!(flow.column_spacing(), 2);
    assert_eq!(flow.row_spacing(), 2);
    assert_eq!(flow.max_children_per_line(), 100);
    assert_eq!(flow.selection_mode(), gtk::SelectionMode::Multiple);
}

#[gtk::test]
fn trash_tile_uses_day_view_square_thumbnail_spec() {
    let _ = gtk::init();
    let tile = build_trash_tile(media_item(7), empty_loader());

    assert!(tile.is::<crate::ui::square_tile::SquareTile>());
    assert_eq!(tile.target(), TRASH_TILE_PX);
    assert_eq!(TRASH_TILE_PX, 270);
    assert_eq!(TRASH_THUMB_SIZE, ThumbnailSize::Large);
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

/// Insert a single trashed item and pump the main loop until the
/// `TrashPage`'s FlowBox has loaded exactly one tile for it.
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

    let page = TrashPage::new(pool.clone(), empty_loader());
    let flow = page.imp().flow_box.get();
    pump_until(&ctx, 100, || flow.observe_children().n_items() == 1);
    assert_eq!(
        flow.observe_children().n_items(),
        1,
        "TrashPage::new should load the one trashed item into FlowBox"
    );
    (page, id, real_path)
}

/// 点 Restore 后，FlowBox 必须立即清掉已还原的项 —— 之前因为没调
/// `page.refresh()`，tile 还残留在界面上让用户以为还原失败。
#[gtk::test]
fn restore_btn_refreshes_flow_box_after_restoring_items() {
    let (page, id, real_path) = page_with_one_trashed_item();
    let ctx = glib::MainContext::default();
    let flow = page.imp().flow_box.get();

    *page.imp().trashed_ids.borrow_mut() = vec![id];
    page.imp().restore_btn.get().emit_clicked();

    pump_until(&ctx, 200, || flow.observe_children().n_items() == 0);
    assert_eq!(
        flow.observe_children().n_items(),
        0,
        "Flow box should be empty after restoring the only item (refresh wasn't called?)"
    );

    let _ = std::fs::remove_file(&real_path);
}

#[gtk::test]
fn header_bar_uses_glass_header() {
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
    let page = TrashPage::new(pool, empty_loader());
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
    let page = TrashPage::with_media_list(pool.clone(), empty_loader(), shared.clone());
    let flow = page.imp().flow_box.get();
    pump_until(&ctx, 100, || flow.observe_children().n_items() == 1);
    assert_eq!(flow.observe_children().n_items(), 1);

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

/// 部分永久删除后，FlowBox 必须移除被删的项；只保留剩余的 trashed 项。
/// 之前因为只在全空时才 `show_empty_trash`，部分删除后残留旧 tile。
#[gtk::test]
fn delete_btn_refreshes_flow_box_after_partial_delete() {
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

    let page = TrashPage::new(pool.clone(), empty_loader());
    let flow = page.imp().flow_box.get();
    pump_until(&ctx, 100, || flow.observe_children().n_items() == 2);
    assert_eq!(
        flow.observe_children().n_items(),
        2,
        "Both trashed items should be loaded into FlowBox"
    );

    // 只删 A
    *page.imp().trashed_ids.borrow_mut() = vec![id_a];
    page.imp().delete_btn.get().emit_clicked();

    pump_until(&ctx, 200, || flow.observe_children().n_items() == 1);
    assert_eq!(
        flow.observe_children().n_items(),
        1,
        "Flow box should retain only the remaining trashed item after partial delete"
    );

    let _ = std::fs::remove_file(&path_a);
    let _ = std::fs::remove_file(&path_b);
}
