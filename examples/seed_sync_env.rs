//! Seed a sandboxed environment for end-to-end sync verification.
//!
//! Stores the WebDAV credential in the platform keyring and creates one sync
//! job pointing at a local WebDAV endpoint, using the environment's XDG
//! directories. Run inside the same D-Bus session as the client under test:
//!
//! ```text
//! XDG_DATA_HOME=… XDG_CONFIG_HOME=… cargo run --example seed_sync_env -- <endpoint> <password> <local_root>
//! ```
//!
//! `--cleanup` removes the seeded credential from the keyring instead.

use photo_viewer::core::sync::{NewSyncJob, SyncDirection, SyncStore, UploadScope};

const REF: &str = "seed-credential";

fn main() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--cleanup") {
        match photo_viewer::platform::credentials::delete(REF) {
            Ok(()) => println!("removed {REF} from keyring"),
            Err(error) => println!("cleanup failed: {error}"),
        }
        return;
    }

    let endpoint = args.next().expect("endpoint argument");
    let password = args.next().expect("password argument");
    let local_root = args.next().expect("local_root argument");

    let data_dir = photo_viewer::config::data_dir();
    std::fs::create_dir_all(&data_dir).expect("create data dir");
    let pool = photo_viewer::core::db::init_pool(&data_dir.join("photos.db")).expect("init pool");

    // The client loads the credential from the same keyring (or, in tests,
    // from the session keyutils cache), but a keyring hiccup must not hide
    // job-seeding failures, so store failures are warnings, not fatal.
    if let Err(error) = photo_viewer::platform::credentials::store(REF, &password) {
        eprintln!("warning: keyring store unavailable: {error}");
    }
    photo_viewer::core::prefs::set_webdav_sync_enabled(true).expect("persist webdav opt-in");

    let store = SyncStore::new(pool);
    let job = store
        .create_job(&NewSyncJob {
            endpoint,
            username: "seeduser".into(),
            credential_ref: REF.into(),
            local_root: local_root.into(),
            remote_root: "PhotoViewer".into(),
            direction: SyncDirection::Bidirectional,
            upload_scope: UploadScope::All,
            upload_albums: Vec::new(),
        })
        .expect("create sync job");
    println!("seeded sync job {} -> {}", job.id, job.endpoint);
}
