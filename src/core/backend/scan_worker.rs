//! 后台扫描 worker：扫描 root_paths 后 upsert 到 DB
use crate::core::backend::local::LocalBackend;
use crate::core::db::DbPool;
use std::path::PathBuf;
use tokio::task::JoinHandle;

pub fn spawn_scan(pool: DbPool, paths: Vec<PathBuf>) -> JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        let backend = LocalBackend::new(pool);
        for path in &paths {
            tracing::info!("开始扫描: {}", path.display());
            match backend.scan_dir(path) {
                Ok(items) => {
                    let total = items.len();
                    let mut upserted = 0;
                    for item in &items {
                        match backend.upsert(item) {
                            Ok(_) => upserted += 1,
                            Err(e) => tracing::warn!("upsert 失败 {}: {}", item.uri, e),
                        }
                    }
                    tracing::info!("扫描完成: {} 张图片（{} 新增/更新）", total, upserted);
                }
                Err(e) => tracing::error!("扫描失败 {}: {}", path.display(), e),
            }
        }
    })
}
