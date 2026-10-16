//! Explicit local cleanup. No original files and no remote photos are deleted.
use super::*;
use crate::core::{
    db_actor::{DbActorHandle, DbCommand},
    prefs,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DataKind {
    Library,
    Accounts,
    All,
}
impl SyncService {
    pub async fn clear_local_data(&self, actor: DbActorHandle, kind: DataKind) -> Result<()> {
        let restore_enabled = kind == DataKind::Library && prefs::sync_enabled();
        prefs::set_sync_enabled(false).map_err(AppError::Backend)?;
        configuration::GLOBAL_RUN_EPOCH.fetch_add(1, Ordering::AcqRel);
        self.store.begin_reset()?;
        // Reuse the durable reset barrier and publication recovery. Cleanup must
        // not unlink credentials/files underneath an issued operation.
        self.perform_sync_reset().await?;
        let work = self.reset_work()?;
        let _gate = work.gate.lock().await;
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            if kind != DataKind::Library {
                delete_credentials(&pool, crate::platform::credentials::delete_api_key)?;
                actor.execute_blocking(DbCommand::ResetAccountData)?;
            }
            if kind != DataKind::Accounts {
                let report = crate::core::cache::enforce_size_limit_report(
                    &crate::config::cache_dir().join("thumbnails"),
                    0,
                )?;
                if report.failed_files > 0 {
                    return Err(AppError::Backend(format!(
                        "{} thumbnail files could not be deleted",
                        report.failed_files
                    )));
                }
                actor.execute_blocking(DbCommand::ResetLibraryDatabase)?;
            }
            if kind == DataKind::All {
                crate::core::diagnostics::clear_logs()
                    .map_err(|e| AppError::Backend(e.to_string()))?;
            }
            Ok::<_, AppError>(())
        })
        .await
        .map_err(|e| AppError::Backend(e.to_string()))??;
        if restore_enabled {
            prefs::set_sync_enabled(true).map_err(AppError::Backend)?;
        }
        Ok(())
    }
}
fn delete_credentials(
    pool: &crate::core::db::DbPool,
    delete: impl Fn(&str) -> Result<()>,
) -> Result<()> {
    let conn = pool.get()?;
    let mut query = conn.prepare("SELECT credential_ref FROM sync_connections")?;
    let references = query
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(query);
    drop(conn);
    for reference in references {
        delete(&reference)?;
    }
    Ok(())
}
/// Allocated SQLite pages, including indexes. The secret itself is in the
/// system keyring; only account/session references live in photos.db.
pub fn database_usage(pool: &crate::core::db::DbPool, accounts: bool) -> u64 {
    pool.get().ok().and_then(|conn| conn.query_row(
        "SELECT COALESCE(SUM(pgsize),0) FROM dbstat WHERE name IN (SELECT name FROM sqlite_schema WHERE (tbl_name LIKE 'sync_%' OR tbl_name='settings')=?1)",
        [accounts], |r| r.get::<_,u64>(0),
    ).ok()).unwrap_or(0)
}
#[cfg(test)]
mod tests;
