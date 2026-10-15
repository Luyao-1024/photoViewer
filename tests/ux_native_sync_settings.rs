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
    keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    let shell = Shell::new();
    let ui = &shell.ui;
    ui.click(&shell.window.imp().settings_button.get(), "Settings");
    let add = named::<gtk::Button>(&shell.window, "native-sync-add");
    Ui::for_widget(&add).scroll_to_reveal(&add, "Add server");
    Ui::for_widget(&add).click(&add, "Add server");
    let dialog = named::<adw::Dialog>(&shell.window, "native-sync-connection-dialog");
    assert!(ui.wait_until(Duration::from_secs(3), || dialog.is_visible()));
    let entry = named::<adw::EntryRow>(&dialog, "native-sync-endpoint");
    let key = named::<adw::PasswordEntryRow>(&dialog, "native-sync-key");
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
    let key = named::<adw::PasswordEntryRow>(&dialog, "native-sync-key");
    let root = named::<adw::EntryRow>(&dialog, "native-sync-root");
    let form = Ui::for_widget(&entry);
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
        server
    }
}
