//! 启动时一次性引导：扫描 + 聚合 albums
use crate::core::albums;
use crate::core::backend::local::LocalBackend;
use crate::core::db::DbPool;
use crate::core::error::{AppError, Result};
use crate::core::events::ChangeSource;
use crate::core::media_change_notifier::MediaChangeNotifier;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const STARTUP_SCAN_FIRST_NOTIFY_TIMEOUT: Duration = Duration::from_secs(1);
const STARTUP_SCAN_FIRST_NOTIFY_COUNT: usize = 200;

/// Adaptive scan→UI refresh cadence, picked from the cumulative count of items
/// pushed so far. With the scan now fast (~16s for ~3k files), the old fixed
/// 10s interval meant the grid barely moved during a scan; but flushing too
/// eagerly on huge libraries thrashes the ListStore with big batches. So:
/// small libraries refresh often (visible progress), large ones less often.
const NOTIFY_INTERVAL_SMALL: Duration = Duration::from_secs(2); // < 5 000 items
const NOTIFY_INTERVAL_MEDIUM: Duration = Duration::from_secs(5); // 5 000–20 000
const NOTIFY_INTERVAL_LARGE: Duration = Duration::from_secs(10); // >= 20 000
const NOTIFY_INTERVAL_SMALL_UP_TO: usize = 5_000;
const NOTIFY_INTERVAL_MEDIUM_UP_TO: usize = 20_000;

fn notify_interval(scanned: usize) -> Duration {
    if scanned < NOTIFY_INTERVAL_SMALL_UP_TO {
        NOTIFY_INTERVAL_SMALL
    } else if scanned < NOTIFY_INTERVAL_MEDIUM_UP_TO {
        NOTIFY_INTERVAL_MEDIUM
    } else {
        NOTIFY_INTERVAL_LARGE
    }
}

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

/// 同步扫描所有 root_path 并刷新 albums；每个实际 upsert 的 live item 会通过
/// notifier 发送给 GTK 主线程，使启动扫描可以后台进行而不等扫描完成才显示首屏。
pub async fn scan_and_aggregate_with_notifier(
    pool: &DbPool,
    roots: &[PathBuf],
    notifier: MediaChangeNotifier,
) -> Result<()> {
    let pool = pool.clone();
    let roots = roots.to_vec();
    tokio::task::spawn_blocking(move || {
        scan_and_aggregate_with_notifier_blocking(pool, roots, notifier)
    })
    .await
    .map_err(|e| AppError::Backend(format!("scan_and_aggregate_with_notifier join error: {e}")))?
}

fn scan_and_aggregate_with_notifier_blocking(
    pool: DbPool,
    roots: Vec<PathBuf>,
    notifier: MediaChangeNotifier,
) -> Result<()> {
    let backend = LocalBackend::new(pool.clone());
    let mut total_scanned: usize = 0;
    for root in &roots {
        let mut batch = Vec::new();
        let mut last_notify = Instant::now();
        let first_notify_started = Instant::now();
        let mut sent_first_batch = false;
        let indexed = backend.scan_and_upsert_dir_notify(root, |item| {
            if item.trashed_at.is_none() {
                batch.push(item);
                total_scanned += 1;
                let should_flush_first = !sent_first_batch
                    && (batch.len() >= STARTUP_SCAN_FIRST_NOTIFY_COUNT
                        || first_notify_started.elapsed() >= STARTUP_SCAN_FIRST_NOTIFY_TIMEOUT);
                let should_flush_later = sent_first_batch
                    && last_notify.elapsed() >= notify_interval(total_scanned);
                if should_flush_first || should_flush_later {
                    let batch_len = batch.len();
                    tracing::info!(
                        target: crate::core::log_targets::BROWSING,
                        "STARTUP_SCAN_NOTIFY interval_flush root={} batch_len={} elapsed_ms={} first_batch={} total_scanned={} interval_secs={}",
                        root.display(),
                        batch_len,
                        last_notify.elapsed().as_millis(),
                        !sent_first_batch,
                        total_scanned,
                        notify_interval(total_scanned).as_secs()
                    );
                    notifier.upserted_batch(ChangeSource::StartupScan, std::mem::take(&mut batch));
                    sent_first_batch = true;
                    last_notify = Instant::now();
                }
            }
        })?;
        tracing::info!(
            target: crate::core::log_targets::BROWSING,
            "STARTUP_SCAN_NOTIFY root_finished root={} final_batch_len={} indexed={}",
            root.display(),
            batch.len(),
            indexed
        );
        notifier.upserted_batch(ChangeSource::StartupScan, std::mem::take(&mut batch));
        tracing::info!("扫描完成 {}: {} 张新增/更新", root.display(), indexed);
    }
    albums::refresh(&pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_interval_picks_tier_by_cumulative_count() {
        // < 5 000 → 2s
        assert_eq!(notify_interval(0), NOTIFY_INTERVAL_SMALL);
        assert_eq!(notify_interval(4_999), NOTIFY_INTERVAL_SMALL);
        // 5 000–19 999 → 5s
        assert_eq!(notify_interval(5_000), NOTIFY_INTERVAL_MEDIUM);
        assert_eq!(notify_interval(19_999), NOTIFY_INTERVAL_MEDIUM);
        // >= 20 000 → 10s
        assert_eq!(notify_interval(20_000), NOTIFY_INTERVAL_LARGE);
        assert_eq!(notify_interval(500_000), NOTIFY_INTERVAL_LARGE);
    }
}
