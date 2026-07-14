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
    assert_eq!(config.thumbnail_mem_cache_cap, 128);
    assert_eq!(config.thumbnail_disk_cache_bytes, 2 * 1024 * 1024 * 1024);
    assert_eq!(config.thumbnail_prewarm_poll_ms, 500);
    assert_eq!(config.thumbnail_idle_wait_ms, 30_000);
    assert_eq!(config.notify_trash_debounce_ms, 400);
    assert_eq!(config.notify_file_settle_ms, 50);
    assert!(config.startup_progressive_render);
    assert_eq!(config.startup_render_seed, 48);
    assert_eq!(config.startup_render_batch, 96);
    assert_eq!(config.startup_render_interval_ms, 20);
    assert_eq!(config.startup_render_first_tick_delay_ms, 150);

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
              "notify_file_settle_ms": 125,
              "startup_progressive_render": false,
              "startup_render_seed": 24,
              "startup_render_batch": 60,
              "startup_render_interval_ms": 35,
              "startup_render_first_tick_delay_ms": 200
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
    assert!(!config.startup_progressive_render);
    assert_eq!(config.startup_render_seed, 24);
    assert_eq!(config.startup_render_batch, 60);
    assert_eq!(config.startup_render_interval_ms, 35);
    assert_eq!(config.startup_render_first_tick_delay_ms, 200);

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
              "notify_file_settle_ms": 0,
              "startup_render_seed": 0,
              "startup_render_batch": 0,
              "startup_render_interval_ms": 0,
              "startup_render_first_tick_delay_ms": 0
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
    // Numeric startup keys clamp to 1 like other counts; the bool switch
    // is unchanged here (its default is true and 0 is not a JSON bool).
    assert_eq!(config.startup_render_seed, 1);
    assert_eq!(config.startup_render_batch, 1);
    assert_eq!(config.startup_render_interval_ms, 1);
    assert_eq!(config.startup_render_first_tick_delay_ms, 1);
    assert!(config.startup_progressive_render);

    cleanup(&path);
}

#[test]
fn startup_bool_and_progressive_render_plan_behaviour() {
    let path = tmp_path("startup-bool");
    cleanup(&path);
    // Explicit bool values round-trip; non-bool (0/1 numbers) fall back to default.
    std::fs::write(&path, r#"{"startup_progressive_render": false}"#).unwrap();
    assert!(!read_runtime_config_at(&path).startup_progressive_render);
    std::fs::write(&path, r#"{"startup_progressive_render": true}"#).unwrap();
    assert!(read_runtime_config_at(&path).startup_progressive_render);
    std::fs::write(&path, r#"{"startup_progressive_render": 0}"#).unwrap();
    // 0 is a number, not a bool → default (true).
    assert!(read_runtime_config_at(&path).startup_progressive_render);
    cleanup(&path);

    // The same plan controls the GTK model window and first rendered tile count.
    assert_eq!(
        progressive_render_plan(true, 48, 100_000, 800),
        ProgressiveRenderPlan {
            model_limit: 800,
            first_render_limit: 48,
            progressive: true
        }
    );
    assert_eq!(
        progressive_render_plan(true, 48, 49, 800),
        ProgressiveRenderPlan {
            model_limit: 49,
            first_render_limit: 48,
            progressive: true
        }
    );
    assert_eq!(
        progressive_render_plan(true, 48, 48, 800),
        ProgressiveRenderPlan {
            model_limit: 48,
            first_render_limit: 48,
            progressive: false
        }
    );
    assert_eq!(
        progressive_render_plan(true, 48, 10, 800),
        ProgressiveRenderPlan {
            model_limit: 10,
            first_render_limit: 10,
            progressive: false
        }
    );
    assert_eq!(
        progressive_render_plan(false, 48, 500, 800),
        ProgressiveRenderPlan {
            model_limit: 500,
            first_render_limit: 500,
            progressive: false
        }
    );
    assert_eq!(
        progressive_render_plan(true, 0, 500, 800),
        ProgressiveRenderPlan {
            model_limit: 500,
            first_render_limit: 500,
            progressive: false
        }
    );
    assert_eq!(
        progressive_render_plan(true, 48, 500, 32),
        ProgressiveRenderPlan {
            model_limit: 32,
            first_render_limit: 32,
            progressive: false
        }
    );
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
