//! Reusable `GtkSignalListItemFactory` for virtual grid slots.

use super::model::GridSlotState;
use super::{TileBinding, VirtualMediaGrid};
use crate::core::identity::MediaId;
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::media_grid::{format_tile_duration, thumbnail_request_mtime};
use crate::ui::square_tile::SquareTile;
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Per-list-item objects created once in `setup` and reconfigured in `bind`.
#[derive(Clone)]
pub(super) struct FactoryCell {
    pub tile: SquareTile,
    pub binding: Rc<RefCell<Option<TileBinding>>>,
    thumbnail_request: Rc<RefCell<Option<PendingThumbnailRequest>>>,
}

impl FactoryCell {
    pub(super) fn new(tile: SquareTile, binding: Rc<RefCell<Option<TileBinding>>>) -> Self {
        Self {
            tile,
            binding,
            thumbnail_request: Rc::new(RefCell::new(None)),
        }
    }
}

#[derive(Clone)]
struct PendingThumbnailRequest {
    cache_key: Option<String>,
    cancellation: Arc<AtomicBool>,
    loader: Arc<ThumbnailLoader>,
}

pub(super) fn install(grid: &VirtualMediaGrid) {
    let factory = gtk::SignalListItemFactory::new();

    {
        let weak = grid.downgrade();
        factory.connect_setup(move |_, object| {
            let Ok(list_item) = object.clone().downcast::<gtk::ListItem>() else {
                return;
            };
            let Some(grid) = weak.upgrade() else {
                return;
            };
            let tile = SquareTile::new();
            tile.set_height_for_width(true);
            // GridView columns expand to fill a viewport. The tile must fill
            // that allocated cell as well; centering a fixed-width square
            // leaves a large blank gutter whenever the window is resized.
            // The scroller/CSS box model can make that final cell a few
            // pixels narrower than its preferred target, so do not report
            // the target as a hard minimum during GTK's measure pass.
            tile.set_allow_width_shrink(true);
            tile.set_halign(gtk::Align::Fill);
            tile.set_hexpand(true);
            tile.set_valign(gtk::Align::Fill);
            tile.set_vexpand(true);
            // The custom context-menu layer must return focus to the tile
            // that opened it before it disappears. Without a focusable tile,
            // GTK falls back to GridView's first item and visibly scrolls the
            // viewport to the top for one frame.
            tile.set_can_focus(true);
            let binding = Rc::new(RefCell::new(None));
            let binding_for_context = binding.clone();
            let weak_for_context = grid.downgrade();
            let tile_for_context = tile.downgrade();
            let gesture = gtk::GestureClick::new();
            gesture.set_button(3);
            gesture.connect_released(move |gesture, _, x, y| {
                if gesture.current_button() != 3 {
                    return;
                }
                let Some(binding) = binding_for_context.borrow().clone() else {
                    return;
                };
                if let (Some(grid), Some(tile)) =
                    (weak_for_context.upgrade(), tile_for_context.upgrade())
                {
                    grid.show_context_menu(tile.upcast_ref(), &binding, x, y);
                }
            });
            tile.add_controller(gesture);

            list_item.set_child(Some(&tile));
            grid.register_factory_cell(FactoryCell::new(tile, binding));
        });
    }

    {
        let weak = grid.downgrade();
        factory.connect_bind(move |_, object| {
            let Ok(list_item) = object.clone().downcast::<gtk::ListItem>() else {
                return;
            };
            let Some(grid) = weak.upgrade() else {
                return;
            };
            let Some(tile) = list_item
                .child()
                .and_then(|child| child.downcast::<SquareTile>().ok())
            else {
                return;
            };
            let Some(cell) = grid.factory_cell_for(&tile) else {
                return;
            };
            let Some(slot) = list_item
                .item()
                .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
                .map(|boxed| boxed.borrow::<GridSlotState>().clone())
            else {
                return;
            };
            bind_cell(&grid, &list_item, &cell, slot);
        });
    }

    {
        let weak = grid.downgrade();
        factory.connect_unbind(move |_, object| {
            let Ok(list_item) = object.clone().downcast::<gtk::ListItem>() else {
                return;
            };
            let Some(grid) = weak.upgrade() else {
                return;
            };
            let Some(tile) = list_item
                .child()
                .and_then(|child| child.downcast::<SquareTile>().ok())
            else {
                return;
            };
            let Some(cell) = grid.factory_cell_for(&tile) else {
                return;
            };
            cancel_thumbnail_request(&cell);
            *cell.binding.borrow_mut() = None;
            cell.tile.clear_for_rebind();
            list_item.set_activatable(false);
            list_item.set_selectable(false);
        });
    }

    {
        let weak = grid.downgrade();
        factory.connect_teardown(move |_, object| {
            let Ok(list_item) = object.clone().downcast::<gtk::ListItem>() else {
                return;
            };
            let grid = weak.upgrade();
            teardown_list_item(grid.as_ref(), &list_item);
        });
    }

    grid.set_list_factory(&factory);
}

/// Reverse the permanent child setup when GTK retires a list item.
///
/// GtkGridView may retire items while replacing the initial provisional layout
/// with authoritative metadata. This must remain valid even when the grid was
/// disposed first, because the ListItem still owns its setup child until this
/// callback clears it.
fn teardown_list_item(grid: Option<&VirtualMediaGrid>, list_item: &gtk::ListItem) {
    let tile = list_item
        .child()
        .and_then(|child| child.downcast::<SquareTile>().ok());

    if let (Some(grid), Some(tile)) = (grid, tile.as_ref()) {
        if let Some(cell) = grid.factory_cell_for(tile) {
            cancel_thumbnail_request(&cell);
            *cell.binding.borrow_mut() = None;
            cell.tile.clear_for_rebind();
        }
        grid.remove_factory_cell(tile);
    }

    list_item.set_activatable(false);
    list_item.set_selectable(false);
    list_item.set_child(None::<&gtk::Widget>);
}

fn bind_cell(
    grid: &VirtualMediaGrid,
    list_item: &gtk::ListItem,
    cell: &FactoryCell,
    slot: GridSlotState,
) {
    cancel_thumbnail_request(cell);
    cell.tile.clear_for_rebind();
    *cell.binding.borrow_mut() = None;
    cell.tile.set_target(grid.spec().tile_size());

    match slot {
        GridSlotState::Filler { .. } => {
            cell.tile.set_opacity(0.0);
            cell.tile.set_can_target(false);
            list_item.set_activatable(false);
            list_item.set_selectable(false);
        }
        GridSlotState::Placeholder { .. } => {
            cell.tile.show_loading_placeholder();
            cell.tile.set_can_target(false);
            list_item.set_activatable(false);
            list_item.set_selectable(false);
        }
        GridSlotState::Ready {
            slot,
            media_offset: _,
            item,
        } => bind_ready_cell(grid, list_item, cell, slot, *item),
    }
}

fn bind_ready_cell(
    grid: &VirtualMediaGrid,
    list_item: &gtk::ListItem,
    cell: &FactoryCell,
    slot: u32,
    item: crate::core::media::MediaItem,
) {
    let spec = grid.spec();
    let loader = grid.loader();
    let item_mtime = thumbnail_request_mtime(&item);
    let thumbnail_uri = grid.thumbnail_uri_for(&item);
    let cache_key =
        ThumbnailLoader::cache_key_for(&thumbnail_uri, spec.thumbnail_size(), Some(item_mtime));
    let binding = TileBinding::new(
        grid.layout_generation(),
        slot,
        MediaId::from(item.id),
        cache_key.clone(),
    );
    *cell.binding.borrow_mut() = Some(binding.clone());
    cell.tile.set_cache_key(cache_key.clone());
    let cancellation = Arc::new(AtomicBool::new(false));
    *cell.thumbnail_request.borrow_mut() = Some(PendingThumbnailRequest {
        cache_key: cache_key.clone(),
        cancellation: cancellation.clone(),
        loader: loader.clone(),
    });
    cell.tile.set_motion_badge_visible(
        spec.mode() == crate::core::section_model::GroupBy::Day && item.is_motion_photo(),
    );
    if spec.mode() == crate::core::section_model::GroupBy::Day && item.is_video() {
        cell.tile.set_video_duration(
            item.video_duration_secs
                .and_then(format_tile_duration)
                .as_deref(),
        );
    }
    cell.tile.set_favorite_badge_visible(
        spec.mode() == crate::core::section_model::GroupBy::Day && item.is_favorite,
    );
    if grid.is_selected(MediaId::from(item.id)) {
        cell.tile.add_css_class("media-selected");
    }
    cell.tile.set_can_target(true);
    list_item.set_activatable(true);
    list_item.set_selectable(false);

    let load_started = std::time::Instant::now();
    if let Some(loaded) =
        loader.try_load_mem_cached(&thumbnail_uri, spec.thumbnail_size(), Some(item_mtime))
    {
        // A cache hit is painted on the next idle turn just like an async
        // result. Keep the visible skeleton in the intervening frame instead
        // of briefly showing an empty recycled card.
        cell.tile.show_loading_placeholder();
        defer_thumbnail_paint(
            cell.tile.downgrade(),
            grid.downgrade(),
            cell.binding.clone(),
            binding,
            loaded,
            load_started,
            true,
            false,
        );
        return;
    }

    cell.tile.show_loading_placeholder();
    // Instant low-res placeholder from the embedded EXIF thumbnail, if the
    // landing preload (or a prior pass) put one in mem. Painted via the same
    // guarded path as a full thumb; `defer_thumbnail_paint` skips it if a full
    // thumbnail already arrived. The full-thumb request below still fires and
    // upgrades the preview.
    if let Some(exif) = loader.try_load_exif_thumb_cached(&thumbnail_uri, Some(item_mtime)) {
        defer_thumbnail_paint(
            cell.tile.downgrade(),
            grid.downgrade(),
            cell.binding.clone(),
            binding.clone(),
            exif,
            load_started,
            true,
            true,
        );
    }
    // Throttle the request through the grid's batcher so a ~100-tile landing
    // does not enqueue ~100 requests in one frame (the p99 queue_wait tail).
    let tile = cell.tile.clone();
    let binding_state = cell.binding.clone();
    let grid_weak = grid.downgrade();
    grid.enqueue_thumbnail(move || {
        let Some(grid) = grid_weak.upgrade() else {
            return;
        };
        request_thumbnail(
            &grid,
            tile,
            binding_state,
            binding,
            crate::core::media::MediaItem {
                uri: thumbnail_uri,
                ..item
            },
            loader,
            load_started,
            cancellation,
        );
    });
}

/// Apply a thumbnail outside `GtkSignalListItemFactory::bind`.
///
/// GTK 4.22 updates a GridView's internal accessibility tree while it emits
/// `bind`. Changing CSS classes on the tile in that same call stack can
/// re-enter that bookkeeping. An idle callback also naturally coalesces a
/// burst of in-memory cache hits, while the binding comparison prevents a
/// recycled cell from receiving an old texture.
pub(super) fn defer_thumbnail_paint(
    tile_weak: glib::WeakRef<SquareTile>,
    grid_weak: glib::WeakRef<VirtualMediaGrid>,
    binding_state: Rc<RefCell<Option<TileBinding>>>,
    binding: TileBinding,
    loaded: crate::core::thumbnails::LoadedThumb,
    load_started: std::time::Instant,
    mem_hit: bool,
    is_placeholder: bool,
) {
    glib::idle_add_local_once(move || {
        if !binding_is_current(&binding_state, &binding) {
            return;
        }
        let Some(tile) = tile_weak.upgrade() else {
            return;
        };
        // A low-res EXIF placeholder must never overwrite a full thumbnail that
        // already arrived; full thumbnails always win (and mark the flag).
        if is_placeholder && tile.full_thumbnail_painted() {
            return;
        }
        if let Some(is_light) = loaded.is_light {
            tile.set_background_is_light(is_light);
            if let Some(grid) = grid_weak.upgrade() {
                grid.notify_background_changed();
            }
        }
        tile.set_paintable(Some(&loaded.texture));
        if !is_placeholder {
            tile.mark_full_thumbnail_painted();
        }
        // 端到端 bind→paint 延迟（主线程墙钟；worker 解码在 request 与 rx 之间于
        // 另一线程完成）。用 span 承载点测量：稳定名 "tile:paint"，elapsed_ms 在
        // 构造时（=上屏时刻）求值。mem_hit=true 表示 bind 时即命中 mem_cache（仅
        // GTK idle 延迟）；false 走 request→queue→decode→rx→idle 全程。按 media_id
        // 与 thumb:process（queue_wait_ms / cache_hit）交叉即可拆成 排队 + 解码 +
        // GTK 上屏 三段。不带 target → 默认模块路径（photo_viewer::…）被
        // photo_viewer=debug 放行（显式 "ui::…" target 会被过滤掉）。
        let _tile_paint = tracing::debug_span!(
            "tile:paint",
            media_id = binding.media_id().get(),
            mem_hit,
            elapsed_ms = load_started.elapsed().as_millis() as u64,
        )
        .entered();
    });
}

fn request_thumbnail(
    grid: &VirtualMediaGrid,
    tile: SquareTile,
    binding_state: Rc<RefCell<Option<TileBinding>>>,
    binding: TileBinding,
    item: crate::core::media::MediaItem,
    loader: Arc<ThumbnailLoader>,
    load_started: std::time::Instant,
    cancellation: Arc<AtomicBool>,
) {
    // A rapid direction change can recycle the tile while its request is still
    // waiting in the per-frame batcher. Do not let an obsolete closure consume
    // a thumbnail worker: its result would be rejected at paint time anyway,
    // while the newly visible reverse-scroll tiles wait behind it.
    if cancellation.load(Ordering::Acquire) || !binding_is_current(&binding_state, &binding) {
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            media_id = binding.media_id().get(),
            "VGRID_THUMBNAIL_STALE_REQUEST_DROPPED"
        );
        return;
    }
    let spec = grid.spec();
    let mtime = thumbnail_request_mtime(&item);
    let (tx, rx) = tokio::sync::oneshot::channel();
    loader.request_for_media_cancellable(
        item.id,
        item.uri,
        spec.thumbnail_size(),
        Some(mtime),
        tx,
        crate::core::thumbnails::TIER_BOOST,
        cancellation,
    );

    let tile_weak = tile.downgrade();
    let grid_weak = grid.downgrade();
    glib::spawn_future_local(async move {
        let Ok(loaded) = rx.await else {
            return;
        };
        defer_thumbnail_paint(
            tile_weak,
            grid_weak,
            binding_state,
            binding,
            loaded,
            load_started,
            false,
            false,
        );
    });
}

/// Drop a recycled cell's interest before its old thumbnail consumes a worker.
/// `ThumbnailLoader` only removes the queued job when no other active cell is
/// waiting on the same cache key, so deduplicated requests remain correct.
fn cancel_thumbnail_request(cell: &FactoryCell) {
    let Some(request) = cell.thumbnail_request.borrow_mut().take() else {
        return;
    };
    request.cancellation.store(true, Ordering::Release);
    if let Some(cache_key) = request.cache_key.as_deref() {
        request
            .loader
            .cancel_cancellable_request(cache_key, &request.cancellation);
    }
}

fn binding_is_current(
    binding_state: &Rc<RefCell<Option<TileBinding>>>,
    binding: &TileBinding,
) -> bool {
    binding_state
        .borrow()
        .as_ref()
        .is_some_and(|current| current == binding)
}

#[cfg(test)]
mod tests;
