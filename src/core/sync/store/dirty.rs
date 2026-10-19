//! Dirty truth: what the user changed locally, and what may clear it.
//!
//! A dirty bit is a *field on the existing record*, never a queue. The uplink
//! view is derived from those fields at dispatch time, so a change made while
//! an operation is in flight is visible to the next dispatch rather than lost
//! behind a snapshot that was taken before it.
//!
//! Three categories are tracked independently because they are uploaded in
//! different ways and cleared by different evidence:
//!
//! * **metadata** — a supported field changed (favorite, path, capture date).
//!   A conditional record update; no resource is involved.
//! * **content** — the original bytes changed. Needs a new asset pair, so it is
//!   only recorded here and acted on in C1.4.
//! * **deletion** — an explicit remote-delete intent. Never inferred from a row
//!   disappearing, a scope change, or an unreachable drive.
//!
//! Each category carries its own version counter, its own acknowledged
//! counter, and its own edit event. Clearing a category requires proof that
//! *that* category was accepted at *that* version: an edit made while the
//! operation was in flight advances the version, so the callback no longer
//! matches and the obligation correctly survives.
//!
//! `native_dirty` is one operation state: 0=clean, 1=new, 2=modified,
//! 4=permanently deleted. Content and metadata share state 2; their independent
//! version/acknowledgement counters decide which payload the update needs.
//! Moving to or restoring from trash is metadata, never deletion. A callback
//! settles only its captured versions and preserves edits made in flight.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::core::db::DbPool;
use crate::core::error::{AppError, Result};

use super::{SyncStore, SyncWrite, SyncWriteResult};

/// Pending operation state on `sync_entries.native_dirty`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NativeDirty(pub u8);

impl NativeDirty {
    pub const CLEAN: u8 = 0;
    pub const NEW: u8 = 1;
    pub const MODIFIED: u8 = 2;
    pub const METADATA: u8 = Self::MODIFIED;
    pub const CONTENT: u8 = Self::MODIFIED;
    pub const DELETE: u8 = 4;

    pub fn bits(self) -> u8 {
        self.0
    }

    pub fn contains(self, bit: u8) -> bool {
        self.0 == bit
    }

    pub fn with(self, bit: u8) -> Self {
        Self(if self.0 == Self::DELETE || bit == Self::DELETE {
            Self::DELETE
        } else if self.0 == Self::NEW || bit == Self::NEW {
            Self::NEW
        } else {
            bit
        })
    }

    pub fn without(self, bit: u8) -> Self {
        Self(if self.0 == bit { Self::CLEAN } else { self.0 })
    }

    pub fn is_clean(self) -> bool {
        self.0 == Self::CLEAN
    }

    pub fn names(self) -> Vec<&'static str> {
        match self.0 {
            Self::NEW => vec!["new"],
            Self::MODIFIED => vec!["modified"],
            Self::DELETE => vec!["delete"],
            _ => Vec::new(),
        }
    }
}

/// The three independently tracked change categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeCategory {
    Metadata,
    Content,
    Deletion,
}

impl ChangeCategory {
    pub fn bit(self) -> u8 {
        match self {
            Self::Metadata => NativeDirty::METADATA,
            Self::Content => NativeDirty::CONTENT,
            Self::Deletion => NativeDirty::DELETE,
        }
    }

    fn version_column(self) -> &'static str {
        match self {
            Self::Metadata => "native_metadata_version",
            Self::Content => "native_content_version",
            Self::Deletion => "native_deletion_version",
        }
    }

    #[cfg(test)]
    fn acked_column(self) -> &'static str {
        match self {
            Self::Metadata => "native_acked_metadata_version",
            Self::Content => "native_acked_content_version",
            Self::Deletion => "native_acked_deletion_version",
        }
    }
}

/// A real user edit, as the service records it.
///
/// `edited_at_ms` is the moment the user acted. It is never the file's mtime,
/// the capture date, the moment the row was read, or the moment a migration
/// touched it — those are all facts about something other than the edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditEventStamp {
    pub edited_at_ms: i64,
    pub source: String,
    pub event_id: String,
}

/// One entry's outstanding obligations, captured for one dispatch.
///
/// The captured versions are the whole point: a callback that comes back for
/// this capture may only clear this obligation if the category's version still
/// equals what was sent. A newer edit leaves the newer obligation standing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirtyEntry {
    pub entry_id: i64,
    pub job_id: i64,
    pub relative_path: String,
    pub dirty: NativeDirty,
    pub local_version: i64,
    pub photo_id: Option<String>,
    pub master_id: Option<String>,
    pub asset_revision: Option<String>,
    pub master_revision: Option<String>,
    pub content_generation: Option<String>,
    pub metadata_version: i64,
    pub content_version: i64,
    pub deletion_version: i64,
    pub acked_metadata_version: i64,
    pub acked_content_version: i64,
    pub acked_deletion_version: i64,
    pub metadata_event: Option<EditEventStamp>,
    pub content_event: Option<EditEventStamp>,
    pub deletion_event: Option<EditEventStamp>,
}

impl DirtyEntry {
    pub fn has_metadata_change(&self) -> bool {
        self.metadata_version > self.acked_metadata_version
    }
    pub fn has_content_change(&self) -> bool {
        self.content_version > self.acked_content_version
    }
    pub fn version_for(&self, category: ChangeCategory) -> i64 {
        match category {
            ChangeCategory::Metadata => self.metadata_version,
            ChangeCategory::Content => self.content_version,
            ChangeCategory::Deletion => self.deletion_version,
        }
    }

    pub fn event_for(&self, category: ChangeCategory) -> Option<&EditEventStamp> {
        match category {
            ChangeCategory::Metadata => self.metadata_event.as_ref(),
            ChangeCategory::Content => self.content_event.as_ref(),
            ChangeCategory::Deletion => self.deletion_event.as_ref(),
        }
    }

    /// The record names an upload would address. Absent for a photo that has
    /// never been created remotely, which is the `NEW` case.
    pub fn remote_names(&self) -> Option<(&str, &str)> {
        match (&self.photo_id, &self.master_id) {
            (Some(photo), Some(master)) => Some((photo.as_str(), master.as_str())),
            _ => None,
        }
    }
}

/// Wipe every obligation for a category once, or leave it exactly as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CategoryOutcome {
    /// The captured version still matched, so this obligation is cleared.
    Cleared,
    /// A newer edit arrived while the operation was in flight; it stays dirty.
    Superseded,
}

#[derive(Debug)]
pub enum DirtyOutcome {
    /// The media row and its obligation were written in one transaction.
    Recorded {
        entry_id: i64,
        dirty: NativeDirty,
    },
    Entries(Vec<DirtyEntry>),
    Confirmed(CategoryOutcome),
}

#[derive(Debug)]
pub enum DirtyCommand {
    /// Record a real local change against every enabled native job whose root
    /// covers this media. No jobs covering it is the normal case and writes
    /// nothing.
    NoteLocalChange {
        media_id: i64,
        category: ChangeCategory,
        stamp: EditEventStamp,
    },
    /// Derive the in-flight obligations for one job, oldest entry first.
    UplinkView { job_id: i64 },
    /// Confirm one category of one entry against the version that was sent.
    #[cfg(test)]
    Confirm {
        entry_id: i64,
        category: ChangeCategory,
        captured_version: i64,
        photo_id: Option<String>,
        master_id: Option<String>,
        asset_revision: Option<String>,
        master_revision: Option<String>,
        content_generation: Option<String>,
    },
}

impl SyncStore {
    fn write_dirty(&self, command: DirtyCommand) -> Result<DirtyOutcome> {
        match self.write(SyncWrite::Dirty(Box::new(command)))? {
            SyncWriteResult::Dirty(outcome) => Ok(outcome),
            _ => Err(AppError::Backend(
                "synchronization dirty write returned an unexpected result".into(),
            )),
        }
    }

    /// Record a real local change inside a transaction the caller already owns.
    ///
    /// This is the form every mutation path uses: the caller's transaction
    /// already holds the media write, and the obligation joins it. A change
    /// nobody syncs to and an obligation for a change nobody made are both
    /// worse than neither, so they are written together or not at all.
    pub fn note_local_change_in(
        tx: &rusqlite::Transaction<'_>,
        media_id: i64,
        category: ChangeCategory,
        stamp: &EditEventStamp,
    ) -> Result<Option<DirtyOutcome>> {
        note(tx, media_id, category, stamp)
    }

    /// Outstanding obligations for one job, in the order they should be sent.
    pub fn uplink_view(&self, job_id: i64) -> Result<Vec<DirtyEntry>> {
        match self.write_dirty(DirtyCommand::UplinkView { job_id })? {
            DirtyOutcome::Entries(entries) => Ok(entries),
            _ => Err(AppError::Backend(
                "reading the uplink view returned an unexpected result".into(),
            )),
        }
    }

    /// The current local version of one dirty entry, if it is still dirty.
    ///
    /// Dispatch already holds the ordered uplink snapshot. This lookup is the
    /// narrow recheck used around asynchronous preparation, so one changed
    /// entry does not force every remaining entry to be re-read.
    pub(crate) fn uplink_entry_version(&self, job_id: i64, entry_id: i64) -> Result<Option<i64>> {
        self.pool
            .get()?
            .query_row(
                "SELECT native_local_version
                   FROM sync_entries
                  WHERE job_id = ?1 AND id = ?2 AND native_dirty <> 0",
                params![job_id, entry_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    /// Confirm one category against the version that was actually sent.
    #[cfg(test)]
    pub fn confirm_category(
        &self,
        entry_id: i64,
        category: ChangeCategory,
        captured_version: i64,
        identity: ConfirmedIdentity,
    ) -> Result<CategoryOutcome> {
        match self.write_dirty(DirtyCommand::Confirm {
            entry_id,
            category,
            captured_version,
            photo_id: identity.photo_id,
            master_id: identity.master_id,
            asset_revision: identity.asset_revision,
            master_revision: identity.master_revision,
            content_generation: identity.content_generation,
        })? {
            DirtyOutcome::Confirmed(outcome) => Ok(outcome),
            _ => Err(AppError::Backend(
                "confirming a category returned an unexpected result".into(),
            )),
        }
    }
}

/// Cloud identity a successful callback registers. A confirmed create records
/// it even when later edits keep the entry dirty, which is what converts the
/// residual work to an update instead of another create.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfirmedIdentity {
    pub photo_id: Option<String>,
    pub master_id: Option<String>,
    pub asset_revision: Option<String>,
    pub master_revision: Option<String>,
    pub content_generation: Option<String>,
}

fn note(
    tx: &rusqlite::Transaction<'_>,
    media_id: i64,
    category: ChangeCategory,
    stamp: &EditEventStamp,
) -> Result<Option<DirtyOutcome>> {
    // Account selection survives global Off. It must not stand in for consent
    // to record new local changes while history is being cleared or stays off.
    if super::reset::pending(tx)? || !super::reset::local_changes_enabled(tx)? {
        return Ok(None);
    }
    // Only enabled jobs whose local root actually covers this media may claim
    // it. Enrolling a path is an explicit user choice, never a side effect.
    let mut entries: Vec<(i64, Option<i64>)> = Vec::new();
    {
        let mut stmt = tx.prepare(
            "SELECT j.id, e.id
             FROM sync_jobs j
             JOIN sync_connections c ON c.id = j.connection_id
             JOIN media_items m ON m.id = ?1
                       AND substr(m.path, 1, length(rtrim(j.local_root, '/')) + 1)
                           = rtrim(j.local_root, '/') || '/'
             LEFT JOIN sync_entries e
                    ON e.job_id = j.id
                   AND e.media_id = m.id
                   AND e.relative_path = substr(m.path, length(rtrim(j.local_root, '/')) + 2)
              WHERE c.enabled = 1
                AND (j.upload_scope='all' OR EXISTS (
                    SELECT 1 FROM sync_job_upload_albums a WHERE a.job_id=j.id
                    AND m.folder_path=CASE WHEN a.relative_album='' THEN rtrim(j.local_root,'/')
                        ELSE rtrim(j.local_root,'/')||'/'||a.relative_album END))",
        )?;
        let rows = stmt.query_map([media_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
        })?;
        for row in rows {
            entries.push(row?);
        }
    }

    let mut recorded = Vec::new();
    for (job_id, entry_id) in entries {
        if let Some(id) = entry_id {
            let retired: bool = tx.query_row(
                "SELECT native_retired FROM sync_entries WHERE id=?1",
                [id],
                |r| r.get(0),
            )?;
            if retired && category != ChangeCategory::Deletion {
                continue;
            }
        }
        let entry_id = match entry_id {
            Some(id) => id,
            // Off discarded the old sync history. Startup pruning can observe
            // files removed during that interval only after opt-in; an index
            // row alone cannot establish a cloud deletion obligation.
            None if category == ChangeCategory::Deletion => continue,
            // New content and edits inside the selected scope enroll normally.
            None => {
                let path = media_path(tx, media_id)?;
                let relative = relative_under(tx, job_id, &path)?;
                tx.execute(
                    "INSERT INTO sync_entries(job_id, media_id, relative_path, native_dirty)
                     VALUES(?1, ?2, ?3, ?4)",
                    params![job_id, media_id, relative, NativeDirty::NEW],
                )?;
                tx.last_insert_rowid()
            }
        };
        recorded.push(mark(tx, job_id, entry_id, category, stamp)?);
    }

    Ok(match recorded.len() {
        0 => None,
        _ => {
            // A change nobody syncs — a file outside every job root, or one
            // written while synchronization is off — never reaches `recorded`,
            // so it never schedules a run either. Only a change that became an
            // obligation restarts the quiet period.
            crate::core::sync::debounce::note_local_change();
            Some(DirtyOutcome::Recorded {
                entry_id: recorded[0].0,
                dirty: recorded[0].1,
            })
        }
    })
}

fn mark(
    tx: &rusqlite::Transaction<'_>,
    _job_id: i64,
    entry_id: i64,
    category: ChangeCategory,
    stamp: &EditEventStamp,
) -> Result<(i64, NativeDirty)> {
    let bit = category.bit();
    // `native_local_version` advances on every real change; the category
    // counter advances for the category that changed. The acknowledged counter
    // never moves here, so an in-flight operation's captured version stops
    // matching the moment the user edits again.
    //
    // `mark` is the single funnel every local change passes through, so it is
    // also where a cached digest of the local original stops being true. Any
    // real edit here is a statement that the bytes are no longer the ones we
    // measured.
    tx.execute(
        &format!(
            "UPDATE sync_entries
                SET native_dirty = CASE WHEN ?2=4 THEN 4
                        WHEN native_dirty=4 THEN 4
                        WHEN native_photo_id IS NULL THEN 1 ELSE 2 END,
                    native_local_version = native_local_version + 1,
                    native_digest_entry_id=NULL,native_digest_version=NULL,native_digest_sha256=NULL,native_digest_size=NULL,
                    {version} = {version} + 1,
                    native_{event}_edited_ms=?3,native_{event}_source=?4,native_{event}_event_id=?5,
                    updated_at = unixepoch()
              WHERE id = ?1",
            version = category.version_column(),
            event = match category {ChangeCategory::Metadata=>"metadata",ChangeCategory::Content=>"content",ChangeCategory::Deletion=>"deletion"}
        ),
        params![
            entry_id,
            bit,
            stamp.edited_at_ms,stamp.source,stamp.event_id
        ],
    )?;
    let dirty: i64 = tx.query_row(
        "SELECT native_dirty FROM sync_entries WHERE id = ?1",
        [entry_id],
        |row| row.get(0),
    )?;
    Ok((entry_id, NativeDirty(dirty as u8)))
}

fn read_entries(
    conn: &rusqlite::Connection,
    job_id: i64,
    generation: Option<&str>,
) -> Result<Vec<DirtyEntry>> {
    // `generation` scopes the read to the cloud the entry was last confirmed
    // against. When the server database was rebuilt, entries from another
    // generation are still reported — they remain dirty obligations — but they
    // carry no revision to compare against.
    let mut stmt = conn.prepare(
        "SELECT id, job_id, relative_path, native_dirty, native_local_version,
                native_photo_id, native_master_id, native_asset_revision,
                native_master_revision, native_content_generation,
                native_metadata_version, native_content_version, native_deletion_version,
                native_acked_metadata_version,native_acked_content_version,native_acked_deletion_version
         FROM sync_entries
         WHERE job_id = ?1 AND native_dirty <> 0
         ORDER BY id",
    )?;
    let rows = stmt.query_map([job_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, Option<String>>(9)?,
            row.get::<_, i64>(10)?,
            row.get::<_, i64>(11)?,
            row.get::<_, i64>(12)?,
            row.get::<_, i64>(13)?,
            row.get::<_, i64>(14)?,
            row.get::<_, i64>(15)?,
        ))
    })?;
    let mut entries = Vec::new();
    for row in rows {
        let r = row?;
        let _ = generation;
        let events = super::records::read_events(conn, r.0)?;
        let stamp = |e: super::super::native::receive::EditEvent| EditEventStamp {
            edited_at_ms: e.edited_at_ms,
            source: e.source,
            event_id: e.event_id,
        };
        entries.push(DirtyEntry {
            entry_id: r.0,
            job_id: r.1,
            relative_path: r.2,
            dirty: NativeDirty(r.3 as u8),
            local_version: r.4,
            photo_id: r.5,
            master_id: r.6,
            asset_revision: r.7,
            master_revision: r.8,
            content_generation: r.9,
            metadata_version: r.10,
            content_version: r.11,
            deletion_version: r.12,
            metadata_event: events.metadata.map(stamp),
            content_event: events.content.map(stamp),
            deletion_event: events.deletion.map(stamp),
            acked_metadata_version: r.13,
            acked_content_version: r.14,
            acked_deletion_version: r.15,
        });
    }
    Ok(entries)
}

fn media_path(tx: &rusqlite::Transaction<'_>, media_id: i64) -> Result<String> {
    tx.query_row(
        "SELECT path FROM media_items WHERE id = ?1",
        [media_id],
        |row| row.get(0),
    )
    .optional()?
    .ok_or_else(|| AppError::Backend(format!("media item {media_id} no longer exists")))
}

fn relative_under(tx: &rusqlite::Transaction<'_>, job_id: i64, path: &str) -> Result<String> {
    let root: String = tx.query_row(
        "SELECT local_root FROM sync_jobs WHERE id = ?1",
        [job_id],
        |row| row.get(0),
    )?;
    let root = root.trim_end_matches('/');
    path.strip_prefix(root)
        .and_then(|rest| rest.strip_prefix('/'))
        .map(str::to_string)
        .ok_or_else(|| AppError::Backend("a media path is outside its job root".into()))
}

/// Record the cloud identity a confirmed operation established. Only fields
/// the answer actually carried are written; the rest keep whatever was already
/// known rather than being blanked.
#[cfg(test)]
fn register_identity(
    tx: &rusqlite::Transaction<'_>,
    entry_id: i64,
    identity: &ConfirmedIdentity,
) -> Result<()> {
    let Some(photo_id) = identity.photo_id.as_deref() else {
        return Ok(());
    };
    tx.execute(
        "UPDATE sync_entries SET
            native_photo_id  = COALESCE(?2, native_photo_id),
            native_master_id = COALESCE(?3, native_master_id),
            native_asset_revision = COALESCE(?4, native_asset_revision),
            native_master_revision = COALESCE(?5, native_master_revision),
            native_content_generation = COALESCE(?6, native_content_generation),
            updated_at = unixepoch()
          WHERE id = ?1",
        params![
            entry_id,
            photo_id,
            identity.master_id.as_deref(),
            identity.asset_revision.as_deref(),
            identity.master_revision.as_deref(),
            identity.content_generation.as_deref(),
        ],
    )?;
    Ok(())
}

pub(super) fn execute(pool: &DbPool, command: &DirtyCommand) -> Result<DirtyOutcome> {
    match command {
        DirtyCommand::NoteLocalChange {
            media_id,
            category,
            stamp,
        } => {
            let mut conn = pool.get()?;
            // The caller's own transaction supplies the media write; this one
            // only reads the path and writes the obligation alongside it.
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let outcome = note(&tx, *media_id, *category, stamp)?;
            tx.commit()?;
            Ok(outcome.unwrap_or(DirtyOutcome::Entries(Vec::new())))
        }
        DirtyCommand::UplinkView { job_id } => {
            let conn = pool.get()?;
            Ok(DirtyOutcome::Entries(read_entries(&conn, *job_id, None)?))
        }
        #[cfg(test)]
        DirtyCommand::Confirm {
            entry_id,
            category,
            captured_version,
            photo_id,
            master_id,
            asset_revision,
            master_revision,
            content_generation,
        } => confirm(
            pool,
            *entry_id,
            *category,
            *captured_version,
            ConfirmedIdentity {
                photo_id: photo_id.clone(),
                master_id: master_id.clone(),
                asset_revision: asset_revision.clone(),
                master_revision: master_revision.clone(),
                content_generation: content_generation.clone(),
            },
        ),
    }
}

#[cfg(test)]
fn confirm(
    pool: &DbPool,
    entry_id: i64,
    category: ChangeCategory,
    captured_version: i64,
    identity: ConfirmedIdentity,
) -> Result<DirtyOutcome> {
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    let current: i64 = tx.query_row(
        &format!(
            "SELECT {version} FROM sync_entries WHERE id = ?1",
            version = category.version_column()
        ),
        [entry_id],
        |row| row.get(0),
    )?;
    if current != captured_version {
        // A newer edit landed while the operation was in flight. Its obligation
        // stands — but the cloud identity this operation *did* establish is
        // still fact, and recording it is what converts the residual work to an
        // update instead of a second create.
        register_identity(&tx, entry_id, &identity)?;
        tx.execute(
            &format!(
                "UPDATE sync_entries SET {acked} = {captured}
                  WHERE id = ?1 AND {acked} < {captured}",
                acked = category.acked_column(),
                captured = captured_version
            ),
            [entry_id],
        )?;
        tx.commit()?;
        return Ok(DirtyOutcome::Confirmed(CategoryOutcome::Superseded));
    }

    // Register cloud identity even when the entry stays dirty: a confirmed
    // create whose later edits remain must convert to an update, never repeat
    // as another create.
    register_identity(&tx, entry_id, &identity)?;
    tx.execute(
        &format!(
            "UPDATE sync_entries SET {acked}=?2 WHERE id=?1",
            acked = category.acked_column()
        ),
        params![entry_id, captured_version],
    )?;
    tx.execute("UPDATE sync_entries SET native_dirty=CASE
        WHEN native_dirty=4 AND native_deletion_version>native_acked_deletion_version THEN 4
        WHEN native_photo_id IS NULL AND native_dirty=1 THEN 1
        WHEN native_metadata_version>native_acked_metadata_version OR native_content_version>native_acked_content_version THEN 2
        ELSE 0 END,updated_at=unixepoch() WHERE id=?1",[entry_id])?;
    tx.commit()?;
    Ok(DirtyOutcome::Confirmed(CategoryOutcome::Cleared))
}

#[cfg(test)]
mod tests;

/// Record a real local change inside a transaction the caller already owns.
///
/// Exposed at the module level because the mutation paths live in `core::db`,
/// which has no `SyncStore` of its own and must not acquire a second writer.
pub fn note_local_change_in(
    tx: &rusqlite::Transaction<'_>,
    media_id: i64,
    category: ChangeCategory,
    stamp: &EditEventStamp,
) -> Result<Option<DirtyOutcome>> {
    note(tx, media_id, category, stamp)
}
