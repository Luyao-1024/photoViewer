//! 本地文件系统扫描后端
use crate::core::db::{self, DbPool};
use crate::core::error::{AppError, Result};
use crate::core::media::{
    is_supported_media_path, media_kind_from_mime, mime_from_extension, MediaItem, MediaKind,
    NewMediaItem, MEDIA_SUBKIND_MOTION_PHOTO, MEDIA_SUBKIND_STANDARD,
};
use crate::core::metadata;
use crate::core::motion_photo::{self, MediaAttributes};
use chrono::Utc;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError};
use std::time::{Duration, Instant};
use walkdir::WalkDir;

// 诊断：扫描期间累计各阶段耗时，SCAN_SUMMARY 打印后归零。AtomicU64 让生产者线程
// （extract/motion）与消费者线程（upsert）的耗时能汇总到同一计数器。
static SCAN_EXTRACT_MS: AtomicU64 = AtomicU64::new(0);
static SCAN_HASH_MS: AtomicU64 = AtomicU64::new(0);
static SCAN_MOTION_MS: AtomicU64 = AtomicU64::new(0);
static SCAN_UPSORT_MS: AtomicU64 = AtomicU64::new(0);

/// 消费者（主线程）攒一批再提交的间隔。生产者把 extract 出的项经有界 channel 送到
/// 消费者；消费者每到这里就把累计的一批合进**一个事务**提交（`upsert_media_items_batch`），
/// 再转发给 UI。约 2s 一批既让 UI 看到渐进进度，又把每行 autocommit 的 fsync 摊薄成
/// 每批一次——这是十万级冷扫描 DB 写入从数十秒降到秒级的关键。
const UPSERT_FLUSH_INTERVAL: Duration = Duration::from_secs(2);

/// 生产者 → 消费者 channel 容量：限制飞行中的 `NewMediaItem` 数量，让生产者在消费者
/// 落库跟不上时自然背压，内存峰值与图库规模无关。
const SCAN_ITEM_CHANNEL_CAP: usize = 1024;

/// 生产者对一个文件的处理结果：成功提取（待入库）、跳过（不支持 MIME）、或出错
/// （提取失败/解析 panic）。消费者据此聚合诊断计数并批量入库。
// Box the NewMediaItem variant:它约 256 B，而另两个变体无数据，不装箱会把每个
// `WorkOutcome`（及 channel 槽）撑到 256 B。
enum WorkOutcome {
    Item(Box<NewMediaItem>),
    NoneMime,
    Error,
}

fn stream_file_hash(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0_u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// 文件的索引时间信号：created 优先，失败回退 modified。
///
/// `process_file` 存入 DB 的 `file_mtime` 与 `scan_and_upsert_dir` 的跳过
/// 判断**必须**用同一套逻辑，否则会出现「存的与比的口径不一致」导致永不命中。
fn file_index_time(meta: &std::fs::Metadata) -> Option<std::time::SystemTime> {
    meta.created().or_else(|_| meta.modified()).ok()
}

pub struct LocalBackend {
    pool: DbPool,
}

impl LocalBackend {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// 返回内部连接池的引用，供 `notify_watcher` 在事件处理中调用
    /// `albums::refresh(&pool)` 同步刷新物化视图。
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// 递归扫描目录，返回所有支持的媒体项（**不做跳过**，逐个全量提取 + 全文件
    /// 哈希）。供需要完整 `NewMediaItem` 列表的场景（测试、相册预处理）使用。
    /// 启动扫描请用 [`Self::scan_and_upsert_dir`]，它会跳过未改动文件以避免
    /// 重复哈希。
    pub fn scan_dir(&self, root: &Path) -> Result<Vec<NewMediaItem>> {
        let mut items = Vec::new();
        for entry in WalkDir::new(root).follow_links(false).into_iter().flatten() {
            let path = entry.path();
            // 一次 stat 既判 is_file 又喂给 process_file（复用，不再在 process_file 内重复 stat）。
            let file_meta = match std::fs::metadata(path) {
                Ok(m) if m.is_file() => m,
                _ => continue,
            };
            if !is_supported_media_path(path) {
                continue;
            }
            match self.process_file(path, &file_meta) {
                Ok(Some(item)) => items.push(item),
                Ok(None) => {} // 不支持的 MIME
                Err(e) => tracing::warn!("跳过文件 {}: {}", path.display(), e),
            }
        }
        Ok(items)
    }

    /// 启动扫描入口：遍历 `root`，对每个媒体文件 upsert，但**先用 `(uri, file_mtime,
    /// file_size)` 查库短路**——未改动的文件直接跳过，不读全文件做 blake3、不重提
    /// EXIF。返回实际（重新）索引的文件数。
    ///
    /// 这把「每次启动对整个图库重复哈希」（1.8GB → 数秒）降到「逐文件 stat + 一次
    /// 索引查询」（毫秒级），除非文件真的新增/改动。
    pub fn scan_and_upsert_dir(&self, root: &Path) -> Result<usize> {
        self.scan_and_upsert_dir_with(root, |_| {})
    }

    /// 与 [`Self::scan_and_upsert_dir`] 相同，但每个实际 upsert 的项目都会传给
    /// `on_upserted`。用于启动后台扫描把新增/变更项增量推给 UI，同时仍保留
    /// `(uri, file_mtime, file_size)` 未改动短路。
    pub fn scan_and_upsert_dir_notify<F>(&self, root: &Path, on_upserted: F) -> Result<usize>
    where
        F: FnMut(MediaItem),
    {
        self.scan_and_upsert_dir_with(root, on_upserted)
    }

    fn scan_and_upsert_dir_with<F>(&self, root: &Path, mut on_upserted: F) -> Result<usize>
    where
        F: FnMut(MediaItem),
    {
        // 诊断计数器：定位「为什么扫不全」。SCAN_SUMMARY 会在每个 root 扫完时打印；
        // 若该日志缺失，说明扫描中途被中止（panic / 进程退出 / spawn_blocking join 失败）。
        // 归零放在最前面，避免上一轮（或 scan_dir 测试路径）残留污染本次汇总。
        let _ = SCAN_EXTRACT_MS.swap(0, Ordering::Relaxed);
        let _ = SCAN_HASH_MS.swap(0, Ordering::Relaxed);
        let _ = SCAN_MOTION_MS.swap(0, Ordering::Relaxed);
        let _ = SCAN_UPSORT_MS.swap(0, Ordering::Relaxed);
        let started = Instant::now();
        let mut errors = 0u64; // 元数据/upsert 失败，或解析 panic
        let mut none_mime = 0u64; // process_file 返回 None（不支持 MIME）
        let mut indexed = 0usize; // 实际写入 DB 的新增/更新行

        // ── 一次性载入未改动快照（主线程，起生产者之前） ─────────────────────
        // 把「已索引且非回收站」行的 (uri → (mtime, size)) 全量读进 HashMap。扫描线程据此
        // 在内存里做未改动短路——逐文件零 DB 往返，也不再与消费者的写事务争 WAL（此前十
        // 万级图库 ~20s 的读写竞争主要来源就是这条逐行 SELECT 并发了批量写）。快照仅本
        // 轮扫描用，生产者退出即弃。
        let snap_t = Instant::now();
        let unchanged_index = db::load_unchanged_index(&self.pool)?;
        let snap_ms = snap_t.elapsed().as_millis() as u64;

        // ── 生产者线程：walk + (uri,mtime,size) 内存短路 + extract ──────────────
        // 生产者**完全不碰数据库**：未改动短路查主线程预载的 HashMap（纯内存比较），真正
        // 要重新索引的文件 extract 成 NewMediaItem 后经有界 channel 交给消费者。它独立线程
        // 跑，与消费者的批量入库形成流水——提取与落库并行，互不阻塞。
        let root_owned = root.to_path_buf();
        let (item_tx, item_rx) = sync_channel::<WorkOutcome>(SCAN_ITEM_CHANNEL_CAP);
        let producer_pool = self.pool.clone();
        let producer = std::thread::spawn(move || -> Result<(u64, u64, u64)> {
            let backend = LocalBackend::new(producer_pool);
            let mut visited = 0u64;
            let mut supported = 0u64;
            let mut unchanged = 0u64;
            for entry in WalkDir::new(&root_owned)
                .follow_links(false)
                .into_iter()
                .flatten()
            {
                visited += 1;
                let path = entry.path();
                // 一次 stat 既判 is_file 又供未改动短路与 process_file 复用：此前每个文件
                // stat 达 3 次（is_file + 这里 + process_file 内），合并为 1 次。
                let file_meta = match std::fs::metadata(path) {
                    Ok(m) if m.is_file() => m,
                    Ok(_) => continue, // 目录等非普通文件
                    Err(e) => {
                        tracing::warn!("跳过文件 {}: {}", path.display(), e);
                        if item_tx.send(WorkOutcome::Error).is_err() {
                            break;
                        }
                        continue;
                    }
                };
                if !is_supported_media_path(path) {
                    continue;
                }
                supported += 1;
                // 廉价的改动检测：uri + mtime(秒) + size 全部一致即视为未改动。查主线程
                // 预载的快照，纯内存比较——扫描线程完全不碰数据库，也不与消费者的写事
                // 务争 WAL。
                let uri = format!("file://{}", path.display());
                if let Some(mtime) = file_index_time(&file_meta).and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs() as i64)
                }) {
                    if unchanged_index.get(uri.as_str()) == Some(&(mtime, file_meta.len() as i64)) {
                        unchanged += 1;
                        continue;
                    }
                }
                // 单文件损坏用 catch_unwind 隔离，不让一张坏图废掉整轮扫描。
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    backend.process_file(path, &file_meta)
                }));
                let work = match outcome {
                    Ok(Ok(Some(item))) => WorkOutcome::Item(Box::new(item)),
                    Ok(Ok(None)) => WorkOutcome::NoneMime,
                    Ok(Err(e)) => {
                        tracing::warn!("跳过文件 {}: {}", path.display(), e);
                        WorkOutcome::Error
                    }
                    Err(panic_payload) => {
                        let msg = panic_payload
                            .downcast_ref::<String>()
                            .map(|s| s.as_str())
                            .or_else(|| panic_payload.downcast_ref::<&'static str>().copied())
                            .unwrap_or("(non-string panic)");
                        tracing::error!(
                            "扫描文件 {} 时 panic，已跳过以继续扫描其余文件: {}",
                            path.display(),
                            msg
                        );
                        WorkOutcome::Error
                    }
                };
                // 消费者已关闭（应用关闭/中止）：停止再投递。
                if item_tx.send(work).is_err() {
                    break;
                }
            }
            Ok((visited, supported, unchanged))
        });
        // 生产者独占 item_tx：它退出（走完 walk 或因消费者关闭而 send 失败）时 sender
        // 析构，item_rx 随即收到 Disconnected，下面的 drain 循环据此结束。
        // 消费者若提前返回，item_rx 析构 → 生产者下次 send 失败 → 自行退出，无泄漏。

        // ── 消费者（本线程）：批量入库 + 推 UI ──────────────────────────────
        // 每 UPSERT_FLUSH_INTERVAL（约 2s）把攒下的一批合进一个事务提交
        // （`db::upsert_media_items_batch`），把每行 autocommit 的 fsync 摊薄成每批一次；
        // 提交后再把物化行经 on_upserted 推给 notifier（notifier 侧仍按自适应间隔刷新
        // UI）。全库仅此一个写者，无 SQLite 写竞争。生产者写、本线程读/写分离，互不阻塞。
        let mut pending: Vec<NewMediaItem> = Vec::new();
        let mut last_flush = Instant::now();
        loop {
            let timeout = UPSERT_FLUSH_INTERVAL.saturating_sub(last_flush.elapsed());
            match item_rx.recv_timeout(timeout) {
                Ok(WorkOutcome::Item(item)) => pending.push(*item),
                Ok(WorkOutcome::NoneMime) => none_mime += 1,
                Ok(WorkOutcome::Error) => errors += 1,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if last_flush.elapsed() >= UPSERT_FLUSH_INTERVAL && !pending.is_empty() {
                let t = Instant::now();
                match db::upsert_media_items_batch(&self.pool, &pending) {
                    Ok(upserted) => {
                        SCAN_UPSORT_MS
                            .fetch_add(t.elapsed().as_millis() as u64, Ordering::Relaxed);
                        for m in upserted {
                            on_upserted(m);
                            indexed += 1;
                        }
                    }
                    Err(e) => {
                        tracing::warn!("批量 upsert 失败（{} 项）: {}", pending.len(), e);
                        errors += pending.len() as u64;
                    }
                }
                pending.clear();
                last_flush = Instant::now();
            }
        }
        // 生产者已退出：把最后一批落库。
        if !pending.is_empty() {
            let t = Instant::now();
            match db::upsert_media_items_batch(&self.pool, &pending) {
                Ok(upserted) => {
                    SCAN_UPSORT_MS
                        .fetch_add(t.elapsed().as_millis() as u64, Ordering::Relaxed);
                    for m in upserted {
                        on_upserted(m);
                        indexed += 1;
                    }
                }
                Err(e) => {
                    tracing::warn!("批量 upsert 失败（{} 项）: {}", pending.len(), e);
                    errors += pending.len() as u64;
                }
            }
        }

        let producer_result = producer
            .join()
            .map_err(|_| AppError::Backend("scan producer thread panicked".into()))?;
        let (visited, supported, unchanged) = producer_result?;

        let extract_ms = SCAN_EXTRACT_MS.swap(0, Ordering::Relaxed);
        let hash_ms = SCAN_HASH_MS.swap(0, Ordering::Relaxed);
        let motion_ms = SCAN_MOTION_MS.swap(0, Ordering::Relaxed);
        let upsert_ms = SCAN_UPSORT_MS.swap(0, Ordering::Relaxed);
        tracing::debug!(
            target: crate::core::log_targets::STORAGE,
            "SCAN_SUMMARY root={} visited={} supported={} unchanged={} errors={} none_mime={} indexed={} elapsed_ms={} | phases_ms snapshot={} extract={} hash={} motion={} upsert={}",
            root.display(),
            visited,
            supported,
            unchanged,
            errors,
            none_mime,
            indexed,
            started.elapsed().as_millis(),
            snap_ms,
            extract_ms,
            hash_ms,
            motion_ms,
            upsert_ms,
        );
        Ok(indexed)
    }

    /// `file_meta` 由调用方提供（扫描热路径已为未改动短路 stat 过一次），避免在这里
    /// 重复 stat——此前生产者对每个文件 stat 多达 3 次（`is_file` + 短路 metadata + 这里），
    /// 合并后全程只 stat 1 次。
    fn process_file(&self, path: &Path, file_meta: &std::fs::Metadata) -> Result<Option<NewMediaItem>> {
        let t_extract = Instant::now();

        // 按 MIME 路由：标准图片只读一次 256KB 头部，由 extract（dims+EXIF）与动图
        // 检测共享——这样一张 JPEG 只被打开一次，而不是分别给 image_dimensions /
        // read_exif / motion detect 各开一次。HEIC 需要整文件（其 Exif item 可能在
        // 任意位置）且不是动图候选，不读共享头部；视频直接走 ffprobe。
        let mime = mime_from_extension(path).map(str::to_string);
        let head: Option<Vec<u8>> = match mime.as_deref() {
            Some(m) if media_kind_from_mime(m) == Some(MediaKind::Image) && m != "image/heic" => {
                metadata::read_image_head(path).ok()
            }
            _ => None,
        };

        let meta = metadata::extract_with_head(path, head.as_deref())?;
        SCAN_EXTRACT_MS.fetch_add(t_extract.elapsed().as_millis() as u64, Ordering::Relaxed);

        let file_time = file_index_time(file_meta).unwrap_or_else(std::time::SystemTime::now);
        let file_time_utc: chrono::DateTime<Utc> = file_time.into();

        let uri = format!("file://{}", path.display());
        let folder = path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| Path::new("/").to_path_buf());

        // Content hash is intentionally NOT computed at scan time: nothing ever
        // matches rows by blake3_hash (the unchanged-check keys on uri+mtime+size,
        // and moves/copies work via known item ids), so a full-file blake3 was
        // ~7s of pure waste on the scan hot path for ~3200 files. The column is
        // left empty; the only former reader (motion_video_cache_path) now keys
        // on the item id. Re-enable here if content-based dedup becomes a feature.
        let hash = String::new();

        let t_motion = Instant::now();
        // 有共享头部时复用它做动图检测；否则（HEIC/视频）回退到 detect(path)，但二者
        // 都非 JPEG，is_motion_candidate_mime 立即返回 None，不会触发额外读盘。
        let motion_photo = match head.as_deref() {
            Some(h) => motion_photo::detect_with_head(path, h, file_meta.len()),
            None => motion_photo::detect(path),
        };
        SCAN_MOTION_MS.fetch_add(t_motion.elapsed().as_millis() as u64, Ordering::Relaxed);

        Ok(Some(NewMediaItem {
            uri,
            path: path.to_path_buf(),
            folder_path: folder,
            mime_type: meta.mime_type,
            media_subkind: if motion_photo.is_some() {
                MEDIA_SUBKIND_MOTION_PHOTO.into()
            } else {
                MEDIA_SUBKIND_STANDARD.into()
            },
            media_attributes: motion_photo
                .map(MediaAttributes::motion_photo_json)
                .unwrap_or_else(MediaAttributes::standard_json),
            width: meta.width,
            height: meta.height,
            video_duration_secs: meta.video.and_then(|v| v.duration_secs),
            taken_at: meta.taken_at,
            file_mtime: file_time_utc,
            file_size: file_meta.len(),
            blake3_hash: hash,
        }))
    }

    /// 与 [`Self::process_file`] 同样的元数据/哈希提取，但把结果的 `uri` / `path` /
    /// `folder_path` 覆盖成 `uri` / `path` / `folder`，而非 `source` 自身的路径。
    ///
    /// 供回收站对账使用：被外部删除的图片物理上在 `Trash/files/<name>`，但其 DB 行
    /// 必须记原始位置（缩略图解析、还原都靠原始 `uri` 找 `.trashinfo`），所以从
    /// 回收站副本读元数据、写到原始路径下。
    pub fn process_file_at(
        &self,
        source: &Path,
        uri: &str,
        path: &Path,
        folder: &Path,
    ) -> Result<NewMediaItem> {
        let meta = metadata::extract(source)?;
        let file_meta = std::fs::metadata(source)?;
        let file_time = file_index_time(&file_meta).unwrap_or_else(std::time::SystemTime::now);
        let file_time_utc: chrono::DateTime<Utc> = file_time.into();
        let hash = stream_file_hash(source)?;
        let motion_photo = motion_photo::detect(source);
        Ok(NewMediaItem {
            uri: uri.to_string(),
            path: path.to_path_buf(),
            folder_path: folder.to_path_buf(),
            mime_type: meta.mime_type,
            media_subkind: if motion_photo.is_some() {
                MEDIA_SUBKIND_MOTION_PHOTO.into()
            } else {
                MEDIA_SUBKIND_STANDARD.into()
            },
            media_attributes: motion_photo
                .map(MediaAttributes::motion_photo_json)
                .unwrap_or_else(MediaAttributes::standard_json),
            width: meta.width,
            height: meta.height,
            video_duration_secs: meta.video.and_then(|v| v.duration_secs),
            taken_at: meta.taken_at,
            file_mtime: file_time_utc,
            file_size: file_meta.len(),
            blake3_hash: hash,
        })
    }

    /// 从单个文件路径提取元数据并 upsert 到数据库。
    ///
    /// 专为 `notify_watcher` 等增量入口设计：
    ///   - 路径不是文件（目录事件、临时消失等）时返回 `Ok(None)`；
    ///   - 解析失败时返回错误，调用方负责记录日志；
    ///   - upsert 成功时返回 `Ok(Some(MediaItem))`，调用方可以直接转发给
    ///     `MediaChangeNotifier` 而无需再次查询 DB。
    pub fn upsert_from_path(&self, path: &Path) -> Result<Option<MediaItem>> {
        // 一次 stat 既判 is_file 又喂给 process_file，避免重复 stat。
        let file_meta = match std::fs::metadata(path) {
            Ok(m) if m.is_file() => m,
            _ => return Ok(None),
        };
        let item = self.process_file(path, &file_meta)?.ok_or_else(|| {
            AppError::Decode(format!("not a supported media: {}", path.display()))
        })?;
        self.upsert(&item).map(Some)
    }

    /// 删除指定路径对应的索引行，供文件监听的 remove/rename 事件使用。
    pub fn delete_path(&self, path: &Path) -> Result<usize> {
        db::delete_media_by_path(&self.pool, path)
    }

    /// Insert or update (URI conflict → UPDATE). Returns the fully-materialized
    /// row so callers (notably `notify_watcher`) can forward it to the UI
    /// without a second DB round-trip.
    ///
    /// 更新既有行时一并清空 `trashed_at`：upsert 只在文件确实存在于（原）路径时
    /// 被调用——一个仍标记为回收站的行此刻文件却在原路径，只可能是被外部从系统
    /// 回收站还原了，应重新视为 live，否则还原后的图片不会回到相册、也不会从
    /// 回收站视图消失。
    pub fn upsert(&self, item: &NewMediaItem) -> Result<MediaItem> {
        let conn = self.pool.get()?;
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM media_items WHERE uri = ?1",
                [&item.uri],
                |row| row.get(0),
            )
            .ok();

        if let Some(id) = existing {
            conn.execute(
                "UPDATE media_items
                 SET path=?2, folder_path=?3, mime_type=?4, media_kind=?5,
                     media_subkind=?6, media_attributes=?7, width=?8, height=?9,
                     video_duration_secs=?10, taken_at=?11, file_mtime=?12,
                     file_size=?13, blake3_hash=?14,
                     trashed_at=NULL, indexed_at=unixepoch()
                 WHERE id=?1",
                rusqlite::params![
                    id,
                    item.path.to_string_lossy(),
                    item.folder_path.to_string_lossy(),
                    item.mime_type,
                    db::media_kind_db_value(&item.mime_type),
                    item.media_subkind,
                    item.media_attributes,
                    item.width,
                    item.height,
                    item.video_duration_secs,
                    item.taken_at.map(|t| t.timestamp()),
                    item.file_mtime.timestamp(),
                    item.file_size as i64,
                    item.blake3_hash,
                ],
            )?;
            drop(conn);
            Ok(db::get_media_item(&self.pool, id)?)
        } else {
            let id = db::insert_media_item(&self.pool, item)?;
            Ok(db::get_media_item(&self.pool, id)?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn stream_file_hash_matches_blake3_hash_for_file_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large-ish.bin");
        let mut file = std::fs::File::create(&path).unwrap();
        for i in 0..4096_u32 {
            file.write_all(&i.to_le_bytes()).unwrap();
        }
        drop(file);

        let bytes = std::fs::read(&path).unwrap();
        let expected = blake3::hash(&bytes).to_hex().to_string();

        assert_eq!(stream_file_hash(&path).unwrap(), expected);
    }

    #[test]
    fn upsert_returns_inserted_media_item_with_populated_id() {
        use crate::core::media::NewMediaItem;
        use chrono::Utc;

        let dir = tempfile::tempdir().unwrap();
        let path = write_plain_jpeg_in(dir.path(), "x.jpg");
        let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
        let backend = LocalBackend::new(pool.clone());

        let new_item = NewMediaItem {
            uri: format!("file://{}", path.display()),
            path: path.clone(),
            folder_path: dir.path().to_path_buf(),
            mime_type: "image/jpeg".into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: Some(64),
            height: Some(48),
            video_duration_secs: None,
            taken_at: None,
            file_mtime: Utc::now(),
            file_size: std::fs::metadata(&path).unwrap().len(),
            blake3_hash: "placeholder".into(),
        };

        let returned = backend.upsert(&new_item).expect("upsert should succeed");
        assert!(
            returned.id > 0,
            "returned MediaItem must have a populated id"
        );
        assert_eq!(returned.uri, new_item.uri);
        assert_eq!(returned.blake3_hash, "placeholder");
    }

    #[test]
    fn upsert_from_path_returns_inserted_media_item() {
        let dir = tempfile::tempdir().unwrap();
        let _path = write_plain_jpeg_in(dir.path(), "new.jpg");
        let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
        let backend = LocalBackend::new(pool.clone());

        let returned = backend
            .upsert_from_path(&dir.path().join("new.jpg"))
            .expect("upsert_from_path should succeed");
        let item = returned.expect("expected Some(MediaItem) for a valid jpeg");
        assert!(item.id > 0);
        assert!(item.path.ends_with("new.jpg"));
    }

    #[test]
    fn upsert_from_path_returns_none_for_directory_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
        let backend = LocalBackend::new(pool.clone());

        let returned = backend
            .upsert_from_path(&dir.path().join("subdir"))
            .expect("directory path should not error");
        assert!(
            returned.is_none(),
            "directory path must yield None, not Some"
        );
    }

    #[test]
    fn upsert_from_path_returns_updated_item_for_existing_uri() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_plain_jpeg_in(dir.path(), "dup.jpg");
        let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
        let backend = LocalBackend::new(pool.clone());

        let first = backend
            .upsert_from_path(&path)
            .unwrap()
            .expect("first upsert must yield Some");
        // Re-write the file with different (still-valid) image content so the
        // second upsert reflects it. blake3_hash is no longer computed at scan
        // time (always empty), so assert on the decoded dimensions, which change
        // 64x48 -> 32x32.
        write_distinct_jpeg_in(&path, 32, 32, [255, 0, 0]);
        let second = backend
            .upsert_from_path(&path)
            .unwrap()
            .expect("second upsert must yield Some");
        assert_eq!(first.id, second.id, "upsert must reuse the same id");
        assert_ne!(
            first.width, second.width,
            "second upsert must reflect new content (dimensions changed)"
        );
    }

    /// Test-only helper: write a 64x48 plain JPEG (mirrors
    /// `tests/common/mod.rs::write_plain_jpeg` without requiring that
    /// module to be in scope for the lib's own test binary).
    fn write_plain_jpeg_in(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        use image::{ImageBuffer, Rgb};
        let img = ImageBuffer::<Rgb<u8>, _>::from_fn(64, 48, |_, _| Rgb([128, 128, 128]));
        let path = dir.join(name);
        img.save(&path).unwrap();
        path
    }

    /// Test-only helper: overwrite an existing JPEG with a different-sized,
    /// different-colored image so its blake3 hash differs but EXIF decoding
    /// still succeeds.
    fn write_distinct_jpeg_in(path: &std::path::Path, w: u32, h: u32, color: [u8; 3]) {
        use image::{ImageBuffer, Rgb};
        let img = ImageBuffer::<Rgb<u8>, _>::from_fn(w, h, |_, _| Rgb(color));
        img.save(path).unwrap();
    }

    fn append_micro_video_tail(path: &std::path::Path, video_len: usize) {
        let bytes = std::fs::read(path).unwrap();
        let xmp = format!(
            r#"<x:xmpmeta><rdf:Description GCamera:MicroVideo="1" GCamera:MicroVideoOffset="{video_len}" GCamera:MicroVideoPresentationTimestampUs="123456"/></x:xmpmeta>"#
        );
        // 把 XMP 包进标准 APP1 段插到 SOI 之后（与真实 Google MicroVideo 一致），而不是
        // 把裸 XMP 追加到 JPEG 末尾——后者不是合法的 JPEG 段结构，detect 的段定位会漏掉。
        const XMP_APP1_SIG: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
        let payload_len = XMP_APP1_SIG.len() + xmp.len();
        let seg_len = u16::try_from(payload_len + 2).unwrap();
        let mut seg = Vec::with_capacity(4 + payload_len);
        seg.extend_from_slice(&[0xFF, 0xE1]);
        seg.extend_from_slice(&seg_len.to_be_bytes());
        seg.extend_from_slice(XMP_APP1_SIG);
        seg.extend_from_slice(xmp.as_bytes());

        let mut out = Vec::with_capacity(bytes.len() + seg.len() + video_len);
        out.extend_from_slice(&bytes[..2]); // SOI
        out.extend_from_slice(&seg); // APP1 XMP（紧跟 SOI）
        out.extend_from_slice(&bytes[2..]); // 原 JPEG 余下部分（含 EOI）

        let mut video = vec![0_u8; video_len];
        video[0..4].copy_from_slice(&(24_u32.to_be_bytes()));
        video[4..8].copy_from_slice(b"ftyp");
        video[8..12].copy_from_slice(b"mp42");
        out.extend_from_slice(&video);
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn upsert_from_path_persists_motion_photo_attributes() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_plain_jpeg_in(dir.path(), "motion.jpg");
        append_micro_video_tail(&path, 96);
        let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
        let backend = LocalBackend::new(pool);

        let item = backend
            .upsert_from_path(&path)
            .unwrap()
            .expect("motion photo jpeg should be indexed");

        assert_eq!(item.media_subkind, MEDIA_SUBKIND_MOTION_PHOTO);
        let attrs = motion_photo::MediaAttributes::from_json(&item.media_attributes);
        let info = attrs
            .motion_photo
            .expect("motion photo attributes should be persisted");
        assert_eq!(info.video_length, 96);
        assert_eq!(info.presentation_timestamp_us, Some(123_456));
    }

    /// 文件监听器看到被外部还原的文件重新出现在原路径 → `upsert_from_path`。
    /// 此刻行仍是 trashed，upsert 必须清掉 `trashed_at`，否则图片既不回相册、
    /// 也赖在回收站视图里。
    #[test]
    fn upsert_clears_trashed_at_when_file_reappears() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_plain_jpeg_in(dir.path(), "restored.jpg");
        let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
        let backend = LocalBackend::new(pool.clone());

        let item = backend
            .upsert_from_path(&path)
            .unwrap()
            .expect("initial upsert must yield Some");
        crate::core::db::mark_trashed(&pool, item.id).unwrap();
        assert!(
            crate::core::db::get_media_item(&pool, item.id)
                .unwrap()
                .trashed_at
                .is_some(),
            "precondition: row must be trashed"
        );

        // 模拟文件被外部还原后监听器收到的 Create 事件
        let restored = backend
            .upsert_from_path(&path)
            .unwrap()
            .expect("re-upsert must yield Some");
        assert_eq!(restored.id, item.id);
        assert!(
            restored.trashed_at.is_none(),
            "upsert of a reappearing file must clear trashed_at (external restore)"
        );
        assert!(
            crate::core::db::list_trashed_media(&pool)
                .unwrap()
                .is_empty(),
            "restored item must no longer be in the trash list"
        );
    }

    /// 启动扫描路径：文件在应用关闭期间被外部还原。即便 mtime/size 与索引时
    /// 完全一致，未改动短路也不能对 trashed 行命中——否则扫描会跳过它，
    /// `trashed_at` 永远清不掉。
    #[test]
    fn scan_reindexes_restored_file_clearing_trashed_at() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_plain_jpeg_in(dir.path(), "scan-restored.jpg");
        let pool = crate::core::db::init_pool(&dir.path().join("t.db")).unwrap();
        let backend = LocalBackend::new(pool.clone());

        let item = backend
            .upsert_from_path(&path)
            .unwrap()
            .expect("initial upsert must yield Some");
        crate::core::db::mark_trashed(&pool, item.id).unwrap();

        // 文件仍在原路径（被外部还原），mtime/size 未变
        backend.scan_and_upsert_dir(dir.path()).unwrap();

        let after = crate::core::db::get_media_item(&pool, item.id).unwrap();
        assert!(
            after.trashed_at.is_none(),
            "startup scan must re-index a restored (present) file and clear trashed_at"
        );
    }
}
