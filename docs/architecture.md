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

- Sidebar row 0: Photos root page.
- Sidebar row 1: Albums page.
- Sidebar row 2: Trash page.
- `PhotosPage` owns Year/Month/Day `MediaGrid` instances backed by the same `gio::ListStore`.
- Opening a photo pushes `ViewerPage`.
- Viewer actions can open `EditorPage` or reveal side panels.

Pages receive navigation and data dependencies via explicit setter methods rather than global state.

## GTK Widget Pattern

Custom widgets follow the gtk4-rs composite template pattern:

- `imp` module with `#[derive(CompositeTemplate)]`.
- `#[template(file = "../../data/ui/<name>.ui")]`.
- `#[template_child]` fields stored in `RefCell`.
- Public wrapper declared with `glib::wrapper!`.

Edit the `.blp` source files. `build.rs` compiles them into `.ui` files and bundles UI/icon assets into a GResource during `cargo build`.
