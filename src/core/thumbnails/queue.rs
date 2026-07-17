use super::cache::cache_key_str;
use super::decode::{generate, pixbuf_is_light, DecodeOrigin};
use super::{
    BackgroundPullState, LoadedThumb, LoaderState, PriItem, QueuedEntry, SharedQueue,
    SharedStatsDirtyCallback, TIER_BACKGROUND,
};
use crate::core::db::DbPool;
use crate::core::db_actor::{DbActorHandle, DbCommand};
use crate::core::identity::MediaId;
use crate::core::telemetry::{OperationTrace, TraceChain};
use gtk4::gdk::Texture;
use std::cmp::Reverse;
use std::path::PathBuf;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::{debug, warn};

pub(in crate::core::thumbnails) fn worker_loop(
    queue: SharedQueue,
    pool: DbPool,
    cache_dir: PathBuf,
    state: Arc<Mutex<LoaderState>>,
    bg: Arc<BackgroundPullState>,
    db_actor: Option<DbActorHandle>,
    stats_dirty_callback: SharedStatsDirtyCallback,
) {
    while let Some(req) = next_request_or_pull(&queue, &pool, &bg, &state) {
        // This INFO-level wrapper is dormant unless the `thumbnail` chain (or
        // legacy all-span Chrome trace) is selected. It remains available in
        // release builds, unlike the high-volume debug-only decode spans.
        let operation_trace = OperationTrace::start(TraceChain::Thumbnail, "generate_thumbnail");
        let operation_stage = operation_trace.stage("worker_process");
        operation_stage.record("detail", req.uri.as_str());
        operation_stage.record("item_count", 1);
        operation_stage.record(
            "queue_wait_ms",
            req.enqueued_at.elapsed().as_millis() as u64,
        );
        // `thumb:process` spans the worker's per-item work (queue pickup →
        // result), with `queue_wait_ms` recorded as a field. It parents the
        // `thumb:generate` span created inside `generate`.
        let process_span = tracing::debug_span!(
            "thumb:process",
            uri = %req.uri,
            size = ?req.size,
            tier = req.tier,
            media_id = req.media_id,
            queue_wait_ms = req.enqueued_at.elapsed().as_millis(),
            // 在 generate 返回 DecodeOrigin 后 record：磁盘命中 vs 冷生成。
            cache_hit = tracing::field::Empty,
        );
        let _process_guard = process_span.enter();
        match generate(&cache_dir, &req.uri, req.size, req.mtime) {
            Ok((pb, origin)) => {
                // 磁盘命中意味着此前一次冷生成已标记过该行（缓存文件只在标记它的
                // 那次生成中写出），故命中无需重标；只有冷生成才需要更新
                // thumbnail_generated_at。
                let was_cache_hit = matches!(origin, DecodeOrigin::DiskCache);
                process_span.record("cache_hit", was_cache_hit);
                let generated_media_id = (req.media_id != 0).then_some(req.media_id);
                let is_light = pixbuf_is_light(&pb);
                let texture = Texture::for_pixbuf(&pb);
                let loaded = LoadedThumb {
                    texture: texture.clone(),
                    is_light,
                };
                let waiters = {
                    let mut st = match state.lock() {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                    // Keep background prewarm separate from foreground browse
                    // history. Otherwise a long idle prewarm pass can evict
                    // thumbnails the user just saw, defeating instant reverse
                    // scrolling. A foreground lookup promotes this small-cache
                    // entry into `mem_cache` on first use.
                    if req.tier == TIER_BACKGROUND {
                        st.prewarm_mem_cache
                            .put(req.cache_key.clone(), loaded.clone());
                    } else {
                        st.mem_cache.put(req.cache_key.clone(), loaded.clone());
                    }
                    st.in_flight.remove(&req.cache_key).unwrap_or_default()
                };
                debug!(
                    target: crate::core::log_targets::THUMBNAILS,
                    "THUMB_LOADER_TRACE worker_loaded uri={} size={:?} tier={} media_id={} texture={}x{} waiters={} cache_key={} cache_hit={}",
                    req.uri,
                    req.size,
                    req.tier,
                    req.media_id,
                    pb.width(),
                    pb.height(),
                    waiters.len(),
                    req.cache_key,
                    was_cache_hit,
                );
                if let Some(media_id) = generated_media_id {
                    if was_cache_hit {
                        // 磁盘命中：该行已被前一次冷生成标记过，无需重标——既省一次
                        // 经单写 DbActor 的 UPDATE（其尾部拥塞正是滚动卡顿来源），也让
                        // 热路径纹理在零 DB 记账下立即送达。
                        debug!(
                            target: crate::core::log_targets::THUMBNAILS,
                            "THUMB mark_skipped cache_hit media_id={} uri={}",
                            media_id, req.uri,
                        );
                    } else {
                        // 冷生成：同步更新 thumbnail_generated_at，保持「交付前已标记」
                        // 契约（集成测试与统计计数都依赖 rx 解析时该行已被标记）。
                        // debug span 度量 worker 在标记上的阻塞时间（仅冷路径）。
                        let result = {
                            let _mark =
                                tracing::debug_span!("thumb:mark_generated", media_id,).entered();
                            if let Some(actor) = db_actor.as_ref() {
                                actor
                                    .execute_blocking_in_trace(
                                        operation_trace.clone(),
                                        DbCommand::MarkThumbnailsGenerated {
                                            ids: vec![MediaId::from(media_id)],
                                        },
                                    )
                                    .map(|_| ())
                            } else {
                                crate::core::db::mark_thumbnails_generated(&pool, &[media_id])
                                    .map(|_| ())
                            }
                        };
                        if let Err(e) = result {
                            crate::core::telemetry::log_warning(
                                &operation_trace,
                                "mark_generated",
                                &e,
                            );
                            warn!("更新缩略图状态失败: {}", e);
                        } else if let Ok(callback) = stats_dirty_callback.lock() {
                            debug!(
                                target: crate::core::log_targets::THUMBNAILS,
                                "THUMB_LOADER_TRACE mark_generated media_id={} uri={}",
                                media_id, req.uri,
                            );
                            if let Some(callback) = callback.as_ref() {
                                callback();
                            }
                        }
                    }
                }
                // 交付纹理：冷路径下标记已先于此完成（保契约）；命中路径无 DB 记账，
                // 纹理在 generate 后即刻送达。
                for waiter in waiters {
                    if !waiter.is_cancelled() {
                        let _ = waiter.reply.send(loaded.clone());
                    }
                }
            }
            Err(e) => {
                drop_in_flight(&state, &req.cache_key);
                crate::core::telemetry::log_error(&operation_trace, "generate", &e);
                warn!(
                    target: crate::core::log_targets::THUMBNAILS,
                    "THUMB worker_failed uri={} size={:?} tier={} error={}",
                    req.uri,
                    req.size,
                    req.tier,
                    e
                );
            }
        }
    }
}

/// 取下一个工作项：优先队列（网格请求），队列空时从 DB 批量拉取
/// `worker_count` 条需生成的项一次性入队并唤醒所有 worker。
pub(in crate::core::thumbnails) fn next_request_or_pull(
    queue: &SharedQueue,
    pool: &DbPool,
    bg: &Arc<BackgroundPullState>,
    state: &Arc<Mutex<LoaderState>>,
) -> Option<PriItem> {
    let (lock, cvar) = &**queue;
    loop {
        // 1) 优先从队列弹（BOOST/NORMAL，网格可见请求）
        let mut q = lock.lock().ok()?;
        loop {
            if q.closed {
                return None;
            }
            if let Some(Reverse(item)) = q.heap.pop() {
                if q.queued.get(&item.cache_key).map(|e| e.tier) == Some(item.tier) {
                    q.queued.remove(&item.cache_key);
                    return Some(item);
                }
                continue; // 过期项
            }
            break; // 堆空
        }
        drop(q);

        // 2) 队列空，从 DB 批量拉取需生成的项
        if bg.enabled.load(AtomicOrdering::Relaxed) {
            if let Some(item) = pull_batch_and_enqueue(pool, bg, queue, state) {
                return Some(item);
            }
        }

        // 3) 无可做，阻塞等待。
        let q = lock.lock().ok()?;
        if q.closed {
            return None;
        }
        if !q.heap.is_empty() {
            continue;
        }
        let wait_dur = if bg.enabled.load(AtomicOrdering::Relaxed) {
            std::time::Duration::from_millis(
                crate::core::runtime_config::thumbnail_prewarm_poll_ms(),
            )
        } else {
            std::time::Duration::from_millis(crate::core::runtime_config::thumbnail_idle_wait_ms())
        };
        let (q2, _timed_out) = cvar.wait_timeout(q, wait_dur).ok()?;
        drop(q2);
    }
}

/// 从 DB 批量拉取 `worker_count` 条需生成的项，全部入队并唤醒其他 worker，
/// 返回一条给调用方自己处理（等价于调用方先从队里弹一条）。
///
/// 已缓存（`thumbnail_generated_at >= file_mtime`）的项由 DB 查询自动过滤，
/// 不再需要磁盘 stat。拉取到末尾返回 `None`；下次超时重试时会因为已缓存项增加
/// 而自然收敛。
pub(in crate::core::thumbnails) fn pull_batch_and_enqueue(
    pool: &DbPool,
    bg: &BackgroundPullState,
    queue: &SharedQueue,
    state: &Mutex<LoaderState>,
) -> Option<PriItem> {
    let batch_size = *bg.worker_count.lock().ok()? as u32;
    let start_offset = {
        let mut off = bg.offset.lock().ok()?;
        let start = *off;
        *off = off.saturating_add(batch_size);
        start
    };
    let page = crate::core::db::list_media_needing_thumbnail_from_live_offset(
        pool,
        start_offset,
        batch_size,
    )
    .ok()?;
    if page.is_empty() {
        if let Ok(mut off) = bg.offset.lock() {
            *off = 0;
        }
        return None;
    }

    let size = *bg.size.lock().ok()?;

    // 全部转成 PriItem，批量入队
    let items: Vec<PriItem> = page
        .iter()
        .map(|item| {
            let mtime = Some(std::time::SystemTime::from(item.file_mtime));
            let cache_key =
                cache_key_str(&item.uri, size, mtime).unwrap_or_else(|| format!("bg:{}", item.uri));
            PriItem {
                tier: TIER_BACKGROUND,
                seq: 0,
                cache_key,
                uri: item.uri.clone(),
                size,
                mtime,
                enqueued_at: Instant::now(),
                media_id: item.id,
            }
        })
        .collect();

    let (lock, cvar) = &**queue;
    let mut st = state.lock().ok()?;
    let mut q = lock.lock().ok()?;
    let mut first = None;
    for item in items {
        if st.in_flight.contains_key(&item.cache_key) || q.queued.contains_key(&item.cache_key) {
            continue;
        }
        if first.is_none() {
            st.in_flight.insert(item.cache_key.clone(), Vec::new());
            first = Some(item);
            continue;
        }
        if q.queued.len() < crate::core::runtime_config::thumbnail_queue_capacity() {
            st.in_flight.insert(item.cache_key.clone(), Vec::new());
            q.queued.insert(
                item.cache_key.clone(),
                QueuedEntry {
                    tier: TIER_BACKGROUND,
                    uri: item.uri.clone(),
                    size: item.size,
                    mtime: item.mtime,
                    enqueued_at: item.enqueued_at,
                    media_id: item.media_id,
                },
            );
            q.heap.push(Reverse(item));
        }
    }
    let queued_any = !q.heap.is_empty();
    drop(q);
    drop(st);
    if queued_any {
        // 唤醒所有 sleep 的 worker 来消费刚入队的项。
        cvar.notify_all();
    }

    first
}

/// 生成失败时移除在途项，让等待者的 `rx` 收到 `Err` 而非永久挂起。
pub(in crate::core::thumbnails) fn drop_in_flight(state: &Mutex<LoaderState>, cache_key: &str) {
    if let Ok(mut st) = state.lock() {
        st.in_flight.remove(cache_key);
    }
}
