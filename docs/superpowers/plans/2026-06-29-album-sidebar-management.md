# Album Sidebar Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the capped album sidebar preview with a fixed-height scrollable album list, add album context management, and trash-backed single/batch folder-album deletion.

**Architecture:** Split the sidebar into a main navigation `Gtk.ListBox` and a dedicated album `Gtk.ListBox` inside `Gtk.ScrolledWindow`, so Photos/Trash/Settings stay stable while album rows scroll independently. Put trash-backed album deletion in core album operations and let UI actions call that boundary, refresh the repository-backed media list, and rebuild album rows. Keep virtual albums navigable but non-deletable.

**Tech Stack:** Rust, GTK4/Libadwaita composite templates, Blueprint UI, rusqlite-backed core DB helpers, existing `MediaRepository`/trash pipeline, cargo integration tests.

---

## File Structure

- Modify `data/ui/window.blp`: split sidebar into `sidebar_list`, `album_scroll`, and `album_list`.
- Modify `src/ui/window.rs`: remove sidebar `AllAlbums`/More dispatch, rebuild albums in `album_list`, add context menu and album selection mode helpers.
- Modify `src/core/album_ops.rs`: add `delete_album_to_trash` and `delete_albums_to_trash`.
- Modify `tests/sidebar_navigation.rs`: update sidebar layout expectations and add >15 album coverage without More.
- Add `tests/album_delete.rs`: verify core folder album deletion and virtual rejection.
- Modify `tests/ui_context_menu.rs`: verify album context menu CSS/action availability using exported menu builder.
- Modify `tests/ux_click_flows.rs`: update collapse/expand assertions to use `album_scroll`.
- Modify `docs/modules/albums-trash.md` and `docs/modules/ui-design.md`: update sidebar and deletion contracts.

---

### Task 1: Sidebar Uses a Dedicated Scrollable Album List

**Files:**
- Modify: `data/ui/window.blp`
- Modify: `src/ui/window.rs`
- Modify: `tests/sidebar_navigation.rs`
- Modify: `tests/ux_click_flows.rs`

- [ ] **Step 1: Write the failing sidebar layout test**

In `tests/sidebar_navigation.rs`, replace the old `assert_more_albums_row_opens_album_browser_page()` helper with `assert_album_sidebar_scroll_region_contains_all_albums()`:

```rust
fn assert_album_sidebar_scroll_region_contains_all_albums() {
    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer.TestScrollableAlbums")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");
    let window = MainWindow::new(&app);
    window.populate_sidebar();

    let tmp = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&tmp.path().join("test.db")).unwrap();
    let loader = Arc::new(photo_viewer::core::thumbnails::ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    let nav = window.nav_view();

    let media_list: gtk::gio::ListStore = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    let photos = PhotosPage::new(media_list.clone(), loader.clone());
    photos.set_nav_target(&nav);
    photos.set_db_pool(pool.clone());
    nav.push(&photos);

    window.set_resources(pool.clone(), loader, media_list);

    for i in 0..25 {
        let folder = format!("/tmp/album-{i:02}");
        let uri = format!("file://{folder}/cover.jpg");
        let path = format!("{folder}/cover.jpg");
        db::insert_media_item(&pool, &make_item(&uri, &path, &folder)).unwrap();
    }
    albums::refresh(&pool).unwrap();
    window.populate_album_rows();
    window.connect_sidebar(&nav);

    assert_eq!(
        window.imp().targets.borrow().len(),
        3,
        "main sidebar targets should contain only Photos, AlbumsHeader, and Trash",
    );
    assert_eq!(
        window.imp().album_rows.borrow().len(),
        28,
        "sidebar album list should render all 25 folder albums plus 3 virtual albums",
    );
    assert!(
        visible_flag(window.imp().album_scroll.get().upcast_ref()),
        "expanded album section should show its scroll region",
    );

    let sidebar = window.imp().sidebar_list.get();
    assert_eq!(
        sidebar.observe_children().n_items(),
        3,
        "main sidebar list should contain only Photos, Albums header, and Trash",
    );
    let trash_row = sidebar.row_at_index(2).expect("Trash row remains stable");
    sidebar.select_row(Some(&trash_row));
    assert_eq!(
        nav.visible_page().map(|page| page.title()).as_deref(),
        Some(tr("page.trash.title").as_str()),
        "Trash should remain a stable main sidebar row",
    );
}
```

Update `sidebar_navigation_suite()` to call this new helper instead of the old More helper.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test sidebar_navigation`

Expected: FAIL because `imp().album_scroll` and `imp().album_list` do not exist yet, or because only 15 album rows are rendered and `AllAlbums` still exists.

- [ ] **Step 3: Update the Blueprint template**

Edit `data/ui/window.blp` so the sidebar surface contains one main list, then a bounded album scroll region, then footer:

```blueprint
Gtk.ListBox sidebar_list {
  css-classes: ["glass-sidebar"];
  hexpand: true;
  selection-mode: single;
}

Gtk.ScrolledWindow album_scroll {
  css-classes: ["glass-sidebar-album-scroll"];
  hexpand: true;
  vexpand: true;
  min-content-height: 120;
  max-content-height: 420;
  hscrollbar-policy: never;
  vscrollbar-policy: automatic;

  Gtk.ListBox album_list {
    css-classes: ["glass-sidebar", "glass-sidebar-album-list"];
    hexpand: true;
    selection-mode: single;
  }
}
```

Keep the existing settings footer after `album_scroll`.

- [ ] **Step 4: Rework `src/ui/window.rs` template children and targets**

In `imp::MainWindow`, add:

```rust
#[template_child]
pub album_scroll: TemplateChild<gtk::ScrolledWindow>,
#[template_child]
pub album_list: TemplateChild<gtk::ListBox>,
pub album_targets: RefCell<Vec<Album>>,
```

Remove `more_albums_row` and `has_more_albums`.

Change `SidebarTarget` to remove `AllAlbums`:

```rust
pub enum SidebarTarget {
    Photos,
    AlbumsHeader,
    Trash,
}
```

In `populate_sidebar`, append only Photos, AlbumsHeader, and Trash to `sidebar_list`, and push the same three targets.

- [ ] **Step 5: Rebuild all albums inside `album_list`**

Replace `rebuild_album_rows` row insertion/removal logic with:

```rust
let album_list = self.imp().album_list.get();
while let Some(child) = album_list.first_child() {
    album_list.remove(&child);
}
self.imp().album_rows.borrow_mut().clear();
self.imp().album_targets.borrow_mut().clear();

let albums = list_with_favorites(&pool).unwrap_or_default();
let expanded = self.imp().albums_expanded.get();
self.imp().album_scroll.get().set_visible(expanded);

for album in albums {
    let row = build_album_row(&album);
    row.set_visible(true);
    self.attach_album_dnd(&row, album.folder_path.to_string_lossy().into_owned());
    self.attach_album_context_menu(&row, album.clone());
    album_list.append(&row);
    self.imp().album_rows.borrow_mut().push(row);
    self.imp().album_targets.borrow_mut().push(album);
}
```

Update `toggle_albums_expanded` to set `album_scroll` visible instead of hiding individual album rows:

```rust
self.imp().album_scroll.get().set_visible(expanded);
```

Update `reselect_active_album_row` to look in `album_targets` and select rows from `album_list`.

- [ ] **Step 6: Connect album list navigation**

In `connect_sidebar`, keep `sidebar_list.connect_row_selected` for Photos and Trash only. Add a separate handler:

```rust
let album_list = self.imp().album_list.get();
album_list.connect_row_selected(
    glib::clone!(@weak self as window, @weak nav_view => move |_list, row| {
        let Some(row) = row else { return; };
        if window.imp().selecting_programmatically.get() {
            return;
        }
        let album = {
            let targets = window.imp().album_targets.borrow();
            let Some(album) = targets.get(row.index() as usize).cloned() else {
                return;
            };
            album
        };
        *window.imp().active_album.borrow_mut() = Some(album.folder_path.clone());
        window.open_album(&nav_view, album);
    }),
);
```

- [ ] **Step 7: Update dependent tests**

In `tests/ux_click_flows.rs`, keep `let first_album_row = window.imp().album_rows.borrow()[0].clone();` but expect collapse/expand to toggle `album_scroll` rather than individual row visibility if the row remains visible inside a hidden scroll parent:

```rust
assert!(visible_flag(window.imp().album_scroll.get().upcast_ref()));
release_click_on_widget(header.upcast_ref());
assert!(!visible_flag(window.imp().album_scroll.get().upcast_ref()));
release_click_on_widget(header.upcast_ref());
assert!(visible_flag(window.imp().album_scroll.get().upcast_ref()));
```

- [ ] **Step 8: Run the sidebar tests**

Run: `cargo test --test sidebar_navigation`

Expected: PASS.

Run: `cargo test --test ux_click_flows sidebar_clicks_drive_top_level_navigation`

Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add data/ui/window.blp src/ui/window.rs tests/sidebar_navigation.rs tests/ux_click_flows.rs
git commit -m "feat(ui): make sidebar albums scrollable"
```

---

### Task 2: Core Album Deletion Moves Folder Media to Trash

**Files:**
- Modify: `src/core/album_ops.rs`
- Add: `tests/album_delete.rs`

- [ ] **Step 1: Write failing core deletion tests**

Create `tests/album_delete.rs`:

```rust
use chrono::Utc;
use photo_viewer::core::album_ops::{delete_album_to_trash, delete_albums_to_trash};
use photo_viewer::core::albums::{self, Album, FAVORITES_ALBUM_PATH};
use photo_viewer::core::db;
use photo_viewer::core::media::NewMediaItem;
use std::path::PathBuf;

fn make_item(uri: &str, path: &str, folder: &str, hash: &str) -> NewMediaItem {
    NewMediaItem {
        uri: uri.into(),
        path: path.into(),
        folder_path: folder.into(),
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(100),
        height: Some(100),
        video_duration_secs: None,
        taken_at: Some(Utc::now()),
        file_mtime: Utc::now(),
        file_size: 1024,
        blake3_hash: hash.into(),
    }
}

fn real_album(folder: &str) -> Album {
    Album {
        folder_path: PathBuf::from(folder),
        name: folder.into(),
        cover_uri: None,
        photo_count: 1,
        last_modified: Utc::now(),
        is_virtual: false,
    }
}

#[test]
fn delete_album_to_trash_rejects_virtual_album() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&tmp.path().join("test.db")).unwrap();
    let album = Album {
        folder_path: PathBuf::from(FAVORITES_ALBUM_PATH),
        name: "Favorites".into(),
        cover_uri: None,
        photo_count: 0,
        last_modified: Utc::now(),
        is_virtual: true,
    };

    let err = delete_album_to_trash(&pool, &album).expect_err("virtual album delete should fail");
    assert!(
        err.to_string().contains("virtual album"),
        "error should explain virtual albums are not deletable: {err}",
    );
}

#[test]
fn delete_album_to_trash_marks_folder_media_trashed_and_refreshes_albums() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&tmp.path().join("test.db")).unwrap();
    let folder = tmp.path().join("Camera");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("one.jpg");
    std::fs::write(&file, b"jpg").unwrap();
    let folder_s = folder.to_string_lossy().to_string();
    let uri = format!("file://{}", file.display());

    let id = db::insert_media_item(
        &pool,
        &make_item(&uri, &file.to_string_lossy(), &folder_s, "hash-one"),
    )
    .unwrap();
    albums::refresh(&pool).unwrap();
    assert!(albums::list(&pool).unwrap().iter().any(|a| a.folder_path == folder));

    let mutation = delete_album_to_trash(&pool, &real_album(&folder_s)).unwrap();

    assert_eq!(mutation.changed_ids.len(), 1);
    assert_eq!(mutation.changed_ids[0].get(), id);
    assert!(
        db::get_media_item(&pool, id).unwrap().trashed_at.is_some(),
        "album deletion should mark media as trashed",
    );
    assert!(
        albums::list(&pool)
            .unwrap()
            .iter()
            .all(|a| a.folder_path != folder),
        "folder album should disappear after refresh because its media is trashed",
    );
}

#[test]
fn delete_albums_to_trash_handles_multiple_real_albums() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = db::init_pool(&tmp.path().join("test.db")).unwrap();
    let mut albums_to_delete = Vec::new();
    let mut ids = Vec::new();
    for idx in 0..2 {
        let folder = tmp.path().join(format!("Album{idx}"));
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("one.jpg");
        std::fs::write(&file, b"jpg").unwrap();
        let folder_s = folder.to_string_lossy().to_string();
        let uri = format!("file://{}", file.display());
        let id = db::insert_media_item(
            &pool,
            &make_item(&uri, &file.to_string_lossy(), &folder_s, &format!("hash-{idx}")),
        )
        .unwrap();
        ids.push(id);
        albums_to_delete.push(real_album(&folder_s));
    }
    albums::refresh(&pool).unwrap();

    let mutation = delete_albums_to_trash(&pool, &albums_to_delete).unwrap();

    assert_eq!(mutation.changed_ids.len(), 2);
    for id in ids {
        assert!(db::get_media_item(&pool, id).unwrap().trashed_at.is_some());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test album_delete`

Expected: FAIL because `delete_album_to_trash` and `delete_albums_to_trash` do not exist.

- [ ] **Step 3: Implement core deletion**

Append to `src/core/album_ops.rs`:

```rust
use crate::core::identity::MediaId;
use crate::core::repository::{MediaMutation, MediaRepository};

pub fn delete_album_to_trash(pool: &DbPool, album: &albums::Album) -> Result<MediaMutation> {
    delete_albums_to_trash(pool, std::slice::from_ref(album))
}

pub fn delete_albums_to_trash(pool: &DbPool, albums_to_delete: &[albums::Album]) -> Result<MediaMutation> {
    let repo = MediaRepository::new(pool.clone());
    let mut combined = MediaMutation::default();

    for album in albums_to_delete {
        if album.is_virtual {
            return Err(AppError::Backend(format!(
                "virtual album is not deletable: {}",
                album.display_name()
            )));
        }
        let ids = db::list_media_by_folder(pool, &album.folder_path)?
            .into_iter()
            .map(|item| MediaId::from(item.id))
            .collect::<Vec<_>>();
        if ids.is_empty() {
            continue;
        }
        let mutation = repo.move_to_trash(&ids)?;
        combined.changed_ids.extend(mutation.changed_ids);
        combined.changed_items.extend(mutation.changed_items);
        combined.removed_uris.extend(mutation.removed_uris);
    }

    albums::refresh(pool)?;
    Ok(combined)
}
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --test album_delete`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/core/album_ops.rs tests/album_delete.rs
git commit -m "feat(core): trash folder albums"
```

---

### Task 3: Album Context Menu and Single Delete UI

**Files:**
- Modify: `src/ui/window.rs`
- Modify: `tests/ui_context_menu.rs`

- [ ] **Step 1: Write failing context menu tests**

In `tests/ui_context_menu.rs`, add tests for a public or `pub(crate)` helper `build_album_context_menu_for_tests`:

```rust
use chrono::Utc;
use photo_viewer::core::albums::{Album, FAVORITES_ALBUM_PATH};
use photo_viewer::ui::window::build_album_context_menu_for_tests;
use std::path::PathBuf;

fn album(is_virtual: bool) -> Album {
    Album {
        folder_path: if is_virtual {
            PathBuf::from(FAVORITES_ALBUM_PATH)
        } else {
            PathBuf::from("/tmp/Camera")
        },
        name: "Camera".into(),
        cover_uri: None,
        photo_count: 3,
        last_modified: Utc::now(),
        is_virtual,
    }
}

fn button_labels(root: &gtk::Widget) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(button) = root.downcast_ref::<gtk::Button>() {
        if let Some(label) = button.label() {
            out.push(label.to_string());
        }
    }
    let mut child = root.first_child();
    while let Some(widget) = child {
        out.extend(button_labels(&widget));
        child = widget.next_sibling();
    }
    out
}

#[test]
fn real_album_context_menu_contains_manage_and_delete() {
    gtk::init().expect("GTK init failed");
    let popover = build_album_context_menu_for_tests(&album(false));
    let labels = button_labels(popover.upcast_ref());
    assert!(popover.css_classes().iter().any(|c| c == "glass-menu"));
    assert!(labels.iter().any(|label| label == "管理相册"));
    assert!(labels.iter().any(|label| label == "删除相册"));
}

#[test]
fn virtual_album_context_menu_omits_delete() {
    gtk::init().expect("GTK init failed");
    let popover = build_album_context_menu_for_tests(&album(true));
    let labels = button_labels(popover.upcast_ref());
    assert!(labels.iter().any(|label| label == "管理相册"));
    assert!(!labels.iter().any(|label| label == "删除相册"));
}
```

- [ ] **Step 2: Run context menu tests to verify fail**

Run: `cargo test --test ui_context_menu`

Expected: FAIL because `build_album_context_menu_for_tests` does not exist.

- [ ] **Step 3: Add menu builder and row gesture**

In `src/ui/window.rs`, add a helper:

```rust
pub fn build_album_context_menu_for_tests(album: &Album) -> gtk::Popover {
    build_album_context_menu(album, None, None, None)
}

fn build_album_context_menu(
    album: &Album,
    on_manage: Option<Rc<dyn Fn()>>,
    on_delete: Option<Rc<dyn Fn()>>,
    on_select: Option<Rc<dyn Fn()>>,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.add_css_class("glass-menu");
    let menu = gtk::Box::new(gtk::Orientation::Vertical, 4);
    menu.add_css_class("glass-menu-list");

    let manage = gtk::Button::with_label("管理相册");
    manage.add_css_class("glass-menu-item");
    if let Some(callback) = on_manage {
        manage.connect_clicked(move |_| callback());
    }
    menu.append(&manage);

    let multi = gtk::Button::with_label("多选相册");
    multi.add_css_class("glass-menu-item");
    multi.add_css_class("glass-menu-item-suggested");
    if let Some(callback) = on_select {
        multi.connect_clicked(move |_| callback());
    }
    menu.append(&multi);

    if !album.is_virtual {
        let delete = gtk::Button::with_label("删除相册");
        delete.add_css_class("glass-menu-item");
        delete.add_css_class("glass-menu-item-danger");
        if let Some(callback) = on_delete {
            delete.connect_clicked(move |_| callback());
        }
        menu.append(&delete);
    }

    popover.set_child(Some(&menu));
    popover
}
```

Then implement `attach_album_context_menu(&self, row: &gtk::ListBoxRow, album: Album)`:

```rust
let gesture = gtk::GestureClick::new();
gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
gesture.connect_pressed(glib::clone!(@weak self as window, @weak row => move |_gesture, _n, x, y| {
    let nav = window.imp().nav_view.get();
    let album_for_manage = album.clone();
    let album_for_delete = album.clone();
    let popover = build_album_context_menu(
        &album,
        Some(Rc::new(glib::clone!(@weak window, @weak nav => move || {
            *window.imp().active_album.borrow_mut() = Some(album_for_manage.folder_path.clone());
            window.open_album(&nav, album_for_manage.clone());
        }))),
        Some(Rc::new(glib::clone!(@weak window => move || {
            window.confirm_delete_album(album_for_delete.clone());
        }))),
        Some(Rc::new(glib::clone!(@weak window => move || {
            window.enter_album_selection_mode();
        }))),
    );
    popover.set_parent(&row);
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    popover.popup();
}));
row.add_controller(gesture);
```

- [ ] **Step 4: Add single-delete confirmation and refresh**

In `src/ui/window.rs`, add:

```rust
fn confirm_delete_album(&self, album: Album) {
    if album.is_virtual {
        return;
    }
    let dialog = adw::AlertDialog::builder()
        .heading("删除相册")
        .body(format!("相册中的媒体会移入系统回收站：{}", album.display_name()))
        .build();
    dialog.add_response("cancel", &tr("common.cancel"));
    dialog.add_response("delete", "删除");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.connect_response(
        Some("delete"),
        glib::clone!(@weak self as window => move |_, _| {
            window.delete_albums_to_trash_ui(vec![album.clone()]);
        }),
    );
    dialog.present(self);
}

fn delete_albums_to_trash_ui(&self, albums: Vec<Album>) {
    let Some(pool) = self.imp().pool.borrow().clone() else {
        return;
    };
    match crate::core::album_ops::delete_albums_to_trash(&pool, &albums) {
        Ok(mutation) => {
            if let Some(media_list) = self.imp().media_list.borrow().as_ref() {
                remove_uris_from_media_list(media_list, &mutation.removed_uris);
            }
            self.refresh_album_rows();
            if albums.iter().any(|album| {
                self.imp().active_album.borrow().as_ref() == Some(&album.folder_path)
            }) {
                pop_to_photos_root(&self.imp().nav_view.get());
                *self.imp().active_album.borrow_mut() = None;
            }
        }
        Err(err) => tracing::warn!("failed to delete album to trash: {err}"),
    }
}
```

Add `remove_uris_from_media_list` near helpers:

```rust
fn remove_uris_from_media_list(media_list: &gtk::gio::ListStore, removed_uris: &[String]) {
    let removed: std::collections::HashSet<&str> = removed_uris.iter().map(String::as_str).collect();
    let mut idx = 0;
    while idx < media_list.n_items() {
        let remove = media_list
            .item(idx)
            .and_downcast::<glib::BoxedAnyObject>()
            .map(|boxed| boxed.borrow::<crate::core::media::MediaItem>().uri.clone())
            .is_some_and(|uri| removed.contains(uri.as_str()));
        if remove {
            media_list.remove(idx);
        } else {
            idx += 1;
        }
    }
}
```

- [ ] **Step 5: Run context and sidebar tests**

Run: `cargo test --test ui_context_menu`

Expected: PASS.

Run: `cargo test --test sidebar_navigation`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/ui/window.rs tests/ui_context_menu.rs
git commit -m "feat(ui): add album context menu"
```

---

### Task 4: Album Multi-Select Delete

**Files:**
- Modify: `src/ui/window.rs`
- Modify: `tests/ux_click_flows.rs`

- [ ] **Step 1: Write failing multi-select UI test**

In `tests/ux_click_flows.rs`, add:

```rust
#[test]
fn album_sidebar_multi_select_deletes_real_albums() {
    gtk::init().expect("GTK init failed");
    let app = adw::Application::builder()
        .application_id("org.gnome.PhotoViewer.AlbumMultiSelect")
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("test application should register");

    let fixture = build_photos_page_with_nav();
    let window = MainWindow::new(&app);
    window.populate_sidebar();
    window.set_resources(
        fixture.pool.clone(),
        fixture.loader.clone(),
        fixture.media_list.clone(),
    );
    albums::refresh(&fixture.pool).unwrap();
    window.populate_album_rows();

    window.enter_album_selection_mode();
    assert_eq!(
        window.imp().album_list.get().selection_mode(),
        gtk::SelectionMode::Multiple,
        "album selection mode should switch the album list to multiple selection",
    );

    let real_rows: Vec<gtk::ListBoxRow> = window
        .imp()
        .album_targets
        .borrow()
        .iter()
        .enumerate()
        .filter(|(_, album)| !album.is_virtual)
        .take(2)
        .filter_map(|(idx, _)| window.imp().album_list.get().row_at_index(idx as i32))
        .collect();
    assert_eq!(real_rows.len(), 2, "fixture should include two real album rows");
    for row in &real_rows {
        window.imp().album_list.get().select_row(Some(row));
    }
    assert_eq!(window.selected_album_delete_count(), 2);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test ux_click_flows album_sidebar_multi_select_deletes_real_albums`

Expected: FAIL because selection mode helpers do not exist.

- [ ] **Step 3: Add selection mode state and action bar**

In `imp::MainWindow`, add:

```rust
pub album_selection_mode: Cell<bool>,
pub selected_album_paths: RefCell<std::collections::HashSet<PathBuf>>,
```

In `MainWindow`, add:

```rust
pub fn enter_album_selection_mode(&self) {
    self.imp().album_selection_mode.set(true);
    self.imp().album_list.get().set_selection_mode(gtk::SelectionMode::Multiple);
    self.imp().selected_album_paths.borrow_mut().clear();
}

fn exit_album_selection_mode(&self) {
    self.imp().album_selection_mode.set(false);
    self.imp().album_list.get().unselect_all();
    self.imp().album_list.get().set_selection_mode(gtk::SelectionMode::Single);
    self.imp().selected_album_paths.borrow_mut().clear();
}

pub fn selected_album_delete_count(&self) -> usize {
    self.imp().selected_album_paths.borrow().len()
}
```

In the album list `connect_row_selected` handler, branch when selection mode is active:

```rust
if window.imp().album_selection_mode.get() {
    window.sync_selected_album_paths();
    return;
}
```

Implement:

```rust
fn sync_selected_album_paths(&self) {
    let selected = self
        .imp()
        .album_list
        .get()
        .selected_rows()
        .into_iter()
        .filter_map(|row| {
            self.imp()
                .album_targets
                .borrow()
                .get(row.index() as usize)
                .cloned()
        })
        .filter(|album| !album.is_virtual)
        .map(|album| album.folder_path)
        .collect::<std::collections::HashSet<_>>();
    *self.imp().selected_album_paths.borrow_mut() = selected;
}
```

If a virtual row is selected in multiple mode, unselect it after syncing:

```rust
for row in self.imp().album_list.get().selected_rows() {
    if self
        .imp()
        .album_targets
        .borrow()
        .get(row.index() as usize)
        .is_some_and(|album| album.is_virtual)
    {
        self.imp().album_list.get().unselect_row(&row);
    }
}
```

- [ ] **Step 4: Add batch delete command**

Add:

```rust
fn confirm_delete_selected_albums(&self) {
    let selected = self.selected_real_albums();
    if selected.is_empty() {
        return;
    }
    let dialog = adw::AlertDialog::builder()
        .heading("删除所选相册")
        .body(format!("{} 个相册中的媒体会移入系统回收站。", selected.len()))
        .build();
    dialog.add_response("cancel", &tr("common.cancel"));
    dialog.add_response("delete", "删除");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.connect_response(
        Some("delete"),
        glib::clone!(@weak self as window => move |_, _| {
            window.delete_albums_to_trash_ui(selected.clone());
            window.exit_album_selection_mode();
        }),
    );
    dialog.present(self);
}

fn selected_real_albums(&self) -> Vec<Album> {
    let selected = self.imp().selected_album_paths.borrow().clone();
    self.imp()
        .album_targets
        .borrow()
        .iter()
        .filter(|album| !album.is_virtual && selected.contains(&album.folder_path))
        .cloned()
        .collect()
}
```

Expose the batch delete command through a compact `Gtk.ActionBar` below `album_scroll`. Add these template children to `data/ui/window.blp` and `imp::MainWindow`:

```blueprint
Gtk.ActionBar album_selection_bar {
  revealed: false;

  [start]
  Gtk.Button album_selection_cancel_btn {
    label: "取消";
    css-classes: ["glass-toolbar-button"];
  }

  [end]
  Gtk.Button album_selection_delete_btn {
    label: "删除所选相册";
    css-classes: ["glass-toolbar-button", "glass-toolbar-danger"];
  }
}
```

```rust
#[template_child]
pub album_selection_bar: TemplateChild<gtk::ActionBar>,
#[template_child]
pub album_selection_cancel_btn: TemplateChild<gtk::Button>,
#[template_child]
pub album_selection_delete_btn: TemplateChild<gtk::Button>,
```

In `enter_album_selection_mode`, reveal the action bar. In `exit_album_selection_mode`, hide it. In `connect_sidebar`, connect cancel to `exit_album_selection_mode` and delete to `confirm_delete_selected_albums`.

- [ ] **Step 5: Run multi-select test**

Run: `cargo test --test ux_click_flows album_sidebar_multi_select_deletes_real_albums`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/ui/window.rs tests/ux_click_flows.rs
git commit -m "feat(ui): add album multi-select deletion"
```

---

### Task 5: Documentation and Full Verification

**Files:**
- Modify: `docs/modules/albums-trash.md`
- Modify: `docs/modules/ui-design.md`

- [ ] **Step 1: Update album behavior docs**

In `docs/modules/albums-trash.md`, replace the paragraph that says the sidebar keeps at most 15 album rows and shows `sidebar.albums_more` with:

```markdown
Albums are shown under a collapsible "Albums" group header. The group owns a
fixed-height scroll region in the sidebar: Photos, Trash, and Settings remain
stable while the album rows themselves scroll. All virtual and folder albums
are rendered directly in that scroll region; there is no "More" row in the
sidebar.

Right-clicking an album row opens a glass context menu. "Manage Album" opens
the album detail page. Real folder albums also expose "Delete Album", which
moves every media item in that folder to the system trash and then refreshes
the derived album list. Virtual albums such as Favorites, Photos, and Videos
are navigable but not deletable. Album multi-select is limited to deleting
multiple real folder albums through the same trash-backed operation.
```

- [ ] **Step 2: Update UI design docs**

In `docs/modules/ui-design.md`, replace the Window Shell sidebar bullet that mentions "when album count exceeds 15" with:

```markdown
- The Albums header is a collapsible group control. Album rows appear in a
  fixed-height scroll region directly under it, including virtual albums such
  as Favorites, Photos, and Videos. The scroll region contains all albums, so
  the sidebar no longer uses a More row.
```

Add:

```markdown
- Right-click album actions use the shared glass menu treatment. Destructive
  album deletion is available only for real folder albums and communicates that
  media is moved to system trash.
```

- [ ] **Step 3: Run focused verification**

Run:

```bash
cargo test --test sidebar_navigation
cargo test --test ui_context_menu
cargo test --test album_delete
cargo test --test ux_click_flows
```

Expected: all PASS.

- [ ] **Step 4: Run formatting and broader tests**

Run:

```bash
cargo fmt
cargo test
```

Expected: all PASS. Existing GTK warnings are acceptable only if they match documented known warnings in `docs/testing.md`.

- [ ] **Step 5: Commit docs and any formatting-only changes**

```bash
git add docs/modules/albums-trash.md docs/modules/ui-design.md
git commit -m "docs: update album sidebar management"
```
