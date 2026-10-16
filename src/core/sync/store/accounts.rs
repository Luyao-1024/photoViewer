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

fn identity(id: i64, document: &Option<String>) -> Result<String> {
    let scope: Option<NativePinnedScope> = document
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(configuration::json_error)?;
    serde_json::to_string(
        &scope
            .map(|scope| {
                (
                    scope.public_base_url,
                    scope.namespace.tenant,
                    scope
                        .authentication
                        .and_then(|a| a.user)
                        .map(|user| user.id),
                )
            })
            .ok_or(id),
    )
    .map_err(configuration::json_error)
}

pub(crate) fn select_in(tx: &rusqlite::Transaction<'_>, job: Option<i64>) -> Result<()> {
    let connections = {
        let mut stmt = tx.prepare("SELECT c.id,c.native_pinned_scope,c.enabled FROM sync_connections c WHERE EXISTS(SELECT 1 FROM sync_jobs j WHERE j.connection_id=c.id) ORDER BY c.enabled DESC,c.id")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<String>>(1)?,
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
            Some(identity(id, document)?)
        }
        None => match previous.clone() {
            Some(value) => Some(value),
            None => connections
                .first()
                .map(|(id, document, _)| identity(*id, document))
                .transpose()?,
        },
    };
    for (id, document, enabled) in &connections {
        let current = identity(*id, document)?;
        let selected_here = selected.as_ref() == Some(&current);
        if selected_here != *enabled {
            tx.execute(
                "UPDATE sync_connections SET enabled=?2 WHERE id=?1",
                params![id, selected_here],
            )?;
        }
    }
    if let Some(selected) = selected.filter(|value| Some(value) != previous.as_ref()) {
        tx.execute("INSERT INTO settings(key,value) VALUES('native_sync_account',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [selected])?;
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
mod tests {
    use super::*;
    #[test]
    fn single_account_selection_is_persistent_and_keeps_saved_history() {
        let root = tempfile::tempdir().unwrap();
        let photos = root.path().join("photos");
        let archive = root.path().join("archive");
        std::fs::create_dir(&photos).unwrap();
        std::fs::create_dir(&archive).unwrap();
        let pool = crate::core::db::init_pool(&root.path().join("library.db")).unwrap();
        let store = SyncStore::new(pool.clone());
        let a = store
            .create_job(&super::super::tests::new_job(photos.clone()))
            .unwrap();
        let mut other = super::super::tests::new_job(photos);
        other.credential_ref = "second-account".into();
        other.native_pinned.as_mut().unwrap().namespace.tenant = "other-account".into();
        let b = store.create_job(&other).unwrap();
        assert_eq!(store.list_jobs().unwrap(), vec![a.clone()]);
        let mut same = super::super::tests::new_job(archive);
        same.credential_ref = "same-account-archive".into();
        same.native_zone = Some(super::super::super::native::NativeZone::new("ArchiveSync"));
        same.native_pinned = Some(super::super::super::native::demo_pinned_scope(
            "https://sync.example.invalid/",
            &["PrimarySync", "ArchiveSync"],
        ));
        let c = store.create_job(&same).unwrap();
        assert_eq!(store.list_jobs().unwrap().len(), 2);
        let epoch = runtime::Epoch {
            job: a.id,
            config: a.config_generation,
            cycle: "old-account-cycle".into(),
        };
        store
            .runtime(runtime::Command::Begin(epoch.clone()))
            .unwrap();
        assert!(store.epoch_current(&epoch).unwrap());
        store
            .runtime(runtime::Command::SelectAccount { job: Some(b.id) })
            .unwrap();
        assert!(!store.epoch_current(&epoch).unwrap());
        assert_eq!(store.saved_jobs().unwrap().len(), 3);
        assert_eq!(store.list_jobs().unwrap(), vec![b.clone()]);
        let reopened = SyncStore::new(pool);
        reopened.normalize_accounts().unwrap();
        assert!(reopened.job_is_selected(b.id).unwrap());
        // Removing the current account never silently starts an older one.
        reopened.set_job_paused(b.id, true).unwrap();
        reopened.delete_job(b.id).unwrap();
        reopened.normalize_accounts().unwrap();
        assert!(reopened.list_jobs().unwrap().is_empty());
        reopened
            .runtime(runtime::Command::SelectAccount { job: Some(a.id) })
            .unwrap();
        assert!(reopened.job_is_selected(a.id).unwrap());
        assert!(reopened.job_is_selected(c.id).unwrap());
        assert_eq!(
            reopened.get_job(a.id).unwrap().unwrap().config_generation,
            a.config_generation
        );
    }
}
