//! Shared harness for running the real SyncServer binary inside a test.
//!
//! # Why this exists
//!
//! Several suites spawn a real `syncserver` process instead of mocking the
//! wire protocol. A plain `Command::spawn` plus a `Drop` that kills the child
//! is not enough, because a test run can end in ways that never run `Drop`:
//!
//! * `cargo test` is interrupted with Ctrl-C, and the harness kills the test
//!   binary before the destructors run.
//! * the test binary is killed outright (`SIGKILL`) or the machine reboots.
//! * the suite panics inside a spawned thread that outlives the test body.
//!
//! Any of those leaves the server running. It keeps its data-directory lock,
//! its listening socket and its memory, so the leak accumulates across runs:
//! a machine that ran the suite a handful of times ends up with dozens of
//! idle servers, and the next run cannot bind the port it expects.
//!
//! # The three guarantees
//!
//! 1. **Process group** — the child is started in its own process group
//!    (`setpgid`), so it does not receive the terminal's Ctrl-C (`SIGINT`)
//!    and does not die at an arbitrary point in the test. The harness, not
//!    the terminal, decides when it stops.
//! 2. **`PR_SET_PDEATHSIG`** — the child asks the kernel to send it
//!    `SIGKILL` when its parent dies. This is the backstop for the cases
//!    `Drop` cannot cover: the test binary is killed, and no destructor runs.
//!    The kernel does it, so it holds even for `SIGKILL` of the parent.
//! 3. **Explicit `Drop`** — the normal path. The server is asked to stop
//!    gracefully (`SIGTERM`, which the service handles by draining) and is
//!    then reaped. A server that ignores the request is killed outright.
//!
//! `PR_SET_PDEATHSIG` is delivered when the *parent thread* that set it
//! exits, not merely when the process exits. The suites here spawn from the
//! test's own thread and keep the handle on that same thread, so the signal
//! arrives exactly when the test is over.
//!
//! The helper deliberately does not put the child in a new session: a new
//! session would detach it from the controlling terminal entirely, which
//! changes how the service reacts to a terminal hangup.

#![allow(dead_code)]

use std::{
    io,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
};

/// Locate the server binary: an explicit override first, then the sibling
/// checkout's debug and release trees.
///
/// The sibling path is resolved relative to the crate root, not the current
/// working directory, so the helper works from any test's working directory.
pub fn server_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("SYNC_SERVER_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
        eprintln!("SYNC_SERVER_BIN={path:?} does not exist; ignoring it");
    }
    // `CARGO_MANIFEST_DIR` is `<checkout>/photoViewer`; the sibling checkout
    // is `<checkout>/syncserver`.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let sibling = manifest.parent()?.join("syncserver").join("target");
    for build in ["debug", "release"] {
        let path = sibling.join(build).join("syncserver");
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// The message the suites print when no server binary is available. A skipped
/// run is reported honestly rather than turned into a fake pass.
pub const MISSING_BINARY_HINT: &str =
    "skipping: no syncserver binary. Build the sibling checkout with \
     `cargo build --locked` (see native_probe_pins_a_real_m4_server_scope)";

/// A running server process that is guaranteed to be gone when the test ends.
///
/// The handle owns the child. Dropping it stops the server; leaking the
/// handle is not possible in safe Rust, and the kernel-level backstop covers
/// the case where the process never gets to run its destructors.
pub struct ServerProcess {
    child: Child,
    /// The process group the child leads. Signalling the group rather than
    /// the single pid reaches any helper the server itself started.
    group: i32,
}

impl ServerProcess {
    /// Spawn the server binary with `environment` applied on top of the
    /// inherited environment.
    ///
    /// `stdin`, `stdout` and `stderr` are all sent to `/dev/null`.
    pub fn spawn(binary: &Path, environment: &[(&str, &str)]) -> io::Result<Self> {
        Self::spawn_with(binary, environment, &[], Stdio::null(), Stdio::null())
    }

    /// Spawn the server binary, additionally removing `remove` from the
    /// inherited environment.
    ///
    /// A suite that drives the multi-user flow must not inherit a stray
    /// `SYNC_API_KEY_FILE` from the developer's shell: the presence of that
    /// variable is what selects the service's legacy single-account mode.
    pub fn spawn_without(
        binary: &Path,
        environment: &[(&str, &str)],
        remove: &[&str],
    ) -> io::Result<Self> {
        Self::spawn_with(binary, environment, remove, Stdio::null(), Stdio::null())
    }

    /// Spawn with explicit stdout and stderr. `stdin` stays closed so the
    /// server can never block waiting for input.
    pub fn spawn_with_stdio(
        binary: &Path,
        environment: &[(&str, &str)],
        stdout: Stdio,
        stderr: Stdio,
    ) -> io::Result<Self> {
        Self::spawn_with(binary, environment, &[], stdout, stderr)
    }

    fn spawn_with(
        binary: &Path,
        environment: &[(&str, &str)],
        remove: &[&str],
        stdout: Stdio,
        stderr: Stdio,
    ) -> io::Result<Self> {
        let mut command = Command::new(binary);
        command
            .env("SYNC_LOG_PAYLOAD", "off")
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr);
        for (key, value) in environment {
            command.env(key, value);
        }
        for key in remove {
            command.env_remove(key);
        }
        // SAFETY: `pre_exec` runs in the forked child before `execve`. The
        // closure only calls async-signal-safe syscalls (`setpgid`,
        // `prctl`), takes no locks and allocates nothing, which is what
        // `pre_exec` requires.
        unsafe {
            command.pre_exec(|| {
                // Own process group: the terminal's SIGINT must not reach the
                // server, so only the harness stops it.
                if libc::setpgid(0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                // Kernel backstop: if the parent dies without running its
                // destructors, the kernel kills this child. Without it an
                // interrupted or killed test run leaks the server.
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn()?;
        // The child is its own group leader, so its pid is the group id.
        let group = child.id() as i32;
        Ok(Self { child, group })
    }

    /// The server's pid.
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Whether the server is still running.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// Stop the server and reap it.
    ///
    /// `SIGTERM` first, because the service drains its state on that signal
    /// and releases the data-directory lock cleanly. A server that has not
    /// exited within [`ServerProcess::STOP_GRACE`] is killed, so a wedged
    /// process cannot hold the test suite open.
    pub fn stop(&mut self) {
        stop_process_group(self.group, &mut self.child);
    }

    /// How long a server is given to exit after `SIGTERM` before it is killed.
    pub const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(5);
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Run a one-shot server subcommand, such as `users create`, and return its
/// exit status.
///
/// The command is given the same process-group and parent-death protection as
/// a server, so an interrupted test cannot leave a half-finished
/// `users create` holding the data-directory lock.
pub fn run_cli(
    binary: &Path,
    arguments: &[&str],
    environment: &[(&str, &str)],
    remove: &[&str],
    stdin_data: &str,
) -> io::Result<ExitStatus> {
    use std::io::Write;

    let mut command = Command::new(binary);
    command
        .args(arguments)
        .env("SYNC_LOG_PAYLOAD", "off")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, value) in environment {
        command.env(key, value);
    }
    for key in remove {
        command.env_remove(key);
    }
    // SAFETY: as in `ServerProcess::spawn_with` — only async-signal-safe
    // syscalls, no locks, no allocation.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(stdin_data.as_bytes())?;
        stdin.write_all(b"\n")?;
    }
    child.wait()
}

/// Signal a process group and reap the direct child.
///
/// The group is signalled so that helpers the server spawned die with it.
/// The direct child is reaped regardless, so no zombie is left behind.
fn stop_process_group(group: i32, child: &mut Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    // SAFETY: `kill` with a negative pid signals the process group; the group
    // id was obtained from the child's own pid right after `spawn`.
    unsafe {
        libc::kill(-group, libc::SIGTERM);
    }
    let deadline = std::time::Instant::now() + ServerProcess::STOP_GRACE;
    while std::time::Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(_) => break,
        }
    }
    // SAFETY: as above; `SIGKILL` cannot be ignored or blocked.
    unsafe {
        libc::kill(-group, libc::SIGKILL);
    }
    let _ = child.wait();
}
