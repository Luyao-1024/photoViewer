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
const PHOTOS_GRID_COLUMNS_KEY: &str = "photos_grid_columns";
const THUMBNAIL_WORKER_COUNT_KEY: &str = "thumbnail_worker_count";
const THUMBNAIL_SPEED_TIER_KEY: &str = "thumbnail_speed_tier";
const THUMBNAIL_QUEUE_CAPACITY_KEY: &str = "thumbnail_queue_capacity";
const THUMBNAIL_MEM_CACHE_CAP_KEY: &str = "thumbnail_mem_cache_cap";
const THUMBNAIL_DISK_CACHE_BYTES_KEY: &str = "thumbnail_disk_cache_bytes";
const THUMBNAIL_PREWARM_POLL_MS_KEY: &str = "thumbnail_prewarm_poll_ms";
const THUMBNAIL_IDLE_WAIT_MS_KEY: &str = "thumbnail_idle_wait_ms";
const NOTIFY_TRASH_DEBOUNCE_MS_KEY: &str = "notify_trash_debounce_ms";
const NOTIFY_FILE_SETTLE_MS_KEY: &str = "notify_file_settle_ms";
// Progressive first-page render: render a viewport-sized seed of tiles first,
// then fill the rest of the first page in paced background ticks so the window
// is interactive long before all loaded tiles are built.
const STARTUP_PROGRESSIVE_RENDER_KEY: &str = "startup_progressive_render";
const STARTUP_RENDER_SEED_KEY: &str = "startup_render_seed";
const STARTUP_RENDER_BATCH_KEY: &str = "startup_render_batch";
const STARTUP_RENDER_INTERVAL_MS_KEY: &str = "startup_render_interval_ms";
const STARTUP_RENDER_FIRST_TICK_DELAY_MS_KEY: &str = "startup_render_first_tick_delay_ms";

pub const DEFAULT_INITIAL_MEDIA_PAGE_SIZE: u32 = 500;
pub const DEFAULT_VIRTUAL_MEDIA_PAGE_SIZE: u32 = 500;
pub const DEFAULT_UI_MEDIA_LIST_CAP: usize = 1500;
pub const DEFAULT_MAX_RENDERED_GRID_ITEMS: usize = 800;
pub const DEFAULT_GRID_RENDER_ABSOLUTE_CAP: usize = 1_200;
pub const DEFAULT_GRID_RENDER_EXPAND_STEP: usize = 200;
pub const DEFAULT_GRID_REPRIORITIZE_DEBOUNCE_MS: u64 = 120;
pub const DEFAULT_PHOTOS_GRID_COLUMNS: usize = 4;
pub const MIN_PHOTOS_GRID_COLUMNS: usize = 1;
pub const MAX_PHOTOS_GRID_COLUMNS: usize = 12;
pub const DEFAULT_THUMBNAIL_QUEUE_CAPACITY: usize = 8192;
/// Sized for the **grid scroll working set** (Day/Month Medium tiles), not just
/// viewer swipe-back. A traced warm-cache scroll showed ~84% of worker time was
/// re-decoding disk-cached JPEGs because the 128-entry LRU evicted visible tiles
/// and they were re-decoded on scroll-back / overscan churn. 256 holds ~3.5× the
/// ~72-tile Day landing window (visible + 4× overscan) plus scroll-back headroom,
/// halving the re-decode ratio for ~+128 MB resident (Medium ≈ 0.75–1 MB/entry).
/// Viewer ±1 prefetch remains a strict subset. Worst case (all-Large, 4 MB/entry)
/// is bounded in practice because Large is only requested from trash/album-browser
/// (dozens of items), not the scroll grid. Override via `runtime.json`
/// `thumbnail_mem_cache_cap`.
pub const DEFAULT_THUMBNAIL_MEM_CACHE_CAP: usize = 256;
pub const DEFAULT_THUMBNAIL_DISK_CACHE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const DEFAULT_THUMBNAIL_PREWARM_POLL_MS: u64 = 500;
pub const DEFAULT_THUMBNAIL_IDLE_WAIT_MS: u64 = 30_000;
pub const DEFAULT_NOTIFY_TRASH_DEBOUNCE_MS: u64 = 400;
pub const DEFAULT_NOTIFY_FILE_SETTLE_MS: u64 = 50;
/// Master switch for progressive first-page grid rendering.
pub const DEFAULT_STARTUP_PROGRESSIVE_RENDER: bool = true;
/// Number of tiles built on the first (seed) render — sized to roughly fill
/// ~1.5× the Day-mode viewport (Day tiles are the largest, so this also
/// comfortably fills Year/Month which show more tiles per row).
pub const DEFAULT_STARTUP_RENDER_SEED: usize = 48;
/// Tiles appended per progressive tick.
pub const DEFAULT_STARTUP_RENDER_BATCH: usize = 96;
/// Delay between progressive ticks — lets the compositor paint the newly added
/// tiles and process input between chunks so the window stays responsive.
pub const DEFAULT_STARTUP_RENDER_INTERVAL_MS: u64 = 20;
/// Delay before the FIRST progressive tick — longer than `interval` so the
/// seed render's thumbnails generate and deliver (set_paintable needs the main
/// thread) before the fill starts competing for it. Directly lowers the
/// launch→first-thumbnail time.
pub const DEFAULT_STARTUP_RENDER_FIRST_TICK_DELAY_MS: u64 = 150;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub initial_media_page_size: u32,
    pub virtual_media_page_size: u32,
    pub ui_media_list_cap: usize,
    pub max_rendered_grid_items: usize,
    pub grid_render_absolute_cap: usize,
    pub grid_render_expand_step: usize,
    pub grid_reprioritize_debounce_ms: u64,
    pub photos_grid_columns: usize,
    pub thumbnail_worker_count: usize,
    pub thumbnail_queue_capacity: usize,
    pub thumbnail_mem_cache_cap: usize,
    pub thumbnail_disk_cache_bytes: u64,
    pub thumbnail_prewarm_poll_ms: u64,
    pub thumbnail_idle_wait_ms: u64,
    pub notify_trash_debounce_ms: u64,
    pub notify_file_settle_ms: u64,
    pub startup_progressive_render: bool,
    pub startup_render_seed: usize,
    pub startup_render_batch: usize,
    pub startup_render_interval_ms: u64,
    pub startup_render_first_tick_delay_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressiveRenderPlan {
    /// Number of items to load into the GTK-facing model window.
    pub model_limit: usize,
    /// Number of loaded items to build on the first grid rebuild.
    pub first_render_limit: usize,
    /// Whether remaining loaded items should be filled in paced ticks.
    pub progressive: bool,
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
        photos_grid_columns: read_bounded_usize(
            &obj,
            PHOTOS_GRID_COLUMNS_KEY,
            DEFAULT_PHOTOS_GRID_COLUMNS,
            MIN_PHOTOS_GRID_COLUMNS,
            MAX_PHOTOS_GRID_COLUMNS,
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
        startup_progressive_render: read_bool(
            &obj,
            STARTUP_PROGRESSIVE_RENDER_KEY,
            DEFAULT_STARTUP_PROGRESSIVE_RENDER,
        ),
        startup_render_seed: read_usize(&obj, STARTUP_RENDER_SEED_KEY, DEFAULT_STARTUP_RENDER_SEED),
        startup_render_batch: read_usize(
            &obj,
            STARTUP_RENDER_BATCH_KEY,
            DEFAULT_STARTUP_RENDER_BATCH,
        ),
        startup_render_interval_ms: read_u64(
            &obj,
            STARTUP_RENDER_INTERVAL_MS_KEY,
            DEFAULT_STARTUP_RENDER_INTERVAL_MS,
        ),
        startup_render_first_tick_delay_ms: read_u64(
            &obj,
            STARTUP_RENDER_FIRST_TICK_DELAY_MS_KEY,
            DEFAULT_STARTUP_RENDER_FIRST_TICK_DELAY_MS,
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

pub fn photos_grid_columns() -> usize {
    load().photos_grid_columns
}

pub fn set_photos_grid_columns(columns: usize) -> Result<(), String> {
    let columns = columns.clamp(MIN_PHOTOS_GRID_COLUMNS, MAX_PHOTOS_GRID_COLUMNS);
    write_usize_at(&runtime_config_path(), PHOTOS_GRID_COLUMNS_KEY, columns)
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

pub fn startup_progressive_render() -> bool {
    load().startup_progressive_render
}

pub fn startup_render_seed() -> usize {
    load().startup_render_seed
}

pub fn startup_render_batch() -> usize {
    load().startup_render_batch
}

pub fn startup_render_interval_ms() -> u64 {
    load().startup_render_interval_ms
}

pub fn startup_render_first_tick_delay_ms() -> u64 {
    load().startup_render_first_tick_delay_ms
}

/// Build the shared loading/rendering plan for a bounded first page.
///
/// Pure (no I/O) so it can be unit-tested independently of `load()`.
pub fn progressive_render_plan(
    enabled: bool,
    seed: usize,
    total_len: usize,
    steady_limit: usize,
) -> ProgressiveRenderPlan {
    let model_limit = total_len.min(steady_limit);
    let first_render_limit = if enabled && seed > 0 && model_limit > seed {
        seed
    } else {
        model_limit
    };
    ProgressiveRenderPlan {
        model_limit,
        first_render_limit,
        progressive: first_render_limit < model_limit,
    }
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

fn read_bounded_usize(
    obj: &Map<String, Value>,
    key: &str,
    default: usize,
    minimum: usize,
    maximum: usize,
) -> usize {
    obj.get(key)
        .and_then(|v| v.as_u64())
        .map(|v| (v as usize).clamp(minimum, maximum))
        .unwrap_or(default.clamp(minimum, maximum))
}

fn read_u64(obj: &Map<String, Value>, key: &str, default: u64) -> u64 {
    obj.get(key)
        .and_then(|v| v.as_u64())
        .map(|v| v.max(1))
        .unwrap_or(default)
}

fn read_bool(obj: &Map<String, Value>, key: &str, default: bool) -> bool {
    obj.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}

#[cfg(test)]
mod tests;
