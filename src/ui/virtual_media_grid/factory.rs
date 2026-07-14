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
use std::sync::Arc;

/// Per-list-item objects created once in `setup` and reconfigured in `bind`.
#[derive(Clone)]
pub(super) struct FactoryCell {
    pub tile: SquareTile,
    pub binding: Rc<RefCell<Option<TileBinding>>>,
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
            grid.register_factory_cell(FactoryCell { tile, binding });
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
    let item_mtime = thumbnail_request_mtime(&item);
    let cache_key =
        ThumbnailLoader::cache_key_for(&item.uri, spec.thumbnail_size(), Some(item_mtime));
    let binding = TileBinding::new(
        grid.layout_generation(),
        slot,
        MediaId::from(item.id),
        cache_key.clone(),
    );
    *cell.binding.borrow_mut() = Some(binding.clone());
    cell.tile.set_cache_key(cache_key);
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

    let loader = grid.loader();
    if let Some(loaded) =
        loader.try_load_mem_cached(&item.uri, spec.thumbnail_size(), Some(item_mtime))
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
        );
        return;
    }

    cell.tile.show_loading_placeholder();
    request_thumbnail(
        grid,
        cell.tile.clone(),
        cell.binding.clone(),
        binding,
        item,
        loader,
    );
}

/// Apply a thumbnail outside `GtkSignalListItemFactory::bind`.
///
/// GTK 4.22 updates a GridView's internal accessibility tree while it emits
/// `bind`. Changing CSS classes on the tile in that same call stack can
/// re-enter that bookkeeping. An idle callback also naturally coalesces a
/// burst of in-memory cache hits, while the binding comparison prevents a
/// recycled cell from receiving an old texture.
fn defer_thumbnail_paint(
    tile_weak: glib::WeakRef<SquareTile>,
    grid_weak: glib::WeakRef<VirtualMediaGrid>,
    binding_state: Rc<RefCell<Option<TileBinding>>>,
    binding: TileBinding,
    loaded: crate::core::thumbnails::LoadedThumb,
) {
    glib::idle_add_local_once(move || {
        let current_matches = binding_state
            .borrow()
            .as_ref()
            .is_some_and(|current| current == &binding);
        if !current_matches {
            return;
        }
        let Some(tile) = tile_weak.upgrade() else {
            return;
        };
        if let Some(is_light) = loaded.is_light {
            tile.set_background_is_light(is_light);
            if let Some(grid) = grid_weak.upgrade() {
                grid.notify_background_changed();
            }
        }
        tile.set_paintable(Some(&loaded.texture));
    });
}

fn request_thumbnail(
    grid: &VirtualMediaGrid,
    tile: SquareTile,
    binding_state: Rc<RefCell<Option<TileBinding>>>,
    binding: TileBinding,
    item: crate::core::media::MediaItem,
    loader: Arc<ThumbnailLoader>,
) {
    let spec = grid.spec();
    let mtime = thumbnail_request_mtime(&item);
    let (tx, rx) = tokio::sync::oneshot::channel();
    loader.request_for_media(
        item.id,
        item.uri,
        spec.thumbnail_size(),
        Some(mtime),
        tx,
        crate::core::thumbnails::TIER_BOOST,
    );

    let tile_weak = tile.downgrade();
    let grid_weak = grid.downgrade();
    glib::spawn_future_local(async move {
        let Ok(loaded) = rx.await else {
            return;
        };
        defer_thumbnail_paint(tile_weak, grid_weak, binding_state, binding, loaded);
    });
}

#[cfg(test)]
mod tests;
