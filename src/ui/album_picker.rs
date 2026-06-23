//! AlbumPickerDialog: 列出 albums,选中后弹 Copy/Move 选择,执行后关闭。
//!
//! 这是第一个有状态的自定义对话框(非 `AlertDialog`)。它在 `AdwNavigationView`
//! 之上叠加,所以 `present(host_nav, pool, media_ids)` 调用方传入宿主 nav,
//! dialog 本身以一个新 `AdwNavigationPage` 的形式 push 进去,关闭时 pop。
//!
//! 两级布局:
//! - Level 1: `AdwNavigationPage` 内嵌一个 `ListBox`,每行是 `AdwActionRow`,
//!   显示相册名 + 照片数。点击一行 → push Level 2。
//! - Level 2: `AdwNavigationPage` 内嵌 `AdwToolbarView` + 两个 `Button`:`Copy` / `Move`。
//!   点击 → 在 `spawn_blocking` 中跑 `core::album_ops::add_to_album` → 关闭 dialog。

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::album_ops::{add_to_album, AlbumOpMode};
use crate::core::albums;
use crate::core::db::DbPool;
use crate::core::i18n::{tr, trf};

/// Present the album picker on top of `host_nav`. `media_ids` are the
/// `media_items.id`s the user wants to add (1+ entries; empty is a no-op
/// for the caller). The dialog blocks the nav view while it's open and
/// pops itself when the user confirms or cancels.
pub fn present(host_nav: &adw::NavigationView, pool: DbPool, media_ids: Vec<i64>) {
    if media_ids.is_empty() {
        return;
    }

    // The two-level NavigationView local to this dialog. It lives on top of
    // `host_nav` via a wrapping AdwNavigationPage so ESC / back-pop returns
    // to the level above.
    let inner = adw::NavigationView::new();
    inner.set_vexpand(true);
    inner.set_hexpand(true);

    // Build the level-1 page now (with an empty list) and push it onto
    // `inner`. We populate the list asynchronously from `albums::list` so
    // the dialog shows up instantly even with many albums.
    let list_box = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .css_classes(["boxed-list"])
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .vexpand(true)
        .build();

    let outer = build_album_list_page(&list_box);
    let list_page = adw::NavigationPage::builder()
        .title(&tr("album_picker.title"))
        .child(&outer)
        .build();
    outer.append(&list_box);
    inner.add(&list_page);

    // Wrap the inner nav in a top-level AdwNavigationPage that we push onto
    // `host_nav`. This way:
    // - the dialog has its own back/forward stack (album → action)
    // - cancelling at level 1 pops the wrapping page → back to the caller
    // - the back button in the header takes care of navigation between levels
    let wrapper = adw::NavigationPage::builder()
        .title(&tr("album_picker.title"))
        .child(&inner)
        .build();
    host_nav.push(&wrapper);

    // Populate the album list asynchronously. We hold a strong ref to
    // `list_box` (cloned) so the future can append to it after the function
    // returns.
    let inner_for_listing = inner.clone();
    let pool_for_listing = pool.clone();
    let media_ids_for_rows = media_ids.clone();
    glib::spawn_future_local(async move {
        let albums = match albums::list(&pool_for_listing) {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!("AlbumPicker: albums::list failed: {e}");
                return;
            }
        };
        if albums.is_empty() {
            // Replace the (still empty) list with an empty-state page.
            let empty = adw::StatusPage::builder()
                .title(&tr("album_picker.no_albums_yet.title"))
                .description(&tr("album_picker.no_albums_yet.description"))
                .icon_name("folder-symbolic")
                .vexpand(true)
                .build();
            list_box.set_visible(false);
            outer.append(&empty);
            return;
        }
        for album in albums {
            let row = adw::ActionRow::builder()
                .title(album.display_name())
                .subtitle(trf(
                    "album.count",
                    &[("count", &album.photo_count.to_string())],
                ))
                .activatable(true)
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("folder-symbolic"));
            let inner_clone = inner_for_listing.clone();
            let pool_clone = pool_for_listing.clone();
            let ids_clone = media_ids_for_rows.clone();
            let folder = album.folder_path.clone();
            row.connect_activated(move |_| {
                push_action_page(
                    &inner_clone,
                    pool_clone.clone(),
                    ids_clone.clone(),
                    folder.clone(),
                );
            });
            list_box.append(&row);
        }
    });
}

/// Build the level-1 page chrome: header + outer box. The list_box is
/// appended by the caller.
fn build_album_list_page(list_box: &gtk::ListBox) -> gtk::Box {
    let outer = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    let header = adw::HeaderBar::builder()
        .show_end_title_buttons(false)
        .build();
    outer.append(&header);
    let _ = list_box; // silence unused if not appended
    outer
}

/// Push the level-2 page (Copy / Move) onto `inner`.
fn push_action_page(
    inner: &adw::NavigationView,
    pool: DbPool,
    media_ids: Vec<i64>,
    folder: std::path::PathBuf,
) {
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::builder()
        .show_end_title_buttons(false)
        .build();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .spacing(12)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();

    let name_label = folder
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| folder.display().to_string());
    let title = gtk::Label::builder()
        .label(trf("album_picker.to", &[("name", &name_label)]))
        .css_classes(["title-2"])
        .build();
    let hint = gtk::Label::builder()
        .label(&tr("album_picker.hint"))
        .wrap(true)
        .halign(gtk::Align::Center)
        .css_classes(["dimmed"])
        .build();

    let btn_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .halign(gtk::Align::Center)
        .spacing(12)
        .margin_top(12)
        .build();

    let copy_btn = gtk::Button::with_label(&tr("album_picker.copy"));
    copy_btn.add_css_class("pill");
    copy_btn.add_css_class("suggested-action");

    let move_btn = gtk::Button::with_label(&tr("album_picker.move"));
    move_btn.add_css_class("pill");
    move_btn.add_css_class("destructive-action");

    btn_row.append(&copy_btn);
    btn_row.append(&move_btn);

    content.append(&title);
    content.append(&hint);
    content.append(&btn_row);

    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));

    let page = adw::NavigationPage::builder()
        .title(&tr("album_picker.choose_action.title"))
        .child(&toolbar)
        .build();
    inner.push(&page);

    // Wire Copy / Move buttons to run album_ops on a blocking thread.
    let pool_copy = pool.clone();
    let media_ids_copy = media_ids.clone();
    let folder_copy = folder.clone();
    let inner_copy = inner.clone();
    copy_btn.connect_clicked(move |_| {
        run_op(
            pool_copy.clone(),
            media_ids_copy.clone(),
            folder_copy.clone(),
            AlbumOpMode::Copy,
            inner_copy.clone(),
        );
    });

    let pool_move = pool;
    let media_ids_move = media_ids;
    let folder_move = folder;
    let inner_move = inner.clone();
    move_btn.connect_clicked(move |_| {
        run_op(
            pool_move.clone(),
            media_ids_move.clone(),
            folder_move.clone(),
            AlbumOpMode::Move,
            inner_move.clone(),
        );
    });
}

/// Run `add_to_album` on a blocking thread, then pop the dialog. On error
/// log a warning and pop anyway (the user can re-pick).
fn run_op(
    pool: DbPool,
    media_ids: Vec<i64>,
    folder: std::path::PathBuf,
    mode: AlbumOpMode,
    inner: adw::NavigationView,
) {
    let folder_name = folder
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| folder.display().to_string());
    glib::spawn_future_local(async move {
        // Spawn blocking because add_to_album does synchronous fs I/O.
        let result =
            tokio::task::spawn_blocking(move || add_to_album(&pool, &media_ids, &folder, mode))
                .await;

        match result {
            Ok(Ok(items)) => {
                let verb = match mode {
                    AlbumOpMode::Copy => "Copied",
                    AlbumOpMode::Move => "Moved",
                };
                tracing::info!("{} {} photo(s) to {}", verb, items.len(), folder_name);
                // Pop the entire dialog (inner has 2 levels). Once the
                // user is back at level 1 with no further navigation, the
                // wrapper will be popped by the host's pop handler.
                pop_to_root(&inner);
            }
            Ok(Err(e)) => {
                tracing::warn!("AlbumPicker: add_to_album failed: {e}");
                pop_to_root(&inner);
            }
            Err(e) => {
                tracing::warn!("AlbumPicker: spawn_blocking join failed: {e}");
                pop_to_root(&inner);
            }
        }
    });
}

/// Pop the inner NavigationView back to its root (level 1). The wrapper
/// page stays on `host_nav`; the host's signal handler (or the user
/// pressing back on level 1) will then pop the wrapper.
fn pop_to_root(inner: &adw::NavigationView) {
    // `pop` returns whether a pop happened. We loop until it returns false
    // (i.e. we're back at the root page).
    while inner.pop() {}
}

/// 公共类型别名,方便 photos_page / viewer_page 引用同一签名。
pub type AlbumPickerHandle = ();

/// 「在给定 nav 上展示 picker」的便捷方法。`media_ids` 至少 1 项。
pub struct AlbumPickerDialog;
impl AlbumPickerDialog {
    pub fn present(host_nav: &adw::NavigationView, pool: DbPool, media_ids: Vec<i64>) {
        present(host_nav, pool, media_ids);
    }
}
