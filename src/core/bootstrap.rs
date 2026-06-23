//! 启动时一次性引导：扫描 + 聚合 albums
use crate::core::albums;
use crate::core::backend::local::LocalBackend;
use crate::core::db::DbPool;
use crate::core::error::{AppError, Result};
use std::path::PathBuf;

/// 同步扫描所有 root_path 并刷新 albums 物化视图。
/// 替代 app.rs 里直接调 spawn_scan + ignore 的写法。
pub async fn scan_and_aggregate(pool: &DbPool, roots: &[PathBuf]) -> Result<()> {
    let pool = pool.clone();
    let roots = roots.to_vec();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let backend = LocalBackend::new(pool.clone());
        for root in &roots {
            // 跳过 (uri, file_mtime, file_size) 未改动的文件，避免每次启动都对
            // 整个图库重新做全文件 blake3 哈希与 EXIF 提取。
            let indexed = backend.scan_and_upsert_dir(root)?;
            tracing::info!("扫描完成 {}: {} 张新增/更新", root.display(), indexed);
        }
        albums::refresh(&pool)
    })
    .await
    .map_err(|e| AppError::Backend(format!("scan_and_aggregate join error: {e}")))?
}
