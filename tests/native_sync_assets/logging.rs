//! Capture normal-level sync events while the real current-thread cycle runs.
use std::io::Write;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub(super) struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Capture {
    pub(super) fn install() -> (Self, tracing::subscriber::DefaultGuard) {
        let capture = Self::default();
        let writer = capture.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter("sync=info")
            .with_ansi(false)
            .without_time()
            .with_writer(move || writer.clone())
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        (capture, guard)
    }

    pub(super) fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }

    pub(super) fn lines(&self, message: &str) -> Vec<String> {
        self.text()
            .lines()
            .filter(|line| line.contains(message))
            .map(str::to_owned)
            .collect()
    }
}
