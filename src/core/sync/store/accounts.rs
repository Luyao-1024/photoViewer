//! Connection enablement selects one account; saved jobs keep their own history.
use super::*;

pub(crate) fn same_account(left: &NativePinnedScope, right: &NativePinnedScope) -> bool {
    left.public_base_url == right.public_base_url
        && left.namespace.tenant == right.namespace.tenant
        && left
            .authentication
            .as_ref()
            .and_then(|a| a.user.as_ref())
            .map(|u| &u.id)
            == right
                .authentication
                .as_ref()
                .and_then(|a| a.user.as_ref())
                .map(|u| &u.id)
}

fn identity(id: i64, account: Option<i64>) -> String {
    account.unwrap_or(-id).to_string()
}

pub(crate) fn refresh_media_locations_in(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    tx.execute("DELETE FROM media_items WHERE storage_location=2 AND NOT EXISTS(SELECT 1 FROM sync_entries WHERE media_id=media_items.id)",[])?;
    tx.execute("UPDATE media_items SET storage_location=CASE WHEN EXISTS(SELECT 1 FROM sync_entries e JOIN sync_jobs j ON j.id=e.job_id JOIN sync_connections c ON c.id=j.connection_id WHERE e.media_id=media_items.id AND c.enabled=1 AND e.native_photo_id IS NOT NULL AND e.native_confirmed_generation IS NOT NULL AND e.native_retired=0) THEN 3 ELSE 1 END WHERE storage_location IN (1,3)",[])?;
    Ok(())
}

pub(crate) fn select_in(tx: &rusqlite::Transaction<'_>, job: Option<i64>) -> Result<()> {
    let connections = {
        let mut stmt = tx.prepare("SELECT c.id,c.native_account_id,c.enabled FROM sync_connections c WHERE EXISTS(SELECT 1 FROM sync_jobs j WHERE j.connection_id=c.id) ORDER BY c.enabled DESC,c.id")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, bool>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let previous: Option<String> = tx
        .query_row(
            "SELECT value FROM settings WHERE key='native_sync_account'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let selected = match job {
        Some(job) => {
            let id: i64 = tx.query_row(
                "SELECT connection_id FROM sync_jobs WHERE id=?1",
                [job],
                |r| r.get(0),
            )?;
            let (_, document, _) = connections
                .iter()
                .find(|(current, _, _)| *current == id)
                .ok_or_else(|| AppError::Backend("account task was removed".into()))?;
            Some(identity(id, *document))
        }
        None => match previous.clone() {
            Some(value) => Some(value),
            None => connections
                .first()
                .map(|(id, document, _)| identity(*id, *document)),
        },
    };
    let selection_changed = selected.as_ref() != previous.as_ref();
    let mut enabled_changed = false;
    for (id, document, enabled) in &connections {
        let current = identity(*id, *document);
        let selected_here = selected.as_ref() == Some(&current);
        if selected_here != *enabled {
            enabled_changed = true;
            tx.execute(
                "UPDATE sync_connections SET enabled=?2 WHERE id=?1",
                params![id, selected_here],
            )?;
        }
    }
    if let Some(selected) = selected.filter(|value| Some(value) != previous.as_ref()) {
        tx.execute("INSERT INTO settings(key,value) VALUES('native_sync_account',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [selected])?;
    }
    if enabled_changed || selection_changed {
        refresh_media_locations_in(tx)?;
    }
    Ok(())
}

impl SyncStore {
    /// Includes inactive accounts for the explicit account picker and cleanup.
    pub fn saved_jobs(&self) -> Result<Vec<SyncJob>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!("SELECT {JOB_COLUMNS} FROM sync_jobs j JOIN sync_connections c ON c.id=j.connection_id ORDER BY j.id"))?;
        let rows = stmt
            .query_map([], map_job)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
    pub fn job_is_selected(&self, id: i64) -> Result<bool> {
        let conn = self.pool.get()?;
        Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM sync_jobs j JOIN sync_connections c ON c.id=j.connection_id WHERE j.id=?1 AND c.enabled=1)", [id], |r| r.get(0))?)
    }
    pub(crate) fn normalize_accounts(&self) -> Result<()> {
        self.runtime(runtime::Command::SelectAccount { job: None })
    }
}

#[cfg(test)]
mod tests;
