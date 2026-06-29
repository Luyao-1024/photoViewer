//! Runtime strategy and sizing configuration persisted in `runtime.json`.

use std::path::Path;

use serde_json::{Map, Value};

use crate::config::config_dir;

const RUNTIME_CONFIG_FILE: &str = "runtime.json";

const INITIAL_MEDIA_PAGE_SIZE_KEY: &str = "initial_media_page_size";
const VIRTUAL_MEDIA_PAGE_SIZE_KEY: &str = "virtual_media_page_size";
const UI_MEDIA_LIST_CAP_KEY: &str = "ui_media_list_cap";
const MAX_RENDERED_GRID_ITEMS_KEY: &str = "max_rendered_grid_items";
const GRID_RENDER_ABSOLUTE_CAP_KEY: &str = "grid_render_absolute_cap";
const GRID_RENDER_EXPAND_STEP_KEY: &str = "grid_render_expand_step";
const GRID_REPRIORITIZE_DEBOUNCE_MS_KEY: &str = "grid_reprioritize_debounce_ms";
const THUMBNAIL_WORKER_COUNT_KEY: &str = "thumbnail_worker_count";
const THUMBNAIL_QUEUE_CAPACITY_KEY: &str = "thumbnail_queue_capacity";
const THUMBNAIL_MEM_CACHE_CAP_KEY: &str = "thumbnail_mem_cache_cap";
const THUMBNAIL_DISK_CACHE_BYTES_KEY: &str = "thumbnail_disk_cache_bytes";
const THUMBNAIL_PREWARM_POLL_MS_KEY: &str = "thumbnail_prewarm_poll_ms";
const THUMBNAIL_IDLE_WAIT_MS_KEY: &str = "thumbnail_idle_wait_ms";
const NOTIFY_TRASH_DEBOUNCE_MS_KEY: &str = "notify_trash_debounce_ms";
const NOTIFY_FILE_SETTLE_MS_KEY: &str = "notify_file_settle_ms";

pub const DEFAULT_INITIAL_MEDIA_PAGE_SIZE: u32 = 500;
pub const DEFAULT_VIRTUAL_MEDIA_PAGE_SIZE: u32 = 500;
pub const DEFAULT_UI_MEDIA_LIST_CAP: usize = 1500;
pub const DEFAULT_MAX_RENDERED_GRID_ITEMS: usize = 800;
pub const DEFAULT_GRID_RENDER_ABSOLUTE_CAP: usize = 1_200;
pub const DEFAULT_GRID_RENDER_EXPAND_STEP: usize = 200;
pub const DEFAULT_GRID_REPRIORITIZE_DEBOUNCE_MS: u64 = 120;
pub const DEFAULT_THUMBNAIL_QUEUE_CAPACITY: usize = 8192;
pub const DEFAULT_THUMBNAIL_MEM_CACHE_CAP: usize = 16;
pub const DEFAULT_THUMBNAIL_DISK_CACHE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const DEFAULT_THUMBNAIL_PREWARM_POLL_MS: u64 = 500;
pub const DEFAULT_THUMBNAIL_IDLE_WAIT_MS: u64 = 30_000;
pub const DEFAULT_NOTIFY_TRASH_DEBOUNCE_MS: u64 = 400;
pub const DEFAULT_NOTIFY_FILE_SETTLE_MS: u64 = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub initial_media_page_size: u32,
    pub virtual_media_page_size: u32,
    pub ui_media_list_cap: usize,
    pub max_rendered_grid_items: usize,
    pub grid_render_absolute_cap: usize,
    pub grid_render_expand_step: usize,
    pub grid_reprioritize_debounce_ms: u64,
    pub thumbnail_worker_count: usize,
    pub thumbnail_queue_capacity: usize,
    pub thumbnail_mem_cache_cap: usize,
    pub thumbnail_disk_cache_bytes: u64,
    pub thumbnail_prewarm_poll_ms: u64,
    pub thumbnail_idle_wait_ms: u64,
    pub notify_trash_debounce_ms: u64,
    pub notify_file_settle_ms: u64,
}

fn runtime_config_path() -> std::path::PathBuf {
    config_dir().join(RUNTIME_CONFIG_FILE)
}

fn read_object_at(path: &Path) -> Map<String, Value> {
    match std::fs::read_to_string(path) {
        Ok(data) => serde_json::from_str::<Value>(&data)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default(),
        Err(_) => Map::new(),
    }
}

fn read_runtime_config_at(path: &Path) -> RuntimeConfig {
    let obj = read_object_at(path);
    RuntimeConfig {
        initial_media_page_size: read_u32(
            &obj,
            INITIAL_MEDIA_PAGE_SIZE_KEY,
            DEFAULT_INITIAL_MEDIA_PAGE_SIZE,
        ),
        virtual_media_page_size: read_u32(
            &obj,
            VIRTUAL_MEDIA_PAGE_SIZE_KEY,
            DEFAULT_VIRTUAL_MEDIA_PAGE_SIZE,
        ),
        ui_media_list_cap: read_usize(&obj, UI_MEDIA_LIST_CAP_KEY, DEFAULT_UI_MEDIA_LIST_CAP),
        max_rendered_grid_items: read_usize(
            &obj,
            MAX_RENDERED_GRID_ITEMS_KEY,
            DEFAULT_MAX_RENDERED_GRID_ITEMS,
        ),
        grid_render_absolute_cap: read_usize(
            &obj,
            GRID_RENDER_ABSOLUTE_CAP_KEY,
            DEFAULT_GRID_RENDER_ABSOLUTE_CAP,
        ),
        grid_render_expand_step: read_usize(
            &obj,
            GRID_RENDER_EXPAND_STEP_KEY,
            DEFAULT_GRID_RENDER_EXPAND_STEP,
        ),
        grid_reprioritize_debounce_ms: read_u64(
            &obj,
            GRID_REPRIORITIZE_DEBOUNCE_MS_KEY,
            DEFAULT_GRID_REPRIORITIZE_DEBOUNCE_MS,
        ),
        thumbnail_worker_count: read_usize(
            &obj,
            THUMBNAIL_WORKER_COUNT_KEY,
            default_thumbnail_worker_count(),
        ),
        thumbnail_queue_capacity: read_usize(
            &obj,
            THUMBNAIL_QUEUE_CAPACITY_KEY,
            DEFAULT_THUMBNAIL_QUEUE_CAPACITY,
        ),
        thumbnail_mem_cache_cap: read_usize(
            &obj,
            THUMBNAIL_MEM_CACHE_CAP_KEY,
            DEFAULT_THUMBNAIL_MEM_CACHE_CAP,
        ),
        thumbnail_disk_cache_bytes: read_u64(
            &obj,
            THUMBNAIL_DISK_CACHE_BYTES_KEY,
            DEFAULT_THUMBNAIL_DISK_CACHE_BYTES,
        ),
        thumbnail_prewarm_poll_ms: read_u64(
            &obj,
            THUMBNAIL_PREWARM_POLL_MS_KEY,
            DEFAULT_THUMBNAIL_PREWARM_POLL_MS,
        ),
        thumbnail_idle_wait_ms: read_u64(
            &obj,
            THUMBNAIL_IDLE_WAIT_MS_KEY,
            DEFAULT_THUMBNAIL_IDLE_WAIT_MS,
        ),
        notify_trash_debounce_ms: read_u64(
            &obj,
            NOTIFY_TRASH_DEBOUNCE_MS_KEY,
            DEFAULT_NOTIFY_TRASH_DEBOUNCE_MS,
        ),
        notify_file_settle_ms: read_u64(
            &obj,
            NOTIFY_FILE_SETTLE_MS_KEY,
            DEFAULT_NOTIFY_FILE_SETTLE_MS,
        ),
    }
}

pub fn load() -> RuntimeConfig {
    read_runtime_config_at(&runtime_config_path())
}

pub fn initial_media_page_size() -> u32 {
    load().initial_media_page_size
}

pub fn virtual_media_page_size() -> u32 {
    load().virtual_media_page_size
}

pub fn ui_media_list_cap() -> usize {
    load().ui_media_list_cap
}

pub fn max_rendered_grid_items() -> usize {
    load().max_rendered_grid_items
}

pub fn grid_render_absolute_cap() -> usize {
    load().grid_render_absolute_cap
}

pub fn grid_render_expand_step() -> usize {
    load().grid_render_expand_step
}

pub fn grid_reprioritize_debounce_ms() -> u64 {
    load().grid_reprioritize_debounce_ms
}

pub fn thumbnail_worker_count() -> usize {
    load().thumbnail_worker_count
}

pub fn thumbnail_queue_capacity() -> usize {
    load().thumbnail_queue_capacity
}

pub fn thumbnail_mem_cache_cap() -> usize {
    load().thumbnail_mem_cache_cap
}

pub fn thumbnail_disk_cache_bytes() -> u64 {
    load().thumbnail_disk_cache_bytes
}

pub fn thumbnail_prewarm_poll_ms() -> u64 {
    load().thumbnail_prewarm_poll_ms
}

pub fn thumbnail_idle_wait_ms() -> u64 {
    load().thumbnail_idle_wait_ms
}

pub fn notify_trash_debounce_ms() -> u64 {
    load().notify_trash_debounce_ms
}

pub fn notify_file_settle_ms() -> u64 {
    load().notify_file_settle_ms
}

fn default_thumbnail_worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(4, 8)
}

fn read_u32(obj: &Map<String, Value>, key: &str, default: u32) -> u32 {
    obj.get(key)
        .and_then(|v| v.as_u64())
        .map(|v| v.clamp(1, u32::MAX as u64) as u32)
        .unwrap_or(default)
}

fn read_usize(obj: &Map<String, Value>, key: &str, default: usize) -> usize {
    obj.get(key)
        .and_then(|v| v.as_u64())
        .map(|v| v.max(1) as usize)
        .unwrap_or(default)
}

fn read_u64(obj: &Map<String, Value>, key: &str, default: u64) -> u64 {
    obj.get(key)
        .and_then(|v| v.as_u64())
        .map(|v| v.max(1))
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "photoViewer-runtime-config-test-{}-{}-{}",
            std::process::id(),
            name,
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        path
    }

    fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn missing_runtime_file_uses_central_defaults() {
        let path = tmp_path("missing");
        cleanup(&path);

        let config = read_runtime_config_at(&path);

        assert_eq!(config.initial_media_page_size, 500);
        assert_eq!(config.virtual_media_page_size, 500);
        assert_eq!(config.ui_media_list_cap, 1500);
        assert_eq!(config.max_rendered_grid_items, 800);
        assert_eq!(config.grid_render_absolute_cap, 1_200);
        assert_eq!(config.grid_render_expand_step, 200);
        assert_eq!(config.grid_reprioritize_debounce_ms, 120);
        assert_eq!(config.thumbnail_queue_capacity, 8192);
        assert_eq!(config.thumbnail_mem_cache_cap, 16);
        assert_eq!(config.thumbnail_disk_cache_bytes, 2 * 1024 * 1024 * 1024);
        assert_eq!(config.thumbnail_prewarm_poll_ms, 500);
        assert_eq!(config.thumbnail_idle_wait_ms, 30_000);
        assert_eq!(config.notify_trash_debounce_ms, 400);
        assert_eq!(config.notify_file_settle_ms, 50);

        cleanup(&path);
    }

    #[test]
    fn runtime_file_overrides_sizing_and_strategy_values() {
        let path = tmp_path("overrides");
        cleanup(&path);
        std::fs::write(
            &path,
            r#"{
              "initial_media_page_size": 300,
              "virtual_media_page_size": 700,
              "ui_media_list_cap": 2100,
              "max_rendered_grid_items": 650,
              "grid_render_absolute_cap": 1300,
              "grid_render_expand_step": 250,
              "grid_reprioritize_debounce_ms": 160,
              "thumbnail_worker_count": 3,
              "thumbnail_queue_capacity": 4096,
              "thumbnail_mem_cache_cap": 24,
              "thumbnail_disk_cache_bytes": 104857600,
              "thumbnail_prewarm_poll_ms": 750,
              "thumbnail_idle_wait_ms": 45000,
              "notify_trash_debounce_ms": 900,
              "notify_file_settle_ms": 125
            }"#,
        )
        .unwrap();

        let config = read_runtime_config_at(&path);

        assert_eq!(config.initial_media_page_size, 300);
        assert_eq!(config.virtual_media_page_size, 700);
        assert_eq!(config.ui_media_list_cap, 2100);
        assert_eq!(config.max_rendered_grid_items, 650);
        assert_eq!(config.grid_render_absolute_cap, 1300);
        assert_eq!(config.grid_render_expand_step, 250);
        assert_eq!(config.grid_reprioritize_debounce_ms, 160);
        assert_eq!(config.thumbnail_worker_count, 3);
        assert_eq!(config.thumbnail_queue_capacity, 4096);
        assert_eq!(config.thumbnail_mem_cache_cap, 24);
        assert_eq!(config.thumbnail_disk_cache_bytes, 104857600);
        assert_eq!(config.thumbnail_prewarm_poll_ms, 750);
        assert_eq!(config.thumbnail_idle_wait_ms, 45000);
        assert_eq!(config.notify_trash_debounce_ms, 900);
        assert_eq!(config.notify_file_settle_ms, 125);

        cleanup(&path);
    }

    #[test]
    fn invalid_or_tiny_runtime_values_are_clamped() {
        let path = tmp_path("clamped");
        cleanup(&path);
        std::fs::write(
            &path,
            r#"{
              "initial_media_page_size": 0,
              "virtual_media_page_size": 0,
              "ui_media_list_cap": 0,
              "max_rendered_grid_items": 0,
              "grid_render_absolute_cap": 0,
              "grid_render_expand_step": 0,
              "grid_reprioritize_debounce_ms": 0,
              "thumbnail_worker_count": 0,
              "thumbnail_queue_capacity": 0,
              "thumbnail_mem_cache_cap": 0,
              "thumbnail_disk_cache_bytes": 0,
              "thumbnail_prewarm_poll_ms": 0,
              "thumbnail_idle_wait_ms": 0,
              "notify_trash_debounce_ms": 0,
              "notify_file_settle_ms": 0
            }"#,
        )
        .unwrap();

        let config = read_runtime_config_at(&path);

        assert_eq!(config.initial_media_page_size, 1);
        assert_eq!(config.virtual_media_page_size, 1);
        assert_eq!(config.ui_media_list_cap, 1);
        assert_eq!(config.max_rendered_grid_items, 1);
        assert_eq!(config.grid_render_absolute_cap, 1);
        assert_eq!(config.grid_render_expand_step, 1);
        assert_eq!(config.grid_reprioritize_debounce_ms, 1);
        assert_eq!(config.thumbnail_worker_count, 1);
        assert_eq!(config.thumbnail_queue_capacity, 1);
        assert_eq!(config.thumbnail_mem_cache_cap, 1);
        assert_eq!(config.thumbnail_disk_cache_bytes, 1);
        assert_eq!(config.thumbnail_prewarm_poll_ms, 1);
        assert_eq!(config.thumbnail_idle_wait_ms, 1);
        assert_eq!(config.notify_trash_debounce_ms, 1);
        assert_eq!(config.notify_file_settle_ms, 1);

        cleanup(&path);
    }
}
