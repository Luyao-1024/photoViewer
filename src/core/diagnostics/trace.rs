//! Keep opt-in Perfetto output bounded without cutting a JSON event in half.
use std::{
    fs::File,
    io::{self, Seek, Write},
    sync::{Arc, Mutex, OnceLock},
};
static CONTROL: OnceLock<Arc<Mutex<State>>> = OnceLock::new();
struct State {
    file: File,
    pending: Vec<u8>,
    overflow: bool,
    first: bool,
    written: u64,
    limit: u64,
}
pub(super) struct Writer(Arc<Mutex<State>>);
impl Writer {
    pub fn new(file: File, limit: u64) -> io::Result<Self> {
        let state = Arc::new(Mutex::new(State {
            file,
            pending: Vec::new(),
            overflow: false,
            first: true,
            written: 0,
            limit,
        }));
        let _ = CONTROL.set(state.clone());
        Ok(Self(state))
    }
}
impl State {
    fn line(&mut self) -> io::Result<()> {
        let line = std::mem::take(&mut self.pending);
        if self.overflow {
            self.overflow = false;
            return Ok(());
        }
        let text = std::str::from_utf8(&line)
            .unwrap_or("")
            .trim()
            .trim_end_matches(',');
        if text == "[" {
            if self.written == 0 {
                self.file.write_all(b"[\n")?;
                self.written = 2;
            }
        } else if text == "]" {
            self.file.write_all(b"\n]")?;
            self.written += 2;
        } else if text.starts_with('{')
            && text.ends_with('}')
            && self.written + text.len() as u64 + 4 <= self.limit
        {
            if !self.first {
                self.file.write_all(b",\n")?;
                self.written += 2;
            }
            self.file.write_all(text.as_bytes())?;
            self.written += text.len() as u64;
            self.first = false;
        }
        Ok(())
    }
    fn reset(&mut self, limit: u64) -> io::Result<()> {
        self.file.set_len(0)?;
        self.file.rewind()?;
        self.file.write_all(b"[\n")?;
        self.written = 2;
        self.first = true;
        self.pending.clear();
        self.overflow = false;
        self.limit = limit;
        Ok(())
    }
}
impl Write for Writer {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| io::Error::other("trace writer poisoned"))?;
        for &byte in data {
            if byte == b'\n' {
                state.line()?;
            } else if state.pending.len() < 64 * 1024 {
                state.pending.push(byte);
            } else {
                state.overflow = true;
            }
        }
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| io::Error::other("trace writer poisoned"))?;
        if state.pending.as_slice() == b"]" {
            state.line()?;
        }
        state.file.flush()
    }
}
pub(super) fn limit(bytes: u64) -> u64 {
    (bytes / 16).min(8 * 1024 * 1024)
}
pub(super) fn reset(bytes: u64) -> io::Result<()> {
    if let Some(state) = CONTROL.get() {
        state
            .lock()
            .map_err(|_| io::Error::other("trace writer poisoned"))?
            .reset(limit(bytes))?;
    }
    Ok(())
}
#[cfg(test)]
mod tests;
