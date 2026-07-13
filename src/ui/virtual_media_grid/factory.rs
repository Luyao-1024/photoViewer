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
            // GridView columns expand to fill a viewport. Keep the reusable
            // card at the mode's fixed natural square instead of stretching
            // its texture horizontally inside that allocated column.
            tile.set_halign(gtk::Align::Center);
            tile.set_valign(gtk::Align::Start);
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
            let Some(grid) = weak.upgrade() else {
                return;
            };
            if let Some(tile) = list_item
                .child()
                .and_then(|child| child.downcast::<SquareTile>().ok())
            {
                grid.remove_factory_cell(&tile);
            }
        });
    }

    grid.set_list_factory(&factory);
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
        if let Some(is_light) = loaded.is_light {
            cell.tile.set_background_is_light(is_light);
            grid.notify_background_changed();
        }
        cell.tile.set_paintable(Some(&loaded.texture));
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

#[cfg(test)]
mod tests;
