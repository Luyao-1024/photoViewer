//! User decisions are versioned writes on existing entries, never another queue.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct ManagedPhoto {
    pub entry_id: i64,
    pub relative_path: String,
    pub local_version: i64,
    pub remote: bool,
    pub retired: bool,
    pub conflict: Option<String>,
    pub path: Option<PathBuf>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum PhotoAction {
    DeleteCloud,
    Recreate,
    KeepLocal,
    UseCloud,
}

impl SyncStore {
    pub fn managed_photos(&self, job: i64) -> Result<Vec<ManagedPhoto>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare("SELECT e.id,e.relative_path,e.native_local_version,e.native_photo_id IS NOT NULL,e.native_retired,f.native_detail,m.path FROM sync_entries e LEFT JOIN sync_conflicts f ON f.entry_id=e.id AND f.state='open' LEFT JOIN media_items m ON m.id=e.media_id AND m.trashed_at IS NULL WHERE e.job_id=?1 ORDER BY e.relative_path")?;
        let rows = stmt
            .query_map([job], |r| {
                Ok(ManagedPhoto {
                    entry_id: r.get(0)?,
                    relative_path: r.get(1)?,
                    local_version: r.get(2)?,
                    remote: r.get(3)?,
                    retired: r.get(4)?,
                    conflict: r.get(5)?,
                    path: r.get::<_, Option<String>>(6)?.map(PathBuf::from),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

pub(super) fn apply(
    tx: &rusqlite::Transaction<'_>,
    job: i64,
    entry: i64,
    version: i64,
    conflict: Option<String>,
    action: PhotoAction,
    recovery: Option<String>,
) -> Result<()> {
    let invalid = |message: &str| AppError::Backend(message.into());
    let current: (i64,Option<String>,Option<String>,Option<String>) = tx.query_row("SELECT e.native_local_version,e.native_photo_id,e.native_master_id,f.native_detail FROM sync_entries e LEFT JOIN sync_conflicts f ON f.entry_id=e.id AND f.state='open' WHERE e.job_id=?1 AND e.id=?2",params![job,entry],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    if current.0 != version || current.3 != conflict {
        return Err(invalid(
            "photo changed while the decision was open; refresh and retry",
        ));
    }
    let busy: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_tasks WHERE entry_id=?1 AND state='issued') OR EXISTS(SELECT 1 FROM sync_publications WHERE entry_id=?1 AND state='publishing') OR EXISTS(SELECT 1 FROM sync_job_changes WHERE job_id=?2) OR EXISTS(SELECT 1 FROM settings WHERE key='sync_reset_pending')",params![entry,job],|r|r.get(0))?;
    if busy {
        return Err(invalid(
            "an operation is still being reconciled; synchronize before choosing a resolution",
        ));
    }
    let deletion_pending: bool = tx.query_row(
        "SELECT native_dirty=4 FROM sync_entries WHERE id=?1",
        [entry],
        |r| r.get(0),
    )?;
    if deletion_pending && !matches!(action, PhotoAction::DeleteCloud) {
        return Err(invalid(
            "the permanent deletion must be confirmed by cloud synchronization first",
        ));
    }
    let stamp = serde_json::to_string(&EditEventStamp {
        edited_at_ms: chrono::Utc::now().timestamp_millis(),
        source: super::super::device::device_id_in(tx)?,
        event_id: uuid::Uuid::new_v4().to_string(),
    })
    .map_err(|e| invalid(&e.to_string()))?;
    match action {
        PhotoAction::DeleteCloud => {
            let has_local: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_entries e JOIN media_items m ON m.id=e.media_id WHERE e.id=?1)",
                [entry], |r|r.get(0),
            )?;
            if has_local {
                return Err(invalid(
                    "permanently remove the local file before synchronizing its deletion",
                ));
            }
            if current.1.is_none() || current.3.is_some() {
                return Err(invalid(
                    "cloud deletion requires a confirmed identity without a conflict",
                ));
            }
            tx.execute("UPDATE sync_entries SET native_dirty=4,native_local_version=native_local_version+1,native_deletion_version=native_deletion_version+1,native_deletion_event=?2 WHERE id=?1",params![entry,stamp])?;
        }
        PhotoAction::Recreate => {
            if current.1.is_some() || current.3.is_some() {
                return Err(invalid(
                    "recreation requires a retired identity without a conflict",
                ));
            }
            recreate(tx, entry, &stamp)?;
        }
        PhotoAction::KeepLocal | PhotoAction::UseCloud => {
            if current.3.is_none() {
                return Err(invalid("this conflict was already resolved"));
            }
            let detail: serde_json::Value = serde_json::from_str(current.3.as_deref().unwrap())
                .map_err(|_| invalid("invalid conflict details"))?;
            let photo: Option<super::super::RemotePhoto> = detail
                .get("remote")
                .filter(|p| !p.is_null())
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| invalid("invalid competing photo"))?;
            if let Some(photo) = photo {
                if photo.is_hidden {
                    return Err(invalid(
                        "hidden cloud photos require a supported local representation",
                    ));
                }
                let projection = super::receive::current_remote_photo_in(tx, job, &photo.photo_id)?;
                let encoded = serde_json::to_string(&photo)
                    .map_err(|_| invalid("invalid competing photo"))?;
                if projection.as_deref() != Some(&encoded) {
                    return Err(invalid("cloud projection changed; refresh the conflict"));
                }
                let asset = super::receive::record_revision_in(tx, job, &photo.photo_id)?
                    .ok_or_else(|| invalid("photo has no asset revision"))?;
                let master = super::receive::record_revision_in(tx, job, &photo.master_id)?
                    .ok_or_else(|| invalid("photo has no master revision"))?;
                tx.execute("UPDATE sync_entries SET native_photo_id=?2,native_master_id=?3,native_asset_revision=?4,native_master_revision=?5,native_content_generation=?6,native_retired=0,native_local_version=native_local_version+1 WHERE id=?1",params![entry,photo.photo_id,photo.master_id,asset,master,photo.content_generation])?;
                match action {
                    PhotoAction::KeepLocal => {
                        require_media(tx, entry)?;
                        let complete:i64=tx.query_row("SELECT COUNT(*) FROM sync_native_resources WHERE job_id=?1 AND ((asset_id=?2 AND role='original') OR (asset_id=?3 AND role='thumbnail')) AND state IN ('staged','verified','retained')",params![job,photo.original.asset_id,photo.thumbnail.asset_id],|r|r.get(0))?;
                        if complete != 2 || recovery.is_none() {
                            return Err(invalid("the competing cloud pair must be retained before keeping local content"));
                        }
                        tx.execute("UPDATE sync_native_resources SET state='retained' WHERE job_id=?1 AND ((asset_id=?2 AND role='original') OR (asset_id=?3 AND role='thumbnail')) AND state='staged'",params![job,photo.original.asset_id,photo.thumbnail.asset_id])?;

                        tx.execute("UPDATE sync_entries SET native_dirty=2,native_metadata_version=native_metadata_version+1,native_content_version=native_content_version+1,native_metadata_event=?2,native_content_event=?2,native_resource_baseline=?3 WHERE id=?1",params![entry,stamp,encoded])?;
                    }
                    PhotoAction::UseCloud => {
                        if recovery.is_none() {
                            return Err(invalid("local recovery evidence must be saved before accepting cloud content"));
                        }
                        tx.execute("UPDATE sync_entries SET native_dirty=0,native_resource_baseline=NULL WHERE id=?1",[entry])?;
                        tx.execute("DELETE FROM sync_publications WHERE entry_id=?1 AND state<>'publishing'",[entry])?;
                        tx.execute("UPDATE sync_native_resources SET state='pending',staging_path=NULL WHERE job_id=?1 AND ((asset_id=?2 AND role='original') OR (asset_id=?3 AND role='thumbnail'))",params![job,photo.original.asset_id,photo.thumbnail.asset_id])?;
                        // The identity is already bound by the branch above, so
                        // adoption cannot be reached here — hence no measured
                        // digests. The user chose this photo, not the bytes.
                        super::runtime::project_publication(
                            tx,
                            job,
                            &photo,
                            &std::collections::HashMap::new(),
                        )?;
                        // The user explicitly adopted this path after preserving its old file.
                        // The durable publication remains pending until both roles commit.
                        tx.execute(
                            "UPDATE sync_entries SET native_resource_baseline=?2 WHERE id=?1",
                            params![entry, encoded],
                        )?;
                    }
                    _ => unreachable!(),
                }
            } else if detail["reason"].as_str()
                == Some("remote photo was deleted while local changes remain")
            {
                retire(tx, entry)?;
                if matches!(action, PhotoAction::KeepLocal) {
                    recreate(tx, entry, &stamp)?;
                }
            } else {
                return Err(invalid(
                    "this conflict requires recovery before it can be resolved",
                ));
            }
            tx.execute("UPDATE sync_conflicts SET state='resolved',resolution=?2,resolved_at=unixepoch() WHERE entry_id=?1 AND state='open'",params![entry,serde_json::json!({"choice":action,"recovery":recovery}).to_string()])?;
        }
    }
    Ok(())
}
fn require_media(tx: &rusqlite::Transaction<'_>, entry: i64) -> Result<()> {
    let present:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_entries e JOIN media_items m ON m.id=e.media_id WHERE e.id=?1 AND m.trashed_at IS NULL)",[entry],|r|r.get(0))?;
    if !present {
        return Err(AppError::Backend(
            "restore the local photo before choosing to upload it".into(),
        ));
    }
    Ok(())
}
fn recreate(tx: &rusqlite::Transaction<'_>, entry: i64, stamp: &str) -> Result<()> {
    require_media(tx, entry)?;
    tx.execute("UPDATE sync_entries SET native_retired=0,native_dirty=1,native_local_version=native_local_version+1,native_metadata_version=native_metadata_version+1,native_content_version=native_content_version+1,native_metadata_event=?2,native_content_event=?2 WHERE id=?1",params![entry,stamp])?;
    Ok(())
}
pub(super) fn retire(tx: &rusqlite::Transaction<'_>, entry: i64) -> Result<()> {
    tx.execute("UPDATE sync_entries SET native_retired=1,native_dirty=0,native_photo_id=NULL,native_master_id=NULL,native_asset_revision=NULL,native_master_revision=NULL,native_content_generation=NULL,native_resource_baseline=NULL,native_baseline_metadata=NULL,native_confirmed_generation=NULL WHERE id=?1",[entry])?;
    Ok(())
}
