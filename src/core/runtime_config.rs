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
const THUMBNAIL_SPEED_TIER_KEY: &str = "thumbnail_speed_tier";
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailGenerationSpeed {
    Slow,
    Normal,
    Fast,
    Fastest,
}

impl ThumbnailGenerationSpeed {
    pub fn worker_count(self) -> usize {
        match self {
            Self::Slow => 1,
            Self::Normal => 2,
            Self::Fast => 4,
            Self::Fastest => available_parallelism(),
        }
    }

    /// 稳定的字符串标识，用于持久化档位（避免 worker_count↔tier 的歧义映射）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Slow => "slow",
            Self::Normal => "normal",
            Self::Fast => "fast",
            Self::Fastest => "fastest",
        }
    }

    /// 旧配置迁移回退：仅当 `runtime.json` 没有 tier 字符串时，从 worker_count 反推。
    /// 注意此映射在 Fastest(=cpus) 与 Fast(=4) 数值相近时本就有歧义，故新代码持久化 tier。
    pub fn from_worker_count(count: usize) -> Self {
        let cpus = available_parallelism();
        if count >= cpus {
            return Self::Fastest;
        }
        match count {
            0 | 1 => Self::Slow,
            2 => Self::Normal,
            _ => Self::Fast,
        }
    }
}

/// 解析持久化的 tier 字符串。未知值 → `Err`，调用方据此回退到 `from_worker_count`。
impl std::str::FromStr for ThumbnailGenerationSpeed {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "slow" => Ok(Self::Slow),
            "normal" => Ok(Self::Normal),
            "fast" => Ok(Self::Fast),
            "fastest" => Ok(Self::Fastest),
            _ => Err(()),
        }
    }
}

/// CPU 物理核心数（至少为 1），用于 Fastest 档位的 worker 数量。
/// 通过 sysfs 读取每个 `cpuN` 的 `(physical_package_id, core_id)` 组合并去重——
/// 单独用 `core_id` 会在多路/多 CCX 系统上少算（不同 socket 复用相同 core_id）。
/// 读取失败则回退到 `available_parallelism() / 2`。
fn physical_core_count() -> usize {
    let mut cores = std::collections::BTreeSet::<(String, String)>::new();
    if let Ok(entries) = std::fs::read_dir("/sys/devices/system/cpu") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(num) = name.strip_prefix("cpu") else {
                continue;
            };
            if !num.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let topo = entry.path().join("topology");
            let core_id = std::fs::read_to_string(topo.join("core_id"))
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            // 没有 core_id（离线核 / 无拓扑）→ 跳过，不计入
            if core_id.is_empty() {
                continue;
            }
            let pkg_id = std::fs::read_to_string(topo.join("physical_package_id"))
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            cores.insert((pkg_id, core_id));
        }
    }
    if !cores.is_empty() {
        return cores.len().max(1);
    }
    // Fallback：假设 SMT/HT 每核 2 线程
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(1))
        .unwrap_or(4)
}

/// CPU 物理核心数（至少为 1），用于 Fastest 档位的 worker 数量。
fn available_parallelism() -> usize {
    physical_core_count()
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

pub fn thumbnail_generation_speed() -> ThumbnailGenerationSpeed {
    // 优先读持久化的 tier 字符串（无歧义）；缺失则从 worker_count 迁移回退。
    let obj = read_object_at(&runtime_config_path());
    if let Some(tier) = obj
        .get(THUMBNAIL_SPEED_TIER_KEY)
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
    {
        return tier;
    }
    ThumbnailGenerationSpeed::from_worker_count(thumbnail_worker_count())
}

pub fn set_thumbnail_generation_speed(speed: ThumbnailGenerationSpeed) -> Result<(), String> {
    // 同时写 tier 字符串（UI 读它）和 worker_count（启动时 worker pool 读它）。
    write_string_at(
        &runtime_config_path(),
        THUMBNAIL_SPEED_TIER_KEY,
        speed.as_str(),
    )?;
    write_usize_at(
        &runtime_config_path(),
        THUMBNAIL_WORKER_COUNT_KEY,
        speed.worker_count(),
    )
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
    ThumbnailGenerationSpeed::Normal.worker_count()
}

fn write_usize_at(path: &Path, key: &str, value: usize) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut object = read_object_at(path);
    object.insert(key.to_string(), Value::from(value.max(1)));
    let json = serde_json::to_string_pretty(&Value::Object(object)).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

fn write_string_at(path: &Path, key: &str, value: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut object = read_object_at(path);
    object.insert(key.to_string(), Value::from(value));
    let json = serde_json::to_string_pretty(&Value::Object(object)).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
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
        assert_eq!(
            config.thumbnail_worker_count,
            ThumbnailGenerationSpeed::Normal.worker_count()
        );
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

    #[test]
    fn thumbnail_generation_speed_maps_to_worker_counts() {
        assert_eq!(ThumbnailGenerationSpeed::Slow.worker_count(), 1);
        assert_eq!(ThumbnailGenerationSpeed::Normal.worker_count(), 2);
        assert_eq!(ThumbnailGenerationSpeed::Fast.worker_count(), 4);
        assert_eq!(
            ThumbnailGenerationSpeed::Fastest.worker_count(),
            available_parallelism()
        );
        // tier 字符串往返（无歧义）
        for speed in [
            ThumbnailGenerationSpeed::Slow,
            ThumbnailGenerationSpeed::Normal,
            ThumbnailGenerationSpeed::Fast,
            ThumbnailGenerationSpeed::Fastest,
        ] {
            assert_eq!(
                speed.as_str().parse::<ThumbnailGenerationSpeed>().ok(),
                Some(speed),
                "tier as_str/parse 应往返"
            );
        }
        assert!("bogus".parse::<ThumbnailGenerationSpeed>().is_err());

        // from_worker_count 仅在值确定小于 cpus 时才断言（避免 4 核机器上 4>=4 命中 Fastest）
        let cpus = available_parallelism();
        assert_eq!(
            ThumbnailGenerationSpeed::from_worker_count(1),
            ThumbnailGenerationSpeed::Slow
        );
        if cpus > 2 {
            assert_eq!(
                ThumbnailGenerationSpeed::from_worker_count(2),
                ThumbnailGenerationSpeed::Normal
            );
        }
        if cpus > 3 {
            assert_eq!(
                ThumbnailGenerationSpeed::from_worker_count(3),
                ThumbnailGenerationSpeed::Fast
            );
        }
        if cpus > 4 {
            assert_eq!(
                ThumbnailGenerationSpeed::from_worker_count(4),
                ThumbnailGenerationSpeed::Fast
            );
        }
        // worker_count >= cpu_count maps to Fastest
        assert_eq!(
            ThumbnailGenerationSpeed::from_worker_count(cpus),
            ThumbnailGenerationSpeed::Fastest
        );
        assert_eq!(
            ThumbnailGenerationSpeed::from_worker_count(cpus + 10),
            ThumbnailGenerationSpeed::Fastest
        );
    }

    /// tier 字符串持久化往返：写 Fastest 后读回应是 Fastest（不受核数影响）。
    #[test]
    fn thumbnail_speed_tier_round_trips_via_string() {
        let path = tmp_path("thumbnail-tier");
        cleanup(&path);
        for speed in [
            ThumbnailGenerationSpeed::Slow,
            ThumbnailGenerationSpeed::Normal,
            ThumbnailGenerationSpeed::Fast,
            ThumbnailGenerationSpeed::Fastest,
        ] {
            // 直接写 tier 字符串 + worker_count，模拟 set_thumbnail_generation_speed。
            write_string_at(&path, THUMBNAIL_SPEED_TIER_KEY, speed.as_str()).unwrap();
            write_usize_at(&path, THUMBNAIL_WORKER_COUNT_KEY, speed.worker_count()).unwrap();

            let obj = read_object_at(&path);
            let read = obj
                .get(THUMBNAIL_SPEED_TIER_KEY)
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok());
            assert_eq!(read, Some(speed), "tier 字符串应往返 {speed:?}");
            // worker_count 也应与档位一致
            assert_eq!(
                obj.get(THUMBNAIL_WORKER_COUNT_KEY).and_then(|v| v.as_u64()),
                Some(speed.worker_count() as u64)
            );
        }
        cleanup(&path);
    }

    /// 旧配置迁移：只有 worker_count、没有 tier 字符串时，从 worker_count 回退推导。
    #[test]
    fn missing_tier_falls_back_to_worker_count() {
        let path = tmp_path("thumbnail-legacy");
        cleanup(&path);
        std::fs::write(&path, r#"{"thumbnail_worker_count": 2}"#).unwrap();
        let obj = read_object_at(&path);
        // 没有 tier 键 → 走 from_worker_count 回退
        let speed = obj
            .get(THUMBNAIL_SPEED_TIER_KEY)
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| {
                ThumbnailGenerationSpeed::from_worker_count(
                    obj.get(THUMBNAIL_WORKER_COUNT_KEY)
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize)
                        .unwrap_or(default_thumbnail_worker_count()),
                )
            });
        // 期望值随机器核数而变（2 核机器上 from_worker_count(2) 是 Fastest），
        // 故对照 from_worker_count(2) 本身——此测试验证的是「缺 tier 键 → 走
        // worker_count 回退」的接线，而非某个固定档位。
        assert_eq!(speed, ThumbnailGenerationSpeed::from_worker_count(2));
        cleanup(&path);
    }

    #[test]
    fn writing_thumbnail_generation_speed_preserves_runtime_keys() {
        let path = tmp_path("thumbnail-speed");
        cleanup(&path);
        std::fs::write(
            &path,
            r#"{
              "initial_media_page_size": 300,
              "thumbnail_queue_capacity": 4096
            }"#,
        )
        .unwrap();

        write_usize_at(
            &path,
            THUMBNAIL_WORKER_COUNT_KEY,
            ThumbnailGenerationSpeed::Normal.worker_count(),
        )
        .unwrap();

        let config = read_runtime_config_at(&path);
        assert_eq!(config.thumbnail_worker_count, 2);
        assert_eq!(config.initial_media_page_size, 300);
        assert_eq!(config.thumbnail_queue_capacity, 4096);

        cleanup(&path);
    }
}
