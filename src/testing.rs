//! A throwaway home for tests that drive the real application.
//!
//! [`Sandbox::install()`] redirects `HOME`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME`,
//! `XDG_CACHE_HOME` and `TMPDIR` at a disposable tree that is deleted when the
//! process ends, so a test run neither reads the developer's preferences nor
//! rewrites them.
//! Without it a single UX journey is enough to append a dead
//! `excluded_scan_roots` entry to a real `settings.json`, or to push a fixture
//! photo into a real trash directory.
//!
//! Two facts about GIO and GLib shape this module, and both cost a debugging
//! session to learn.
//!
//! **GIO picks the trash directory by device, not by `XDG_DATA_HOME`.** In
//! `g_local_file_trash()` the home trash — the one at `$XDG_DATA_HOME/Trash` —
//! is used only when the file being trashed is on the same filesystem as
//! `$HOME`. Otherwise `XDG_DATA_HOME` is ignored entirely and GIO falls back to
//! `$topdir/.Trash-$uid` on the file's own mount: a write that escapes the
//! sandbox, at a path the app cannot find again, with a *relative* `Path=` in
//! the `.trashinfo` that [`crate::core::trash`] will never match against the
//! absolute original. So the sandbox root has to share a filesystem with
//! whatever is being trashed, and the assertion below makes a machine that
//! cannot honour that fail loudly instead of silently littering a mount root.
//!
//! **GLib reads the environment once per process.** `g_get_home_dir()` and
//! `g_get_user_data_dir()` cache their answers the first time they are called,
//! which for these tests means the first GTK or GIO call in the process. A
//! second `Sandbox` installed later would set variables nothing reads again, so
//! [`Sandbox::install()`] is idempotent through a [`OnceLock`] and callers must
//! install before `gtk::init()`. Per-test sandboxes are not merely racy in a
//! multi-`#[test]` binary, they are wrong: the second test's environment never
//! reaches GIO, which is still pointing into the first test's deleted
//! directory.
//!
//! The module is deliberately ungated rather than hidden behind a feature: the
//! library's own trash tests (`core::trash::tests`, `ui::trash_page::tests`)
//! need it too, and those can only reach code that the library exports.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tempfile::TempDir;

static SANDBOX: OnceLock<Sandbox> = OnceLock::new();

/// The disposable environment a test runs in.
///
/// Created by [`Sandbox::install`], which is the only supported way to obtain
/// one: the process-wide caching described at the module level makes a
/// hand-rolled second sandbox a no-op that still looks like isolation.
pub struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    /// Install the sandbox, or return the one already installed.
    ///
    /// Call this as the first statement of a test, before `gtk::init()` and
    /// before anything that touches the filesystem. The returned reference is
    /// `&'static` on purpose — the sandbox lives for the whole process whether
    /// or not the caller holds on to it.
    pub fn install() -> &'static Sandbox {
        SANDBOX.get_or_init(Sandbox::build)
    }

    /// The sandbox root, `target/ux-fixtures/sandbox-<pid>`.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A directory on the sandbox's filesystem, removed when the returned
    /// [`TempDir`] drops.
    ///
    /// Trash journeys need this: `tempfile::tempdir()` hands out a tmpfs
    /// directory, and trashing across that boundary is exactly the `EXDEV` →
    /// `ENOTSUP` failure the module docs describe. Scratch directories keep the
    /// `photo-viewer-` prefixes those journeys already used, so a stray one is
    /// still recognisable.
    pub fn scratch(&self, prefix: &str) -> TempDir {
        let base = self.root.join("scratch");
        std::fs::create_dir_all(&base).expect("the sandbox scratch directory should be creatable");
        tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(&base)
            .expect("a sandbox scratch directory should be creatable")
    }

    fn build() -> Self {
        let root = root_path();
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        for dir in [
            &home,
            &root.join("config"),
            &root.join("data"),
            &root.join("cache"),
            &root.join("tmp"),
        ] {
            std::fs::create_dir_all(dir).expect("a sandbox directory should be creatable");
        }
        seed_user_dirs(&home);
        sweep_abandoned_siblings(&root);

        // The rule GIO actually applies, checked rather than assumed: a trash
        // that ends up on a different filesystem from the files it holds is
        // both a broken restore and a write outside this sandbox.
        assert_same_device(&home, &root, "$HOME");

        let sandbox = Sandbox { root };
        sandbox.apply_env();
        sandbox.schedule_removal();
        sandbox
    }

    fn apply_env(&self) {
        set_env("HOME", self.root.join("home"));
        set_env("XDG_CONFIG_HOME", self.root.join("config"));
        set_env("XDG_DATA_HOME", self.root.join("data"));
        set_env("XDG_CACHE_HOME", self.root.join("cache"));
        // Makes every `tempfile::tempdir()` in the process land on the
        // sandbox's filesystem too, which retires the whole class of
        // "a future trash call wrote to /tmp/.Trash-$uid" hazards.
        set_env("TMPDIR", self.root.join("tmp"));
    }

    /// Arrange for the tree to disappear when the process ends normally.
    ///
    /// A `Drop` impl would be the obvious choice and would silently never run:
    /// this struct lives in a [`OnceLock`] static, and Rust does not drop
    /// statics. `atexit` is the hook that does fire, and the sweep in
    /// [`sweep_abandoned_siblings`] covers what it cannot — a `SIGKILL`, a panic
    /// that aborts, a `cargo test` interrupted with Ctrl-C.
    fn schedule_removal(&self) {
        use std::os::unix::ffi::OsStrExt;
        let root = std::ffi::CString::new(self.root.as_os_str().as_bytes())
            .expect("a sandbox path should contain no interior nul byte");
        let _ = ATEXIT_ROOT.set(root);
        // SAFETY: the handler only reads `ATEXIT_ROOT` and calls into `std::fs`;
        // `atexit` runs it on the exiting main thread, after which no other
        // thread is scheduled, so there is nothing to race with.
        unsafe { libc::atexit(remove_sandbox_at_exit) };
    }
}

static ATEXIT_ROOT: OnceLock<std::ffi::CString> = OnceLock::new();

/// Install the sandbox before `main` for this crate's own unit tests.
///
/// The integration binaries under `tests/` each have a single `#[test]` and can
/// simply call [`Sandbox::install()`] as its first statement. The lib test
/// binary cannot: it runs 749 tests in parallel threads, so "the first
/// statement of a test" is a race, and whichever test first touches GLib fixes
/// the cached `$HOME` for every other one. The symptom is a restore that writes
/// to the real trash through GIO and then cannot find the file, because the
/// app's own `trash_roots()` — which reads the environment live, unlike GLib —
/// is already looking in the sandbox.
///
/// A constructor is the only hook that runs before anything else. `#[cfg(test)]`
/// keeps it out of the application, where redirecting `HOME` would be a
/// catastrophe rather than a courtesy.
#[cfg(all(test, unix))]
mod early_install {
    extern "C" fn install_sandbox_before_main() {
        super::Sandbox::install();
    }

    #[used]
    #[link_section = ".init_array"]
    static INSTALL_SANDBOX: extern "C" fn() = install_sandbox_before_main;
}

extern "C" fn remove_sandbox_at_exit() {
    use std::os::unix::ffi::OsStrExt;
    let Some(root) = ATEXIT_ROOT.get() else {
        return;
    };
    let path = Path::new(std::ffi::OsStr::from_bytes(
        root.as_bytes_with_nul().split_last().unwrap().1,
    ));
    let _ = std::fs::remove_dir_all(path);
}

/// Delete sandboxes belonging to processes that are no longer running.
///
/// A leftover tree is a tree of fixtures and trash entries that nothing will
/// ever claim, and the pid in the name is what makes the sweep safe: a
/// concurrent `cargo test` on the same machine has a live pid and is skipped.
/// Linux-only by way of `/proc`, which this crate already assumes.
fn sweep_abandoned_siblings(keep: &Path) {
    let Some(parent) = keep.parent() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|name| name.strip_prefix("sandbox-"))
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == std::process::id() || Path::new("/proc").join(pid.to_string()).exists() {
            continue;
        }
        let _ = std::fs::remove_dir_all(entry.path());
    }
}

fn root_path() -> PathBuf {
    // `TMPDIR_REAL` stays the escape hatch: a caller who needs a different
    // filesystem says so in the environment, as the trash tests always have.
    if let Some(base) = std::env::var_os("TMPDIR_REAL") {
        return PathBuf::from(base).join(format!("sandbox-{}", std::process::id()));
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("ux-fixtures")
        .join(format!("sandbox-{}", std::process::id()))
}

/// Point `XDG_PICTURES_DIR`/`XDG_VIDEOS_DIR` at the sandbox home.
///
/// Without this `pictures_dir()` falls through to `$HOME/Pictures`, which
/// resolves inside the sandbox anyway — but writing the file makes that a
/// decision rather than an accident, and it stops `user-dirs.dirs` from being
/// read out of a real `$HOME` if the sandbox home is ever pointed elsewhere.
fn seed_user_dirs(home: &Path) {
    let config = home.join(".config");
    std::fs::create_dir_all(&config)
        .expect("the sandbox home config directory should be creatable");
    std::fs::write(
        config.join("user-dirs.dirs"),
        format!(
            "XDG_DESKTOP_DIR=\"{}\"\nXDG_PICTURES_DIR=\"{}\"\nXDG_VIDEOS_DIR=\"{}\"\n",
            home.join("Desktop").display(),
            home.join("Pictures").display(),
            home.join("Videos").display()
        ),
    )
    .expect("the sandbox user-dirs.dirs should be writable");
}

fn assert_same_device(a: &Path, b: &Path, label: &str) {
    use std::os::unix::fs::MetadataExt;
    let (Ok(a_meta), Ok(b_meta)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return;
    };
    assert_eq!(
        a_meta.dev(),
        b_meta.dev(),
        "the sandbox {label} at {} and the sandbox root at {} are on different \
         filesystems (device {} vs {}). GIO only uses $XDG_DATA_HOME/Trash for \
         files on the $HOME filesystem; otherwise it falls back to \
         $topdir/.Trash-$uid, which writes outside this sandbox and records a \
         relative path the app cannot resolve back. Point TMPDIR_REAL at a \
         directory on the same filesystem as $HOME.",
        a.display(),
        b.display(),
        a_meta.dev(),
        b_meta.dev()
    );
}

fn set_env(key: &str, value: PathBuf) {
    std::env::set_var(key, value);
}

/// Apply one already-fetched native bootstrap page through the production
/// receive path. Used by isolated tests and read-only-service diagnostics.
pub async fn apply_native_sync_page(
    service: &crate::core::sync::SyncService,
    job_id: i64,
    record_type: &str,
    page: &crate::core::sync::native::receive::QueryResponse,
    record_count: usize,
) -> crate::core::error::Result<(usize, u64, u64, u64)> {
    let job = service
        .store()
        .get_job(job_id)?
        .ok_or_else(|| crate::core::error::AppError::Backend("job removed".into()))?;
    if !service.store().job_is_selected(job_id)? {
        return Err(crate::core::error::AppError::Backend(
            "account is inactive".into(),
        ));
    }
    let zone = job
        .native_zone
        .ok_or_else(|| crate::core::error::AppError::Backend("job has no zone".into()))?;
    let mut report = crate::core::sync::service::receive::ReceiveReport {
        job_id,
        ..Default::default()
    };
    let timing = service
        .apply_native_bootstrap_page(
            job_id,
            &zone,
            record_type,
            &page.records,
            &page.sync_token,
            &mut report,
        )
        .await
        .map_err(|e| crate::core::error::AppError::Backend(e.to_string()))?;
    let started = std::time::Instant::now();
    service.store().save_bootstrap_page(
        job_id,
        record_type,
        page.continuation_marker.clone(),
        record_count,
        &page.sync_token,
    )?;
    Ok((
        timing.saved,
        timing.persist_ms,
        timing.apply_ms,
        started.elapsed().as_millis() as u64,
    ))
}
