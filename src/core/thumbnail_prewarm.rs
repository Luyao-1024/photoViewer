//! 扫描后全库后台缩略图预热。
//!
//! 在启动扫描完成后，通知 ThumbnailLoader 开启后台预热模式：
//! 单个空闲 worker 从 DB 拉取一张未缓存的缩略图生成，再短暂让出 CPU。
//! 视口可见请求仍走 BOOST/NORMAL 优先队列，并可由其余 worker 并行处理。

use crate::core::thumbnails::ThumbnailLoader;
use std::sync::Arc;
use tracing::debug;

/// 启动后台预热：设置标志位 + 唤醒所有 worker。
///
/// 预热器本身不往队列灌任何项——worker 空闲时自己从 DB 分页查询、
/// 检查磁盘缓存是否已存在、只对缺失/变更的项生成缩略图。
pub fn start_background_prewarm(loader: &Arc<ThumbnailLoader>) {
    debug!("PREWARM start (pull model)");
    loader.start_background_prewarm();
}
