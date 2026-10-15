//! Settings journeys drive real controls in an isolated full application shell.
mod common;
use common::{
    interaction::{descendants, Ui},
    shell::Shell,
};
use gtk::subclass::prelude::ObjectSubclassIsExt;
use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::time::Duration;
fn named<T: IsA<gtk::Widget> + glib::object::IsClass + Clone + glib::object::ObjectType>(
    root: &impl IsA<gtk::Widget>,
    name: &str,
) -> T {
    descendants::<T>(root)
        .into_iter()
        .find(|w| w.as_ref().widget_name() == name)
        .unwrap_or_else(|| panic!("missing widget {name}"))
}
use gtk::glib;
#[test]
fn native_settings_probe_drafts_and_scope_use_real_controls() {
    let _sandbox = photo_viewer::testing::Sandbox::install();
    gtk::init().unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    common::credentials::install();
    if let Ok(mode) = std::env::var("PHOTO_VIEWER_SYNC_MATERIAL") {
        photo_viewer::core::prefs::set_liquid_glass(mode != "plain").unwrap();
    }
    if let Ok(theme) = std::env::var("PHOTO_VIEWER_SYNC_THEME") {
        photo_viewer::core::prefs::set_theme_preference(if theme == "light" {
            photo_viewer::core::prefs::ThemePreference::Light
        } else {
            photo_viewer::core::prefs::ThemePreference::Dark
        })
        .unwrap();
    }
    let shell = Shell::new();
    let ui = &shell.ui;
    ui.click(&shell.window.imp().settings_button.get(), "Settings");
    // The main Settings surface has just the switch and a navigation affordance.
    assert!(!descendants::<gtk::Button>(&shell.window)
        .iter()
        .any(|b| b.widget_name() == "native-sync-add"));
    let enabled = named::<gtk::Switch>(&shell.window, "native-sync-enabled");
    Ui::for_widget(&enabled).scroll_to_reveal(&enabled, "Enable cloud sync");
    Ui::for_widget(&enabled).click_native(&enabled, "Enable cloud sync without navigating");
    assert!(ui.wait_until(Duration::from_secs(3), || {
        photo_viewer::core::prefs::sync_enabled()
    }));
    assert!(!descendants::<adw::NavigationPage>(&shell.window)
        .iter()
        .any(|p| p.widget_name() == "native-sync-page"));
    Ui::for_widget(&enabled).click_native(&enabled, "Disable cloud sync");
    click_response(&shell, "button.cancel");
    assert!(photo_viewer::core::prefs::sync_enabled());
    assert!(enabled.is_active());
    Ui::for_widget(&enabled).click_native(&enabled, "Confirm disabling cloud sync");
    click_response(&shell, "native.disable");
    assert!(ui.wait_until(Duration::from_secs(3), || {
        !photo_viewer::core::prefs::sync_enabled()
    }));
    capture(&shell, "main-settings");
    let entry = named::<adw::ActionRow>(&shell.window, "native-sync-entry");
    Ui::for_widget(&entry).scroll_to_reveal(&entry, "Cloud sync settings");
    Ui::for_widget(&entry).click_native(&entry, "Open cloud sync subpage");
    let page = named::<adw::NavigationPage>(&shell.window, "native-sync-page");
    assert!(ui.wait_until(Duration::from_secs(3), || page.is_mapped()));
    let empty = named::<adw::ActionRow>(&page, "native-sync-empty");
    assert!(empty.is_visible());
    let sync = named::<gtk::Button>(&page, "native-sync-now");
    assert!(!sync.is_sensitive());
    capture(&shell, "sync-empty");
    let add = named::<gtk::Button>(&shell.window, "native-sync-add");
    Ui::for_widget(&add).scroll_to_reveal(&add, "Add server");
    Ui::for_widget(&add).click(&add, "Add server");
    let dialog = named::<adw::Dialog>(&shell.window, "native-sync-connection-dialog");
    assert!(ui.wait_until(Duration::from_secs(3), || dialog.is_visible()));
    let entry = named::<adw::EntryRow>(&dialog, "native-sync-endpoint");
    let key = named::<adw::PasswordEntryRow>(&dialog, "native-sync-password");
    let save = named::<gtk::Button>(&dialog, "native-sync-save");
    assert!(!save.is_sensitive());
    let dialog_ui = Ui::for_widget(&entry);
    dialog_ui.type_editable(&entry, "http://sync.example.invalid");
    dialog_ui.type_editable(&key, &"x".repeat(32));
    let probe = named::<gtk::Button>(&dialog, "native-sync-probe");
    dialog_ui.scroll_to_reveal(&probe, "Verify connection");
    dialog_ui.click(&probe, "Verify connection");
    let status = named::<gtk::Label>(&dialog, "native-sync-probe-status");
    assert!(
        ui.wait_until(Duration::from_secs(5), || status.label().contains("HTTPS")),
        "{}",
        status.label()
    );
    assert!(!save.is_sensitive());
    // Choosing actual local albums remains a draft until Save is accepted.
    let root = named::<adw::EntryRow>(&dialog, "native-sync-root");
    dialog_ui.type_editable(&root, &shell.photos_dir().to_string_lossy());
    let albums = named::<gtk::Button>(&dialog, "native-sync-albums");
    dialog_ui.scroll_to_reveal(&albums, "Select upload albums");
    dialog_ui.click(&albums, "Select upload albums");
    assert!(
        ui.wait_until(Duration::from_secs(5), || descendants::<gtk::CheckButton>(
            &shell.window
        )
        .iter()
        .any(|c| c.widget_name().starts_with("native-sync-album:")))
    );
    let check = descendants::<gtk::CheckButton>(&shell.window)
        .into_iter()
        .find(|c| c.widget_name().starts_with("native-sync-album:"))
        .unwrap();
    Ui::for_widget(&check).click_native(&check, "Select the local fixture album");
    let cancel = named::<gtk::Button>(&shell.window, "native-sync-albums-cancel");
    Ui::for_widget(&cancel).scroll_to_reveal(&cancel, "Cancel album selection");
    Ui::for_widget(&cancel).click(&cancel, "Cancel album selection");
    ui.pump(Duration::from_millis(300));
    let cancel = named::<gtk::Button>(&dialog, "native-sync-cancel");
    dialog_ui.scroll_to_reveal(&cancel, "Cancel connection");
    dialog_ui.click(&cancel, "Cancel connection");
    assert!(ui.wait_until(Duration::from_secs(3), || !dialog.is_mapped()));
    let store = photo_viewer::core::sync::SyncStore::new(shell.pool.clone());
    assert!(store.list_jobs().unwrap().is_empty());
    let connections: i64 = shell
        .pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM sync_connections", [], |r| r.get(0))
        .unwrap();
    assert_eq!(connections, 0);
    assert!(!photo_viewer::core::prefs::sync_enabled());
    let server = LocalServer::start(&runtime);
    let add = named::<gtk::Button>(&shell.window, "native-sync-add");
    Ui::for_widget(&add).scroll_to_reveal(&add, "Add server");
    Ui::for_widget(&add).click(&add, "Add server");
    let dialog = named::<adw::Dialog>(&shell.window, "native-sync-connection-dialog");
    let entry = named::<adw::EntryRow>(&dialog, "native-sync-endpoint");
    let key = named::<adw::PasswordEntryRow>(&dialog, "native-sync-password");
    let root = named::<adw::EntryRow>(&dialog, "native-sync-root");
    let username = named::<adw::EntryRow>(&dialog, "native-sync-username");
    let form = Ui::for_widget(&entry);
    form.type_editable(&username, "alice");
    form.type_editable(&entry, &server.endpoint);
    form.type_editable(&key, &"x".repeat(32));
    form.type_editable(&root, &shell.photos_dir().to_string_lossy());
    let probe = named::<gtk::Button>(&dialog, "native-sync-probe");
    form.scroll_to_reveal(&probe, "Verify the actual server");
    form.click(&probe, "Verify the actual server");
    let save = named::<gtk::Button>(&dialog, "native-sync-save");
    assert!(ui.wait_until(Duration::from_secs(10), || save.is_sensitive()));
    let zone = named::<adw::ComboRow>(&dialog, "native-sync-zone");
    assert_eq!(zone.selected(), 0);
    form.scroll_to_reveal(&save, "Save the verified connection");
    form.click(&save, "Save the verified connection");
    assert!(ui.wait_until(Duration::from_secs(10), || !store
        .list_jobs()
        .unwrap()
        .is_empty()));
    let job = store.list_jobs().unwrap().remove(0);
    assert_eq!(
        job.upload_scope,
        photo_viewer::core::sync::UploadScope::SelectedAlbums
    );
    assert!(store.upload_albums(job.id).unwrap().is_empty());
    assert!(ui.wait_until(Duration::from_secs(5), || !dialog.is_mapped()));
    assert!(
        ui.wait_until(Duration::from_secs(5), || descendants::<gtk::Button>(
            &shell.window
        )
        .iter()
        .any(|b| b.widget_name() == format!("native-sync-scope-{}", job.id)))
    );
    let task = named::<adw::ExpanderRow>(&shell.window, &format!("native-sync-job-{}", job.id));
    assert!(!task.is_expanded(), "task details start collapsed");
    capture(&shell, "sync-task");
    assert!(!named::<adw::ActionRow>(&page, "native-sync-empty").is_visible());
    Ui::for_widget(&task).scroll_to_reveal(&task, "Task details");
    Ui::for_widget(&task).click_native(&task, "Expand task details");
    assert!(ui.wait_until(Duration::from_secs(3), || task.is_expanded()));
    let scope = named::<gtk::Button>(&shell.window, &format!("native-sync-scope-{}", job.id));
    Ui::for_widget(&scope).scroll_to_reveal(&scope, "Edit upload albums");
    Ui::for_widget(&scope).click(&scope, "Edit upload albums");
    assert!(
        ui.wait_until(Duration::from_secs(5), || descendants::<gtk::CheckButton>(
            &shell.window
        )
        .iter()
        .any(|c| c.widget_name() == "native-sync-album:"))
    );
    let album = named::<gtk::CheckButton>(&shell.window, "native-sync-album:");
    Ui::for_widget(&album).click_native(&album, "Select root album");
    let save = named::<gtk::Button>(&shell.window, "native-sync-albums-save");
    Ui::for_widget(&save).scroll_to_reveal(&save, "Save album scope");
    Ui::for_widget(&save).click(&save, "Save album scope");
    assert!(ui.wait_until(Duration::from_secs(5), || store
        .upload_albums(job.id)
        .unwrap()
        == vec![String::new()]));
    assert!(!photo_viewer::core::prefs::sync_enabled());
    assert!(ui.wait_until(Duration::from_secs(5), || store
        .get_job(job.id)
        .unwrap()
        .unwrap()
        .config_generation
        > job.config_generation));
    let generation = store.get_job(job.id).unwrap().unwrap().config_generation;
    ui.pump(Duration::from_millis(600));
    assert_eq!(
        task,
        named::<adw::ExpanderRow>(&shell.window, &format!("native-sync-job-{}", job.id)),
        "refresh must keep task widgets, keyboard focus and expansion"
    );
    assert!(task.is_expanded());
    let scope = named::<gtk::Button>(&shell.window, &format!("native-sync-scope-{}", job.id));
    Ui::for_widget(&scope).scroll_to_reveal(&scope, "Reopen scope");
    Ui::for_widget(&scope).click(&scope, "Reopen scope");
    assert!(
        ui.wait_until(Duration::from_secs(5), || descendants::<gtk::CheckButton>(
            &shell.window
        )
        .iter()
        .any(|c| c.widget_name() == "native-sync-album:"))
    );
    let save = named::<gtk::Button>(&shell.window, "native-sync-albums-save");
    Ui::for_widget(&save).scroll_to_reveal(&save, "Save unchanged scope");
    Ui::for_widget(&save).click(&save, "Save unchanged scope");
    ui.pump(Duration::from_millis(500));
    assert_eq!(
        store.get_job(job.id).unwrap().unwrap().config_generation,
        generation
    );
    capture(&shell, "sync-details");
    for (paused, action) in [(true, "Pause task"), (false, "Resume task")] {
        open_task_menu(&shell, job.id);
        let pause = named::<gtk::Button>(&shell.window, &format!("native-sync-pause-{}", job.id));
        task_ui(&shell, job.id).click(&pause, action);
        assert!(ui.wait_until(Duration::from_secs(3), || store
            .get_job(job.id)
            .unwrap()
            .unwrap()
            .paused
            == paused));
    }
    open_task_menu(&shell, job.id);
    ui.pump(Duration::from_millis(1100));
    let menu = named::<gtk::MenuButton>(&shell.window, &format!("native-sync-menu-{}", job.id));
    assert!(
        menu.popover().unwrap().is_mapped(),
        "refresh keeps open menus"
    );
    capture(&shell, "sync-menu");
    let remove = named::<gtk::Button>(&shell.window, &format!("native-sync-remove-{}", job.id));
    task_ui(&shell, job.id).click(&remove, "Remove task");
    click_response(&shell, "button.cancel");
    assert!(store.get_job(job.id).unwrap().is_some());
    let saved = photo_viewer::platform::credentials::load_api_key(&job.credential_ref).unwrap();
    assert!(saved.starts_with("ss1_"));
    assert_ne!(saved, "x".repeat(32));
    ui.pump(Duration::from_millis(600));
    open_task_menu(&shell, job.id);
    let logout = named::<gtk::Button>(&shell.window, &format!("native-sync-logout-{}", job.id));
    task_ui(&shell, job.id).click(&logout, "Sign out");
    assert!(ui.wait_until(Duration::from_secs(5), || store
        .get_job(job.id)
        .unwrap()
        .unwrap()
        .paused));
    assert!(photo_viewer::platform::credentials::load_api_key(&job.credential_ref).is_err());
    ui.pump(Duration::from_millis(600));
    open_task_menu(&shell, job.id);
    let login = named::<gtk::Button>(&shell.window, &format!("native-sync-relogin-{}", job.id));
    task_ui(&shell, job.id).click(&login, "Sign in again");
    let password = named::<adw::PasswordEntryRow>(&shell.window, "native-sync-relogin-password");
    Ui::for_widget(&password).type_editable(&password, &"x".repeat(32));
    let submit = named::<gtk::Button>(&shell.window, "native-sync-relogin-submit");
    Ui::for_widget(&submit).scroll_to_reveal(&submit, "Sign in and resume");
    Ui::for_widget(&submit).click(&submit, "Sign in and resume");
    assert!(ui.wait_until(Duration::from_secs(10), || !store
        .get_job(job.id)
        .unwrap()
        .unwrap()
        .paused));
    assert!(
        photo_viewer::platform::credentials::load_api_key(&job.credential_ref)
            .unwrap()
            .starts_with("ss1_")
    );
    let back = named::<gtk::Button>(&shell.window, "native-sync-back");
    Ui::for_widget(&back).click(&back, "Back to Settings");
    assert!(ui.wait_until(Duration::from_secs(3), || !page.is_mapped()));
    assert!(named::<gtk::Switch>(&shell.window, "native-sync-enabled").is_mapped());
    let entry = named::<adw::ActionRow>(&shell.window, "native-sync-entry");
    Ui::for_widget(&entry).click_native(&entry, "Reopen cloud sync");
    assert!(
        ui.wait_until(Duration::from_secs(3), || named::<adw::NavigationPage>(
            &shell.window,
            "native-sync-page"
        )
        .is_mapped())
    );
    assert_eq!(
        store.get_job(job.id).unwrap().unwrap().config_generation,
        generation
    );
}

fn click_response(shell: &Shell, key: &str) {
    let text = photo_viewer::core::i18n::tr(key);
    assert!(
        shell
            .ui
            .wait_until(Duration::from_secs(5), || descendants::<gtk::Button>(
                &shell.window
            )
            .iter()
            .any(|b| b.is_mapped() && b.label().as_deref() == Some(text.as_str()))),
        "missing dialog response {text}; buttons: {:?}; dialogs: {:?}",
        descendants::<gtk::Button>(&shell.window)
            .iter()
            .map(|b| (b.label(), b.is_mapped()))
            .collect::<Vec<_>>(),
        descendants::<adw::AlertDialog>(&shell.window)
            .iter()
            .map(|d| d.heading())
            .collect::<Vec<_>>()
    );
    let button = descendants::<gtk::Button>(&shell.window)
        .into_iter()
        .find(|b| b.is_mapped() && b.label().as_deref() == Some(text.as_str()))
        .unwrap();
    Ui::for_widget(&button).click(&button, &text);
    shell.ui.pump(Duration::from_millis(250));
}

// Optional review artifacts are snapshots of the real, input-driven UI.
fn capture(shell: &Shell, name: &str) {
    let Some(directory) = std::env::var_os("PHOTO_VIEWER_SYNC_SCREENSHOTS") else {
        return;
    };
    shell.ui.pump(Duration::from_millis(350));
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    // Capture the composited X11 surface: a transparent widget snapshot cannot
    // show backdrop materials or the popover's separate native surface.
    assert!(std::process::Command::new("import")
        .args(["-window", "root"])
        .arg(directory.join(format!("{name}.png")))
        .status()
        .expect("optional screenshots require ImageMagick")
        .success());
}

fn task_ui(shell: &Shell, id: i64) -> Ui {
    let menu = named::<gtk::MenuButton>(&shell.window, &format!("native-sync-menu-{id}"));
    Ui::for_surface(&menu.popover().unwrap(), "Task action popover")
}

fn open_task_menu(shell: &Shell, id: i64) {
    let menu = named::<gtk::MenuButton>(&shell.window, &format!("native-sync-menu-{id}"));
    Ui::for_widget(&menu).scroll_to_reveal(&menu, "Task actions");
    Ui::for_widget(&menu).click(&menu, "Open task actions");
    assert!(shell.ui.wait_until(Duration::from_secs(3), || menu
        .popover()
        .is_some_and(|p| p.is_mapped())));
}

struct LocalServer {
    child: std::process::Child,
    endpoint: String,
    _root: tempfile::TempDir,
}
impl Drop for LocalServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl LocalServer {
    fn start(runtime: &tokio::runtime::Runtime) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        std::fs::create_dir(&data).unwrap();
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
        for (name, value) in [
            ("api.key", "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"),
            (
                "signing.key",
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            ),
        ] {
            let path = root.path().join(name);
            std::fs::write(&path, value).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let endpoint = format!("http://{address}/");
        let child = std::process::Command::new(
            std::env::var_os("SYNC_SERVER_BIN")
                .unwrap_or_else(|| "../syncserver/target/debug/syncserver".into()),
        )
        .env("SYNC_DATA_DIR", data)
        .env("SYNC_API_KEY_FILE", root.path().join("api.key"))
        .env("SYNC_SIGNING_KEY_FILE", root.path().join("signing.key"))
        .env("SYNC_BIND", address.to_string())
        .env("SYNC_PUBLIC_BASE_URL", &endpoint)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
        let server = Self {
            child,
            endpoint,
            _root: root,
        };
        let client = photo_viewer::core::sync::NativeClient::new(
            &photo_viewer::core::sync::NativeEndpoint::parse(&server.endpoint).unwrap(),
            &"x".repeat(32),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if runtime.block_on(client.probe()).is_ok() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "server did not start");
            std::thread::sleep(Duration::from_millis(50));
        }
        use std::io::Write;
        let mut admin = std::process::Command::new(
            std::env::var_os("SYNC_SERVER_BIN")
                .unwrap_or_else(|| "../syncserver/target/debug/syncserver".into()),
        )
        .args(["users", "create", "alice", "--password-stdin"])
        .env("SYNC_DATA_DIR", server._root.path().join("data"))
        .env("SYNC_API_KEY_FILE", server._root.path().join("api.key"))
        .env(
            "SYNC_SIGNING_KEY_FILE",
            server._root.path().join("signing.key"),
        )
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
        writeln!(admin.stdin.take().unwrap(), "{}", "x".repeat(32)).unwrap();
        assert!(admin.wait().unwrap().success());
        server
    }
}
