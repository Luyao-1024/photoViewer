use super::{format_tile_duration, gray_placeholder_texture, thumbnail_request_mtime, ViewSpec};
use crate::core::media::MediaItem;
use crate::core::section_model::GroupBy;
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::square_tile::SquareTile;
use gtk4 as gtk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

pub(super) fn prepare_reused_tile(tile: &SquareTile, spec: ViewSpec, item: &MediaItem) {
    tile.set_target(spec.pixel_size);
    let is_day = spec.mode == GroupBy::Day;
    tile.set_motion_badge_visible(is_day && item.is_motion_photo());
    if is_day && item.is_video() {
        let duration = item
            .video_duration_secs
            .and_then(format_tile_duration)
            .unwrap_or_else(|| "--:--".to_string());
        tile.set_video_duration(Some(&duration));
    } else {
        tile.set_video_duration(None);
    }
    tile.set_favorite_badge_visible(is_day && item.is_favorite);
    let item_mtime = thumbnail_request_mtime(item);
    tile.set_cache_key(ThumbnailLoader::cache_key_for(
        &item.uri,
        spec.thumb_size,
        Some(item_mtime),
    ));
    tile.set_thumbnail_request(Rc::new(|| {}));
}

pub(super) fn sync_flow_child_visibility_for_tile(
    tile: &SquareTile,
    flow_child: &gtk::FlowBoxChild,
) {
    flow_child.set_opacity(if tile.has_css_class("thumb-loading") {
        0.0
    } else {
        1.0
    });
}

pub(super) fn build_photo_picture(
    spec: ViewSpec,
    item: MediaItem,
    media_list: gio::ListStore,
    global_index: u32,
    loader: Arc<ThumbnailLoader>,
    on_background_changed: Rc<dyn Fn()>,
) -> SquareTile {
    let tile = SquareTile::new();
    tile.set_target(spec.pixel_size);
    let is_day = spec.mode == GroupBy::Day;
    tile.set_motion_badge_visible(is_day && item.is_motion_photo());
    if is_day && item.is_video() {
        let duration = item
            .video_duration_secs
            .and_then(format_tile_duration)
            .unwrap_or_else(|| "--:--".to_string());
        tile.set_video_duration(Some(&duration));
    }
    tile.set_favorite_badge_visible(is_day && item.is_favorite);

    let fallback_item = item.clone();
    let size = spec.thumb_size;
    let target_px = spec.pixel_size;
    let item_mtime = thumbnail_request_mtime(&item);
    let initial_cache_key = ThumbnailLoader::cache_key_for(&item.uri, size, Some(item_mtime));
    tracing::debug!(
        target: crate::core::log_targets::BROWSING,
        "THUMB_TILE_TRACE tile_create item_id={} item_name={} uri={} mode={:?} global_index={} size={:?} target_px={} cache_key={:?}",
        item.id,
        item.display_name(),
        item.uri,
        spec.mode,
        global_index,
        size,
        target_px,
        initial_cache_key
    );
    tile.set_cache_key(initial_cache_key);
    if let Some(loaded) = loader.try_load_mem_cached(&item.uri, size, Some(item_mtime)) {
        if let Some(is_light) = loaded.is_light {
            tile.set_background_is_light(is_light);
        }
        tile.set_paintable(Some(&loaded.texture));
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "THUMB_TILE_TRACE tile_cached_paintable item_id={} uri={} mode={:?} global_index={} texture={}x{}",
            item.id,
            item.uri,
            spec.mode,
            global_index,
            loaded.texture.width(),
            loaded.texture.height()
        );
    } else {
        tile.add_css_class("thumb-loading");
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "THUMB_TILE_TRACE tile_loading_class_added item_id={} uri={} mode={:?} global_index={} opacity=0",
            item.id,
            item.uri,
            spec.mode,
            global_index
        );
    }

    let request_once: Rc<dyn Fn()> = Rc::new({
        let loader = loader.clone();
        let on_background_changed = on_background_changed.clone();
        let tile_weak = tile.downgrade();
        let requested = std::cell::Cell::new(false);
        move || {
            if requested.get() {
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "THUMB_TILE_TRACE request_ignored_already_requested global_index={} size={:?}",
                    global_index,
                    size
                );
                return;
            }
            requested.set(true);
            let request_started = Instant::now();

            let current_item = crate::ui::media_list::media_item_at(&media_list, global_index)
                .unwrap_or_else(|| fallback_item.clone());
            let item_name = current_item.display_name().to_string();
            let item_uri = current_item.uri.clone();
            let item_mtime = thumbnail_request_mtime(&current_item);
            let cache_key = ThumbnailLoader::cache_key_for(&item_uri, size, Some(item_mtime));
            if let Some(tile) = tile_weak.upgrade() {
                tile.set_cache_key(cache_key.clone());
            }

            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "THUMB_TILE_TRACE request_begin item_id={} item_name={} uri={} size={:?} global_index={} cache_key={:?} loading_class_present={}",
                current_item.id,
                item_name,
                item_uri,
                size,
                global_index,
                cache_key,
                tile_weak
                    .upgrade()
                    .is_some_and(|tile| tile.has_css_class("thumb-loading"))
            );
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "THUMB grid_request item_id={} item_name={} uri={} size={:?} target_px={} global_index={} queue_len={} in_flight={} media_item_mtime={} request_mtime={:?} cache_key={:?}",
                current_item.id,
                item_name,
                item_uri,
                size,
                target_px,
                global_index,
                loader.queue_len(),
                loader.in_flight_len(),
                current_item.file_mtime,
                item_mtime,
                cache_key
            );

            let (tx, rx) = tokio::sync::oneshot::channel();
            loader.request_for_media(
                current_item.id,
                item_uri.clone(),
                size,
                Some(item_mtime),
                tx,
                crate::core::thumbnails::TIER_BOOST,
            );
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "THUMB_TILE_TRACE request_submitted item_id={} uri={} size={:?} global_index={} queue_len={} in_flight={}",
                current_item.id,
                item_uri,
                size,
                global_index,
                loader.queue_len(),
                loader.in_flight_len()
            );
            let tile_weak = tile_weak.clone();
            let on_background_changed = on_background_changed.clone();
            let item_name = item_name.clone();
            let item_uri = item_uri.clone();
            let thumb_span = tracing::debug_span!("grid:thumb_request");
            glib::spawn_future_local(async move {
                let result = rx.await;
                let _thumb = thumb_span.enter();
                match result {
                    Ok(loaded) => {
                        let elapsed_ms = request_started.elapsed().as_millis();
                        tracing::debug!(
                            target: crate::core::log_targets::BROWSING,
                            "THUMB grid_loaded item_name={} uri={} texture={}x{}",
                            item_name,
                            item_uri,
                            loaded.texture.width(),
                            loaded.texture.height()
                        );
                        tracing::debug!(
                            target: crate::core::log_targets::BROWSING,
                            "THUMB_TILE_TRACE request_loaded item_name={} uri={} texture={}x{} elapsed_ms={}",
                            item_name,
                            item_uri,
                            loaded.texture.width(),
                            loaded.texture.height(),
                            elapsed_ms
                        );
                        if let Some(t) = tile_weak.upgrade() {
                            if let Some(is_light) = loaded.is_light {
                                t.set_background_is_light(is_light);
                                on_background_changed();
                            }
                            tracing::debug!(
                                target: crate::core::log_targets::BROWSING,
                                "THUMB_TILE_TRACE set_paintable_texture item_name={} uri={} loading_class_before={}",
                                item_name,
                                item_uri,
                                t.has_css_class("thumb-loading")
                            );
                            t.set_paintable(Some(&loaded.texture));
                        }
                    }
                    Err(_) => {
                        let elapsed_ms = request_started.elapsed().as_millis();
                        tracing::debug!(
                            target: crate::core::log_targets::BROWSING,
                            "THUMB grid_dropped_placeholder item_name={} uri={}",
                            item_name,
                            item_uri
                        );
                        tracing::debug!(
                            target: crate::core::log_targets::BROWSING,
                            "THUMB_TILE_TRACE request_failed_placeholder item_name={} uri={} elapsed_ms={}",
                            item_name,
                            item_uri,
                            elapsed_ms
                        );
                        if let Some(t) = tile_weak.upgrade() {
                            tracing::debug!(
                                target: crate::core::log_targets::BROWSING,
                                "THUMB_TILE_TRACE set_paintable_placeholder item_name={} uri={} loading_class_before={}",
                                item_name,
                                item_uri,
                                t.has_css_class("thumb-loading")
                            );
                            t.set_paintable(Some(&gray_placeholder_texture()));
                        }
                    }
                }
            });
        }
    });

    tile.set_thumbnail_request(request_once);
    tile
}
