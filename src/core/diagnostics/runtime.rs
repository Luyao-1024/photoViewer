use super::storage::{Entry, Store};
use anyhow::{anyhow, Result};
use std::{
    fmt,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender},
        Arc,
    },
    thread::{self, JoinHandle},
};
use tracing::{
    field::{Field, Visit},
    Event, Subscriber,
};
use tracing_subscriber::{layer::Context, Layer};

enum Command {
    Event(Entry),
    Flush(mpsc::Sender<Result<()>>),
    Clear(mpsc::Sender<Result<()>>),
    Limit(u64, mpsc::Sender<Result<()>>),
    Stop,
}
#[derive(Clone)]
pub(super) struct Logger {
    sender: SyncSender<Command>,
    dropped: Arc<AtomicU64>,
}
pub(super) struct Worker {
    logger: Logger,
    thread: Option<JoinHandle<()>>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.logger.sender.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl Logger {
    pub fn start(dir: &Path, limit: u64) -> Result<Option<(Self, Worker)>> {
        let Some(mut store) = Store::open(dir, limit)? else {
            return Ok(None);
        };
        let (sender, receiver) = mpsc::sync_channel(1024);
        let logger = Logger {
            sender,
            dropped: Arc::new(AtomicU64::new(0)),
        };
        let dropped = logger.dropped.clone();
        let thread = thread::Builder::new().name("diagnostics-writer".into()).spawn(move || {
            while let Ok(command) = receiver.recv() {
                match command {
                    Command::Event(entry) => {
                        let lost = dropped.swap(0, Ordering::Relaxed);
                        if lost > 0 {
                            let warning = Entry { time_ms: entry.time_ms, day: entry.day.clone(), level: "WARN".into(), target: "diagnostics".into(), message: format!("{lost} log events dropped because the logging queue was full") };
                            if let Err(error) = store.append(&warning) { eprintln!("log storage: {error}"); }
                        }
                        if let Err(error) = store.append(&entry) { eprintln!("log storage: {error}"); }
                    },
                    Command::Flush(reply) => { let _ = reply.send(store.flush()); },
                    Command::Clear(reply) => { let _ = reply.send(store.clear()); },
                    Command::Limit(limit, reply) => { let _ = reply.send(store.resize(limit)); },
                    Command::Stop => break,
                }
            }
        })?;
        let worker = Worker {
            logger: logger.clone(),
            thread: Some(thread),
        };
        Ok(Some((logger, worker)))
    }
    pub fn flush(&self) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        self.sender
            .try_send(Command::Flush(tx))
            .map_err(|_| anyhow!("log queue unavailable"))?;
        rx.recv_timeout(std::time::Duration::from_secs(1))
            .map_err(|_| anyhow!("log flush timed out"))?
    }
    pub fn clear(&self) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        self.sender
            .send(Command::Clear(tx))
            .map_err(|_| anyhow!("log worker stopped"))?;
        rx.recv().map_err(|_| anyhow!("log worker stopped"))?
    }
    pub fn resize(&self, limit: u64) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        self.sender
            .send(Command::Limit(limit, tx))
            .map_err(|_| anyhow!("log worker stopped"))?;
        rx.recv().map_err(|_| anyhow!("log worker stopped"))?
    }
}
struct Fields(String);
impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        // Bound before formatting, so a single giant Debug value cannot build an
        // unbounded producer buffer. Credentials are never retained by name.
        if self.0.len() >= 16 * 1024 {
            return;
        }
        use fmt::Write;
        let sensitive = [
            "password",
            "access_token",
            "api_key",
            "authorization",
            "lease_token",
        ]
        .contains(&field.name());
        let mut writer = Limited(&mut self.0);
        if sensitive {
            let _ = write!(writer, " {}=[redacted]", field.name());
        } else {
            let _ = write!(writer, " {}={value:?}", field.name());
        }
    }
}
struct Limited<'a>(&'a mut String);
impl fmt::Write for Limited<'_> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let mut end = value
            .len()
            .min((16 * 1024_usize).saturating_sub(self.0.len()));
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        self.0.push_str(&value[..end]);
        Ok(())
    }
}
impl<S: Subscriber> Layer<S> for Logger {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut fields = Fields(String::new());
        event.record(&mut fields);
        let now = chrono::Local::now();
        let entry = Entry {
            time_ms: now.timestamp_millis(),
            day: now.format("%Y-%m-%d").to_string(),
            level: event.metadata().level().to_string(),
            target: event.metadata().target().to_owned(),
            message: fields.0,
        };
        if self.sender.try_send(Command::Event(entry)).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests;
