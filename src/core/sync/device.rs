//! This installation's stable device identity.
//!
//! Every real edit event this client writes carries a `source`. It has to be
//! the same string for this installation across restarts, because it is what
//! makes two events from "the same device" comparable — a fresh random source
//! per run would make the same user's consecutive edits look like two devices
//! racing.
//!
//! It is not a secret and not a user identity: it exists so a conflict between
//! two installations can be attributed, not so either can be recognised.

use crate::core::db::DbPool;
use crate::core::error::{AppError, Result};

const DEVICE_ID_KEY: &str = "sync_device_id";

/// The device id for this installation, minted on first use and persisted.
///
/// Falls back to a per-process id when the database cannot be reached. That is
/// honest rather than convenient: a temporary database gets a temporary
/// device, and its events stay internally consistent.
pub fn device_id(pool: &DbPool) -> String {
    load_or_mint(pool).unwrap_or_else(|_| uuid::Uuid::new_v4().to_string())
}

fn load_or_mint(pool: &DbPool) -> Result<String> {
    let conn = pool.get()?;
    device_id_in(&conn)
}

pub(crate) fn device_id_in(conn: &rusqlite::Connection) -> Result<String> {
    let minted = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO NOTHING",
        rusqlite::params![DEVICE_ID_KEY, minted],
    )?;
    conn.query_row(
        "SELECT value FROM settings WHERE key=?1",
        [DEVICE_ID_KEY],
        |r| r.get(0),
    )
    .map_err(Into::into)
}

/// The current wall-clock instant in milliseconds, for a real edit event.
///
/// This is only ever a *user action* time. It is never substituted for a
/// missing remote event, and never used to describe a file's content.
pub fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// A fresh event id. Distinct per edit, so two events with the same timestamp
/// from the same device are still distinguishable.
pub fn new_event_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// The failure text a caller sees when a device identity is required but the
/// database is unreachable.
pub fn unavailable() -> AppError {
    AppError::Backend("no database is available to identify this device".into())
}

#[cfg(test)]
mod tests;
