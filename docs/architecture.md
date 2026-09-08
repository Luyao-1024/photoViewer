# Architecture

Photo Viewer is a GNOME desktop photo manager built with Rust, GTK4, and Libadwaita. The codebase is intentionally split into data, UI, and platform integration layers.

## Layers

| Layer | Path | Responsibility |
|---|---|---|
| Core | `src/core/` | Database, filesystem scanning, media model, metadata, thumbnails, albums, trash, preferences, edit pipeline |
| UI | `src/ui/` | GTK widgets, pages, templates, CSS providers, navigation wiring |
| Platform | `src/platform/` | XDG and desktop integration |
| Templates | `data/ui/` | Blueprint source templates for GTK composite widgets |

`src/core/` should stay free of UI widget ownership. The edit module is the main exception-like boundary because it returns `image::DynamicImage` values and uses `glib::ParamValue` for operation parameters.

## Runtime Integration

`src/app.rs::build_app` creates a multi-thread Tokio runtime and enters it for the process lifetime. GTK still owns the main loop, but thumbnail workers and scanner work use Tokio blocking tasks. Do not remove this runtime integration without replacing every async/blocking call path that depends on it.

GTK-facing async setup is dispatched through `gtk::glib::MainContext::default().spawn_local`, then injects shared state such as `DbPool` and `Arc<ThumbnailLoader>` into the main window and pages.

## Navigation

`MainWindow` owns the sidebar and an `adw::NavigationView`.

- Photos/Trash and the small Media Types section use `Gtk.ListBox`. The
  potentially large Albums section is a `gio::ListStore` presented by
  `Gtk.ListView` inside a fixed-height `Gtk.ScrolledWindow`, so only visible
  album rows are realized.
- Photos row → pop to the root `PhotosPage`.
- An album row → push that album's `AlbumDetailPage` **directly** (there is no
  intermediate album-grid page). The Albums header is non-selectable and only
  collapses/expands its children.
- Album rows are **drag-to-reorder** (long-press + drag): each carries a
  `DragSource` whose payload is its `folder_path`, and a `DropTarget` that
  persists the new order via `albums::set_album_order` then rebuilds the rows.
  Right-click opens the album context menu; real folder albums can be deleted
  through a trash-backed operation, and multi-select delete only includes real
  folder albums. The order lives in a standalone `album_order` table (decoupled
  from the `albums` materialized view, which is wiped and rebuilt on every
  scan), and `albums::list_with_favorites` re-applies it — virtual and folder
  albums alike.
- Trash row → push `TrashPage`.
- `PhotosPage` owns Year/Month/Day `VirtualMediaGrid` instances backed by the same bounded `gio::ListStore`; the Photos renderer is always virtual `GtkGridView`.
- Settings is launched from a fixed **gear button** in the sidebar footer, and now
  opens a popup `AdwDialog` instead of pushing a new navigation page.
- Opening a photo pushes `ViewerPage`.
- Viewer actions reveal the overlay `EditorPanel` or the details/filmstrip side
  panels without replacing the current media page.

Pages receive navigation and data dependencies via explicit setter methods rather than global state.

## GTK Widget Pattern

Custom widgets follow the gtk4-rs composite template pattern:

- `imp` module with `#[derive(CompositeTemplate)]`.
- `#[template(resource = "/io/github/luyao_1024/photoviewer/ui/<name>.ui")]`.
- `#[template_child]` fields stored in `RefCell`.
- Public wrapper declared with `glib::wrapper!`.

Edit the `.blp` source files. `build.rs` compiles them under Cargo's `OUT_DIR`
and bundles them into a GResource; generated `.ui` files never modify the
source tree.
