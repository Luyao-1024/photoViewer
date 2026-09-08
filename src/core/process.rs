//! Bounded subprocess execution for untrusted media probes/thumbnailers.
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

pub const MEDIA_TIMEOUT: Duration = Duration::from_secs(15);
const OUTPUT_LIMIT: u64 = 4 * 1024 * 1024;

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        // Each command owns a new process group, including helper descendants.
        // SAFETY: the negative pid identifies only the group created below.
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn output(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    let stdout = tempfile::NamedTempFile::new()?;
    let stderr = tempfile::NamedTempFile::new()?;
    command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout.reopen()?))
        .stderr(Stdio::from(stderr.reopen()?));
    let mut child = ChildGuard(command.spawn()?);
    let started = Instant::now();
    let status = loop {
        if stdout.as_file().metadata()?.len() > OUTPUT_LIMIT
            || stderr.as_file().metadata()?.len() > OUTPUT_LIMIT
        {
            return Err(io::Error::other("media command exceeded output limit"));
        }
        if let Some(status) = child.0.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "media command timed out",
            ));
        }
        std::thread::sleep(
            Duration::from_millis(10).min(timeout.saturating_sub(started.elapsed())),
        );
    };
    // Kill any descendant still holding the output before reading bounded data.
    drop(child);
    let read = |file: &tempfile::NamedTempFile| -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        file.reopen()?.take(OUTPUT_LIMIT).read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    Ok(Output {
        status,
        stdout: read(&stdout)?,
        stderr: read(&stderr)?,
    })
}

#[cfg(test)]
mod tests;
