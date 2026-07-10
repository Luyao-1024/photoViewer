//! 后台扫描 worker：扫描 root_paths 后 upsert 到 DB
use crate::core::backend::local::LocalBackend;
use crate::core::db::DbPool;
use crate::core::db_actor::{DbActorHandle, DbCommand, DbCommandResult};
use crate::core::events::ChangeSource;
use std::path::PathBuf;
use tokio::task::JoinHandle;

pub fn spawn_scan(pool: DbPool, db_actor: DbActorHandle, paths: Vec<PathBuf>) -> JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        let backend = LocalBackend::new(pool);
        for path in &paths {
            tracing::info!("开始扫描: {}", path.display());
            match backend.scan_dir(path) {
                Ok(items) => {
                    let total = items.len();
                    let upserted = match db_actor.execute_blocking(DbCommand::UpsertMediaBatch {
                        source: ChangeSource::StartupScan,
                        items,
                    }) {
                        Ok(DbCommandResult::MediaItems(items)) => items.len(),
                        Ok(other) => {
                            tracing::warn!("scan upsert returned unexpected result: {other:?}");
                            0
                        }
                        Err(e) => {
                            tracing::warn!("scan upsert 失败 {}: {}", path.display(), e);
                            0
                        }
                    };
                    tracing::info!("扫描完成: {} 张图片（{} 新增/更新）", total, upserted);
                }
                Err(e) => tracing::error!("扫描失败 {}: {}", path.display(), e),
            }
        }
    })
}
