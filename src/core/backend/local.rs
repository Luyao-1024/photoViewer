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
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
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

fn is_excluded_path(path: &Path, excluded_roots: &[PathBuf]) -> bool {
    excluded_roots
        .iter()
        .any(|excluded| path.starts_with(excluded))
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
        self.scan_dir_with_exclusions(root, &[])
    }

    pub fn scan_dir_with_exclusions(
        &self,
        root: &Path,
        excluded_roots: &[PathBuf],
    ) -> Result<Vec<NewMediaItem>> {
        let mut items = Vec::new();
        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| !is_excluded_path(entry.path(), excluded_roots))
            .flatten()
        {
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
        self.scan_and_upsert_dir_with(root, &[], |_| {})
    }

    pub fn scan_and_upsert_dir_with_exclusions(
        &self,
        root: &Path,
        excluded_roots: &[PathBuf],
    ) -> Result<usize> {
        self.scan_and_upsert_dir_with(root, excluded_roots, |_| {})
    }

    /// 与 [`Self::scan_and_upsert_dir`] 相同，但每个实际 upsert 的项目都会传给
    /// `on_upserted`。用于启动后台扫描把新增/变更项增量推给 UI，同时仍保留
    /// `(uri, file_mtime, file_size)` 未改动短路。
    pub fn scan_and_upsert_dir_notify<F>(&self, root: &Path, on_upserted: F) -> Result<usize>
    where
        F: FnMut(MediaItem),
    {
        self.scan_and_upsert_dir_with(root, &[], on_upserted)
    }

    pub fn scan_and_upsert_dir_notify_with_exclusions<F>(
        &self,
        root: &Path,
        excluded_roots: &[PathBuf],
        on_upserted: F,
    ) -> Result<usize>
    where
        F: FnMut(MediaItem),
    {
        self.scan_and_upsert_dir_with(root, excluded_roots, on_upserted)
    }

    pub fn scan_and_submit_dir_notify_with_exclusions<C, F>(
        &self,
        root: &Path,
        excluded_roots: &[PathBuf],
        commit_batch: C,
        on_upserted: F,
    ) -> Result<usize>
    where
        C: FnMut(Vec<NewMediaItem>) -> Result<Vec<MediaItem>>,
        F: FnMut(MediaItem),
    {
        self.scan_and_process_dir_with(root, excluded_roots, commit_batch, on_upserted)
    }

    pub fn prune_missing_live_media_under_roots(
        &self,
        roots: &[PathBuf],
        excluded_roots: &[PathBuf],
    ) -> Result<Vec<String>> {
        // 启动对账：清理「DB 仍为 live、但磁盘上已消失」的索引行。按存储列
        // `folder_path` 分桶后做两层批量，避免逐行 stat + 逐行事务：
        //   - 整目录消失 → 一条 `delete_live_media_by_folder` 删光该目录全部 live 行
        //     （「整个相册被删」从 N 次 stat + N 次事务收敛成 1 次 stat + 1 条 DELETE）。
        //   - 目录仍在、仅个别文件消失 → 收集缺失 id，按 `delete_media_by_ids` 分块批量删。
        // 保留 root 作用域、excluded 过滤与 `trashed_at IS NULL` 守卫。
        let rows = db::list_live_media_locations(&self.pool)?;
        let mut by_dir: HashMap<PathBuf, Vec<(i64, String, PathBuf)>> = HashMap::new();
        for (id, uri, path, folder) in rows {
            if !roots.iter().any(|root| path.starts_with(root)) {
                continue;
            }
            if is_excluded_path(&path, excluded_roots) {
                continue;
            }
            by_dir.entry(folder).or_default().push((id, uri, path));
        }

        let mut removed = Vec::new();
        for (dir, entries) in by_dir {
            if !dir.exists() {
                // 整目录消失：folder_path 精确匹配本桶，一条 DELETE 删光全部 live 行。
                db::delete_live_media_by_folder(&self.pool, &dir)?;
                removed.extend(entries.into_iter().map(|(_, uri, _)| uri));
                continue;
            }
            // 目录仍在：仅对真正缺失的文件按 id 分块批量删。
            let missing: Vec<i64> = entries
                .iter()
                .filter(|(_, _, path)| !path.exists())
                .map(|(id, _, _)| *id)
                .collect();
            if missing.is_empty() {
                continue;
            }
            let deleted = db::delete_media_by_ids(&self.pool, &missing)?;
            // `delete_media_by_ids` 带 `trashed_at IS NULL` 守卫；并发 trash 的极小窗口下
            // 实际删除数可能少于 missing，按 deleted 截断避免向 UI 多报已删 uri。
            let mut missing_entries = entries.into_iter().filter(|(_, _, path)| !path.exists());
            for _ in 0..deleted {
                if let Some((_, uri, _)) = missing_entries.next() {
                    removed.push(uri);
                }
            }
        }
        Ok(removed)
    }

    #[tracing::instrument(
        name = "scan:upsert_dir",
        skip(self, excluded_roots, on_upserted),
        fields(
            root = %root.display(),
            snapshot_ms,
            extract_ms,
            hash_ms,
            motion_ms,
            upsert_ms
        )
    )]
    fn scan_and_upsert_dir_with<F>(
        &self,
        root: &Path,
        excluded_roots: &[PathBuf],
        on_upserted: F,
    ) -> Result<usize>
    where
        F: FnMut(MediaItem),
    {
        self.scan_and_process_dir_with(
            root,
            excluded_roots,
            |items| db::upsert_media_items_batch(&self.pool, &items),
            on_upserted,
        )
    }

    #[tracing::instrument(
        name = "scan:process_dir",
        skip(self, excluded_roots, commit_batch, on_upserted),
        fields(
            root = %root.display(),
            snapshot_ms,
            extract_ms,
            hash_ms,
            motion_ms,
            upsert_ms
        )
    )]
    fn scan_and_process_dir_with<C, F>(
        &self,
        root: &Path,
        excluded_roots: &[PathBuf],
        mut commit_batch: C,
        mut on_upserted: F,
    ) -> Result<usize>
    where
        C: FnMut(Vec<NewMediaItem>) -> Result<Vec<MediaItem>>,
        F: FnMut(MediaItem),
    {
        // 诊断计数器：定位「为什么扫不全」。SCAN_SUMMARY 会在每个 root 扫完时打印；
        // 若该日志缺失，说明扫描中途被中止（panic / 进程退出 / spawn_blocking join 失败）。
        // 归零放在最前面，避免上一轮（或 scan_dir 测试路径）残留污染本次汇总。
        let _ = SCAN_EXTRACT_MS.swap(0, Ordering::Relaxed);
        let _ = SCAN_HASH_MS.swap(0, Ordering::Relaxed);
        let _ = SCAN_MOTION_MS.swap(0, Ordering::Relaxed);
        let _ = SCAN_UPSORT_MS.swap(0, Ordering::Relaxed);
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
        let excluded_roots = excluded_roots.to_vec();
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
                .filter_entry(|entry| !is_excluded_path(entry.path(), &excluded_roots))
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
                let batch_len = pending.len();
                let batch = std::mem::take(&mut pending);
                match commit_batch(batch) {
                    Ok(upserted) => {
                        SCAN_UPSORT_MS.fetch_add(t.elapsed().as_millis() as u64, Ordering::Relaxed);
                        for m in upserted {
                            on_upserted(m);
                            indexed += 1;
                        }
                    }
                    Err(e) => {
                        tracing::warn!("批量 upsert 失败（{} 项）: {}", batch_len, e);
                        errors += batch_len as u64;
                    }
                }
                last_flush = Instant::now();
            }
        }
        // 生产者已退出：把最后一批落库。
        if !pending.is_empty() {
            let t = Instant::now();
            let batch_len = pending.len();
            let batch = std::mem::take(&mut pending);
            match commit_batch(batch) {
                Ok(upserted) => {
                    SCAN_UPSORT_MS.fetch_add(t.elapsed().as_millis() as u64, Ordering::Relaxed);
                    for m in upserted {
                        on_upserted(m);
                        indexed += 1;
                    }
                }
                Err(e) => {
                    tracing::warn!("批量 upsert 失败（{} 项）: {}", batch_len, e);
                    errors += batch_len as u64;
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
        // Per-phase totals are recorded as fields on the `scan:upsert_dir` span.
        // The accumulators aggregate across the producer/consumer threads (a
        // per-call span can't), while the span's own duration is the wall-clock
        // total — together they replace the old SCAN_SUMMARY timing log.
        let span = tracing::Span::current();
        span.record("snapshot_ms", snap_ms);
        span.record("extract_ms", extract_ms);
        span.record("hash_ms", hash_ms);
        span.record("motion_ms", motion_ms);
        span.record("upsert_ms", upsert_ms);
        tracing::debug!(
            target: crate::core::log_targets::STORAGE,
            "SCAN_SUMMARY root={} visited={} supported={} unchanged={} errors={} none_mime={} indexed={}",
            root.display(),
            visited,
            supported,
            unchanged,
            errors,
            none_mime,
            indexed
        );
        Ok(indexed)
    }

    /// `file_meta` 由调用方提供（扫描热路径已为未改动短路 stat 过一次），避免在这里
    /// 重复 stat——此前生产者对每个文件 stat 多达 3 次（`is_file` + 短路 metadata + 这里），
    /// 合并后全程只 stat 1 次。
    fn process_file(
        &self,
        path: &Path,
        file_meta: &std::fs::Metadata,
    ) -> Result<Option<NewMediaItem>> {
        Self::process_file_static(path, file_meta)
    }

    fn process_file_static(
        path: &Path,
        file_meta: &std::fs::Metadata,
    ) -> Result<Option<NewMediaItem>> {
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
        let motion_photo = if meta.mime_type == "image/jpeg" {
            match head.as_deref() {
                Some(h) => motion_photo::detect_with_head(path, h, file_meta.len()),
                None => motion_photo::detect(path),
            }
        } else {
            None
        };
        SCAN_MOTION_MS.fetch_add(t_motion.elapsed().as_millis() as u64, Ordering::Relaxed);

        let mut media_attributes = motion_photo
            .map(|info| MediaAttributes {
                motion_photo: Some(info),
                ..MediaAttributes::default()
            })
            .unwrap_or_default();
        media_attributes.animated = meta.mime_type == "image/gif";

        Ok(Some(NewMediaItem {
            uri,
            path: path.to_path_buf(),
            folder_path: folder,
            mime_type: meta.mime_type,
            media_subkind: if media_attributes.motion_photo.is_some() {
                MEDIA_SUBKIND_MOTION_PHOTO.into()
            } else {
                MEDIA_SUBKIND_STANDARD.into()
            },
            media_attributes: media_attributes.to_json(),
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
        let motion_photo = if meta.mime_type == "image/jpeg" {
            motion_photo::detect(source)
        } else {
            None
        };
        let mut media_attributes = motion_photo
            .map(|info| MediaAttributes {
                motion_photo: Some(info),
                ..MediaAttributes::default()
            })
            .unwrap_or_default();
        media_attributes.animated = meta.mime_type == "image/gif";
        Ok(NewMediaItem {
            uri: uri.to_string(),
            path: path.to_path_buf(),
            folder_path: folder.to_path_buf(),
            mime_type: meta.mime_type,
            media_subkind: if media_attributes.motion_photo.is_some() {
                MEDIA_SUBKIND_MOTION_PHOTO.into()
            } else {
                MEDIA_SUBKIND_STANDARD.into()
            },
            media_attributes: media_attributes.to_json(),
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
        let Some(item) = Self::new_item_from_path(path)? else {
            return Ok(None);
        };
        self.upsert(&item).map(Some)
    }

    /// 从单个文件路径提取元数据并生成待写入 DB 的 `NewMediaItem`。
    ///
    /// 该方法不访问数据库，供文件监听器在后台线程完成文件系统/解码工作后，
    /// 将真正的 DB 提交交给 `DbActor` 单线程串行执行。
    pub fn new_item_from_path(path: &Path) -> Result<Option<NewMediaItem>> {
        // 一次 stat 既判 is_file 又喂给 process_file，避免重复 stat。
        let file_meta = match std::fs::metadata(path) {
            Ok(m) if m.is_file() => m,
            _ => return Ok(None),
        };
        Self::process_file_static(path, &file_meta)
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
mod tests;
