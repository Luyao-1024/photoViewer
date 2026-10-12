//! Full FlowBox rebuild: window layout, grouped tiles and section input wiring.

use super::render::{
    build_photo_picture, prepare_reused_tile, sync_flow_child_visibility_for_tile,
};
use super::virtual_paging::{
    build_virtual_placeholder_flow, estimated_virtual_columns, virtual_spacer,
    virtual_spacer_height, virtual_window_item_count,
};
use super::{
    build_library_stats_label, extract_items, runtime_config, should_show_library_stats,
    spec_for_mode, uri_index_map, ActivateCallback, CoverCallback, DisplayedItem, FavoriteCallback,
    FavoriteStateCallback, MediaGrid, SelectionCallback, SquareTile, ViewSpec,
};
use crate::core::i18n::tr;
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::section_model::{
    apply_authoritative_counts, group_items, GroupBy, MediaSection, SectionKey,
};
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::glass_context_menu::{self, GlassMenuItem, GlassMenuItemKind};
use gtk4 as gtk;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use gtk4::{gio, prelude::*};
use libadwaita as adw;
use libadwaita::prelude::{AdwDialogExt, AlertDialogExt};
use std::collections::HashMap;
use std::sync::Arc;

impl MediaGrid {
    /// Tear down the current sections and rebuild them for `mode`.
    #[tracing::instrument(name = "grid:rebuild", skip(self, media_list), fields(source_len = media_list.n_items()))]
    pub(super) fn rebuild(&self, media_list: gtk::gio::ListStore, mode: GroupBy) {
        let source_len = media_list.n_items();
        let progressive_just_seeded = self.seed_progressive_rebuild(source_len, mode);
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "GRID_MODEL_TRACE rebuild_start mode={:?} source_len={} active={} dirty={} rendered_limit={}",
            mode,
            source_len,
            self.imp().active.get(),
            self.imp().dirty_model.get(),
            self.imp().rendered_limit.get()
        );
        let loader = self
            .imp()
            .loader
            .get()
            .expect("MediaGrid::rebuild called before new()")
            .clone();
        let on_activate = self
            .imp()
            .on_activate
            .get()
            .expect("MediaGrid::rebuild called before new()")
            .clone();
        let on_add_to_album = self
            .imp()
            .on_add_to_album
            .get()
            .expect("MediaGrid::rebuild called before new()")
            .clone();
        let on_move_to_trash = self
            .imp()
            .on_move_to_trash
            .get()
            .expect("MediaGrid::rebuild called before new()")
            .clone();
        let on_set_favorite = self
            .imp()
            .on_set_favorite
            .get()
            .expect("MediaGrid::rebuild called before new()")
            .clone();
        let on_query_favorite_state = self
            .imp()
            .on_query_favorite_state
            .get()
            .expect("MediaGrid::rebuild called before new()")
            .clone();
        let on_set_album_cover = self
            .imp()
            .on_set_album_cover
            .get()
            .expect("MediaGrid::rebuild called before new()")
            .clone();
        let enable_context_menu = self.imp().enable_context_menu.get();

        let spec = spec_for_mode(mode);

        // 重建前保存滚动位置，避免因清空/重建子 widget 导致 adjustment.value 归零。
        let saved_scroll = self.imp().scroller.get().vadjustment().value();

        let content = self.imp().content.get();
        if let Some(source) = self.imp().stats_refresh_source.borrow_mut().take() {
            source.remove();
        }
        self.imp().stats_label.borrow_mut().take();
        let mut reusable_tiles = self.detach_reusable_loaded_tiles();
        {
            let clear_span = tracing::info_span!(
                target: crate::core::log_targets::BROWSING,
                "grid:clear_content",
                mode = ?mode
            );
            let _clear = clear_span.enter();
            // Clear any previously built sections.
            while let Some(child) = content.first_child() {
                content.remove(&child);
            }
        }
        self.imp().displayed_items.borrow_mut().clear();

        // Extract MediaItems + a uri→global-index lookup from the store.
        let mut items = {
            let extract_span = tracing::info_span!("grid:extract_items");
            let _extract = extract_span.enter();
            extract_items(&media_list)
        };
        let max_items = self.imp().rendered_limit.get();
        if items.len() > max_items {
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "MediaGrid::rebuild limiting rendered items mode={:?} total={} rendered={}",
                mode,
                items.len(),
                max_items
            );
            items.truncate(max_items);
        }
        let rendered_item_count = items.len() as u32;
        let uri_to_index = uri_index_map(&media_list);
        let total_media_count = if self.imp().full_library_context.get() {
            self.imp()
                .library_total_snapshot
                .get()
                .unwrap_or(source_len)
                .max(source_len)
        } else {
            source_len
        };
        self.imp().virtual_total.set(total_media_count);
        self.ensure_library_metadata_async(loader.clone(), mode);
        let is_loading_virtual_window = self.imp().virtual_page_loading.get();
        let loading_placeholder_count = if is_loading_virtual_window {
            virtual_window_item_count(
                self.imp().virtual_window_start.get(),
                total_media_count,
                runtime_config::virtual_media_page_size(),
            )
        } else {
            0
        };
        let effective_window_len = if loading_placeholder_count > 0 {
            loading_placeholder_count
        } else {
            items.len() as u32
        };
        let window_start = self
            .imp()
            .virtual_window_start
            .get()
            .min(total_media_count.saturating_sub(effective_window_len));
        self.imp().virtual_window_start.set(window_start);
        let viewport_width = self.imp().scroller.get().allocated_width().max(1) as f64;
        let virtual_columns = estimated_virtual_columns(viewport_width.max(1000.0), spec);
        let top_spacer_height =
            virtual_spacer_height(window_start, virtual_columns, viewport_width, spec);
        if top_spacer_height > 0 {
            content.append(&virtual_spacer(top_spacer_height));
        }

        let total_media = total_media_count as usize;
        if self.imp().full_library_context.get()
            && mode == GroupBy::Day
            && (loading_placeholder_count > 0 || !items.is_empty())
        {
            if let Some(stats) = self.imp().library_stats_snapshot.get() {
                if should_show_library_stats(&stats) {
                    let stats_label = build_library_stats_label(stats);
                    content.append(&stats_label);
                    *self.imp().stats_label.borrow_mut() = Some(stats_label.clone());
                    self.start_stats_refresh(loader.clone(), total_media);
                }
            }
        }

        // Group by year/month/day, then emit header + FlowBox per section.
        let mut section_count = 0u32;
        let mut photo_count = 0u32;
        let mut displayed_items = Vec::new();
        let flat_sections = self.imp().flat_sections.get();
        {
            let build_sections_span = tracing::info_span!(
                target: crate::core::log_targets::BROWSING,
                "grid:build_sections",
                mode = ?mode,
                flat_sections,
                loading_placeholder_count,
                sections = tracing::field::Empty,
                photos = tracing::field::Empty
            );
            let _build_sections = build_sections_span.enter();
            if loading_placeholder_count > 0 {
                content.append(&build_virtual_placeholder_flow(
                    spec,
                    loading_placeholder_count,
                ));
                section_count = 1;
                photo_count = loading_placeholder_count;
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "VIRTUAL_SCROLL placeholder_window mode={:?} start={} count={} total={}",
                    mode,
                    window_start,
                    loading_placeholder_count,
                    total_media_count
                );
            } else {
                let sections = self.group_rebuild_sections(items, mode, flat_sections);
                for section in sections {
                    if section.items.is_empty() {
                        continue;
                    }

                    if !flat_sections {
                        // Full-width section header.
                        let header = gtk::Label::builder()
                            .label(&section.label)
                            .halign(gtk::Align::Start)
                            .margin_start(12)
                            .margin_top(12)
                            .margin_bottom(6)
                            .xalign(0.0)
                            .css_classes(["heading"])
                            .build();
                        content.append(&header);
                    }

                    // FlowBox of square thumbnails. `homogeneous` makes every cell the
                    // same size; with each picture's `set_size_request(target)` the
                    // cells become target×target squares. `column/row spacing` is the
                    // thin separator (≤3px); hover styling lives in grid_css.
                    //
                    // `selection_mode` starts at `None` and is flipped to
                    // `Multiple` only while multi-select is active (see
                    // `apply_selection_mode`). `None` stops the FlowBox from
                    // tracking its own selection, so no child can become
                    // `:selected` — and reveal the `.thumb-checkmark` — outside
                    // explicit multi-select. We mirror selection into
                    // `imp.selected` for our own bookkeeping; the focus ring
                    // (driven by `:hover` / `:focus` in grid_css) is unchanged.
                    let flow = gtk::FlowBox::builder()
                        .orientation(gtk::Orientation::Horizontal)
                        .homogeneous(true)
                        .column_spacing(8)
                        .row_spacing(8)
                        .max_children_per_line(100)
                        .selection_mode(gtk::SelectionMode::None)
                        .build();
                    flow.set_activate_on_single_click(true);
                    flow.add_css_class("thumb-grid");
                    // While arrow-keying between tiles, hide the `:hover` hint so the
                    // highlight follows the keyboard focus, not the resting pointer.
                    crate::ui::grid_css::attach_kbd_nav(&flow);

                    photo_count += self.append_rebuilt_section_tiles(
                        &flow,
                        &section,
                        spec,
                        &media_list,
                        &loader,
                        &uri_to_index,
                        &mut reusable_tiles,
                        &mut displayed_items,
                    );

                    self.connect_rebuilt_section_activation(&flow, &section.label, &on_activate);

                    if enable_context_menu {
                        self.connect_rebuilt_section_context_menu(
                            &flow,
                            &on_add_to_album,
                            &on_move_to_trash,
                            &on_set_favorite,
                            &on_query_favorite_state,
                            &on_set_album_cover,
                        );
                    }

                    content.append(&flow);
                    section_count += 1;
                }
            }
            build_sections_span.record("sections", section_count);
            build_sections_span.record("photos", photo_count);
        }
        let rendered_window_len = if loading_placeholder_count > 0 {
            loading_placeholder_count
        } else {
            rendered_item_count
        };
        let loaded_end = window_start.saturating_add(rendered_window_len);
        let bottom_unloaded = total_media_count.saturating_sub(loaded_end);
        let bottom_spacer_height =
            virtual_spacer_height(bottom_unloaded, virtual_columns, viewport_width, spec);
        if bottom_spacer_height > 0 {
            content.append(&virtual_spacer(bottom_spacer_height));
        }
        *self.imp().displayed_items.borrow_mut() = displayed_items;
        self.sync_visible_selection();

        self.restore_rebuilt_scroll(saved_scroll);

        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "MediaGrid::rebuild mode={:?} source_len={} sections={} photos={} spec.pixel_size={}",
            mode,
            source_len,
            section_count,
            photo_count,
            spec.pixel_size
        );
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "GRID_MODEL_TRACE rebuild_finish mode={:?} source_len={} sections={} rendered_photos={} pixel_size={}",
            mode,
            source_len,
            section_count,
            photo_count,
            spec.pixel_size
        );

        // 下一帧 layout 完成后立即请求/提权视口附近缩略图；滚动期间仍走去抖路径。
        let weak = self.downgrade();
        gtk::glib::idle_add_local_once(move || {
            if let Some(this) = weak.upgrade() {
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "GRID_MODEL_TRACE idle_reprioritize_after_rebuild mode={:?}",
                    this.mode()
                );
                this.reprioritize_visible();
            }
        });

        // The first rebuild just built the viewport seed; pace the rest of the
        // first page in background ticks so the window is interactive long
        // before all tiles exist.
        if progressive_just_seeded {
            self.schedule_progressive_render_fill();
        }
    }

    fn seed_progressive_rebuild(&self, source_len: u32, mode: GroupBy) -> bool {
        // Progressive first render: on this grid's FIRST rebuild, cap
        // rendered_limit at a viewport-sized seed so the first paint builds only
        // ~seed tiles. The same runtime plan also controls the model-window cap
        // used by album loading.
        let mut progressive_just_seeded = false;
        if self.imp().progressive_render_pending.replace(false) {
            let steady_limit = self.imp().rendered_limit.get();
            let plan = runtime_config::progressive_render_plan(
                runtime_config::startup_progressive_render(),
                runtime_config::startup_render_seed(),
                source_len as usize,
                steady_limit,
            );
            if plan.progressive {
                self.imp().rendered_limit.set(plan.first_render_limit);
                progressive_just_seeded = true;
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "PROGRESSIVE_RENDER seed mode={:?} seed={} source_len={} model_limit={}",
                    mode,
                    plan.first_render_limit,
                    source_len,
                    plan.model_limit
                );
            }
        }
        progressive_just_seeded
    }

    fn group_rebuild_sections(
        &self,
        items: Vec<MediaItem>,
        mode: GroupBy,
        flat_sections: bool,
    ) -> Vec<MediaSection> {
        let input_len = items.len();
        let group_span = tracing::info_span!(
            target: crate::core::log_targets::BROWSING,
            "grid:group_sections",
            mode = ?mode,
            flat_sections,
            input_len
        );
        let mut sections = {
            let _group = group_span.enter();
            if flat_sections {
                vec![MediaSection {
                    key: SectionKey {
                        year: None,
                        month: None,
                        day: None,
                    },
                    label: String::new(),
                    items,
                }]
            } else {
                group_items(&items, mode)
            }
        };
        // section 头部计数改用整个库的 DB 聚合，而非当前虚拟分页窗口切片。
        // 窗口受 virtual_media_page_size（默认 500）截断，否则一个实际几千张的
        // 年份只会显示窗口里的 500。窗口只决定渲染哪些缩略图，不影响真实计数。
        if !flat_sections && self.imp().full_library_context.get() {
            if let Some(counts) = self.imp().section_count_snapshots.borrow().get(&mode) {
                apply_authoritative_counts(&mut sections, counts);
            }
        }
        sections
    }

    fn append_rebuilt_section_tiles(
        &self,
        flow: &gtk::FlowBox,
        section: &MediaSection,
        spec: ViewSpec,
        media_list: &gio::ListStore,
        loader: &Arc<ThumbnailLoader>,
        uri_to_index: &HashMap<String, u32>,
        reusable_tiles: &mut HashMap<MediaId, SquareTile>,
        displayed_items: &mut Vec<DisplayedItem>,
    ) -> u32 {
        let mut photo_count = 0;
        // Build tiles and remember each child's stable id/current store
        // index. Activation and context menus read this live mapping so
        // incremental child removals cannot leave stale index arrays.
        for item in &section.items {
            let gi = uri_to_index.get(&item.uri).copied().unwrap_or(u32::MAX);
            let media_id = MediaId::from(item.id);
            let on_bg = self
                .imp()
                .on_background_changed
                .get()
                .expect("MediaGrid::rebuild called before new()")
                .clone();
            // Only reuse a rescued tile if it is fully detached. GTK
            // toggle-ref finalization of the old FlowBoxChild can lag,
            // leaving the tile parented when we re-append it — that
            // trips `gtk_flow_box_child_set_child` and the tile never
            // attaches (blank), which is the fast-scroll "stuck" bug.
            // A still-parented tile falls through to a fresh build.
            let reused = reusable_tiles
                .remove(&media_id)
                .filter(|t| t.parent().is_none());
            let picture = if let Some(tile) = reused {
                prepare_reused_tile(&tile, spec, item);
                tile
            } else {
                build_photo_picture(
                    spec,
                    item.clone(),
                    media_list.clone(),
                    gi,
                    loader.clone(),
                    on_bg,
                    false,
                )
            };
            flow.append(&picture);
            if let Some(flow_child) = flow
                .last_child()
                .and_then(|w| w.downcast::<gtk::FlowBoxChild>().ok())
            {
                sync_flow_child_visibility_for_tile(&picture, &flow_child);
                if gi != u32::MAX {
                    displayed_items.push(DisplayedItem {
                        flow_child: flow_child.clone(),
                        window_index: gi,
                        media_id,
                        section_key: section.key.clone(),
                    });
                }
            }
            photo_count += 1;
        }
        photo_count
    }

    fn connect_rebuilt_section_activation(
        &self,
        flow: &gtk::FlowBox,
        section_label: &str,
        on_activate: &ActivateCallback,
    ) {
        // Activation: FlowBox child-activated → look up stable media id.
        // Only explicit multi-select mode (entered via right-click “Multi-select”)
        // toggles selection; otherwise the item opens in viewer.
        let on_act = on_activate.clone();
        let weak = self.downgrade();
        let section_label_for_activation = section_label.to_string();
        flow.connect_child_activated(move |flow, child| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let idx = child.index();
            let Some(displayed_item) = this.displayed_item_for_child(child) else {
                return;
            };
            let media_id = displayed_item.media_id;
            let gi = displayed_item.window_index;
            let current_item = this.media_item_for_id(media_id);
            let item_id = current_item.as_ref().map(|item| item.id).unwrap_or(-1);
            let item_name = current_item
                .as_ref()
                .map(|item| item.display_name().to_string())
                .unwrap_or_else(|| "(missing)".into());
            let item_uri = current_item
                .as_ref()
                .map(|item| item.uri.clone())
                .unwrap_or_else(|| "(missing)".into());
            let is_multi = this.is_multi_select_mode();
            let displayed_indices = this.displayed_indices();
            let displayed_pos = displayed_indices.iter().position(|index| *index == gi);
            tracing::debug!(
                target: crate::core::log_targets::BROWSING,
                "VIEWER_TRACE grid_activate mode={:?} section={} section_child_index={} global_index={} displayed_pos={:?} displayed_len={} displayed_first={:?} displayed_last={:?} item_id={} item_name={} item_uri={} multi_select={}",
                this.mode(),
                section_label_for_activation,
                idx,
                gi,
                displayed_pos,
                displayed_indices.len(),
                displayed_indices.first(),
                displayed_indices.last(),
                item_id,
                item_name.as_str(),
                item_uri.as_str(),
                is_multi
            );
            if is_multi {
                // `flow` comes from the signal arg, no extra upgrade needed.
                this.toggle_selection(media_id, child, flow);
            } else {
                on_act(media_id);
            }
        });
    }

    fn connect_rebuilt_section_context_menu(
        &self,
        flow: &gtk::FlowBox,
        on_add_to_album: &SelectionCallback,
        on_move_to_trash: &SelectionCallback,
        on_set_favorite: &FavoriteCallback,
        on_query_favorite_state: &FavoriteStateCallback,
        on_set_album_cover: &Option<CoverCallback>,
    ) {
        let weak_for_context = self.downgrade();
        let flow_for_ctx = flow.clone();
        let on_add_to_album_ctx = on_add_to_album.clone();
        let on_move_to_trash_ctx = on_move_to_trash.clone();
        let on_set_favorite_ctx = on_set_favorite.clone();
        let on_query_favorite_state_ctx = on_query_favorite_state.clone();
        let on_set_album_cover_ctx = on_set_album_cover.clone();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(3);
        gesture.set_propagation_phase(gtk::PropagationPhase::Capture);

        gesture.connect_released(move |gesture, _n_press, x, y| {
            if gesture.current_button() != 3 {
                return;
            }
            let Some(this) = weak_for_context.upgrade() else {
                return;
            };

            let Some(flow_child_for_ctx) = flow_for_ctx.child_at_pos(x as i32, y as i32) else {
                return;
            };

            let Some(displayed_item) = this.displayed_item_for_child(&flow_child_for_ctx) else {
                return;
            };
            let media_id = displayed_item.media_id;

            let in_multi_mode = this.is_multi_select_mode();
            // Right-click only summons the menu; it never
            // modifies the selection, so no checkmark appears
            // (and none disappear). Acting on the selection
            // while offering "exit multi-select" would
            // contradict itself. The menu targets the full
            // selection only when the right-clicked photo is
            // already part of it, otherwise just that photo.
            let target_indices = this.context_menu_targets(media_id);
            let favorite_state = (on_query_favorite_state_ctx)(target_indices.clone());

            let Some(context_overlay) = this.imp().context_menu_overlay.borrow().clone() else {
                return;
            };
            let mut items = Vec::new();

            // Multi-select / Exit Multi-select.
            if in_multi_mode {
                let weak_exit = weak_for_context.clone();
                items.push(GlassMenuItem::new(
                    tr("photos.batch.exit_multi_select"),
                    GlassMenuItemKind::Danger,
                    move || {
                        if let Some(this) = weak_exit.upgrade() {
                            this.set_multi_select_mode(false);
                        }
                    },
                ));
            } else {
                let weak_enter = weak_for_context.clone();
                let flow_for_ctx_enter = flow_for_ctx.clone();
                let flow_child_for_ctx_enter = flow_child_for_ctx.clone();
                items.push(GlassMenuItem::new(
                    tr("photos.batch.multi_select"),
                    GlassMenuItemKind::Suggested,
                    move || {
                        if let Some(this) = weak_enter.upgrade() {
                            this.set_multi_select_mode(true);
                            this.ensure_context_selection(
                                &flow_for_ctx_enter,
                                &flow_child_for_ctx_enter,
                                media_id,
                            );
                        }
                    },
                ));
            }

            // Favorite / Unfavorite (single and batch context).
            if favorite_state.can_favorite {
                let indices_for_fav = target_indices.clone();
                let on_set_favorite_fav = on_set_favorite_ctx.clone();
                items.push(GlassMenuItem::new(
                    tr("photos.batch.favorite"),
                    GlassMenuItemKind::Normal,
                    move || {
                        on_set_favorite_fav(indices_for_fav.clone(), true);
                    },
                ));
            }
            if favorite_state.can_unfavorite {
                let indices_for_unfav = target_indices.clone();
                let on_set_favorite_unfav = on_set_favorite_ctx.clone();
                items.push(GlassMenuItem::new(
                    tr("photos.batch.unfavorite"),
                    GlassMenuItemKind::Normal,
                    move || {
                        on_set_favorite_unfav(indices_for_unfav.clone(), false);
                    },
                ));
            }

            if !target_indices.is_empty() {
                if !in_multi_mode {
                    if let Some(on_set_album_cover_ctx) = on_set_album_cover_ctx.clone() {
                        items.push(GlassMenuItem::new(
                            tr("album.context.set_cover"),
                            GlassMenuItemKind::Normal,
                            move || {
                                on_set_album_cover_ctx(media_id);
                            },
                        ));
                    }
                }

                let indices_for_album = target_indices.clone();
                let on_add_to_album_ctx = on_add_to_album_ctx.clone();
                items.push(GlassMenuItem::new(
                    tr("photos.batch.move_to_album"),
                    GlassMenuItemKind::Normal,
                    move || {
                        on_add_to_album_ctx(indices_for_album.clone());
                    },
                ));

                let indices_for_trash = target_indices.clone();
                let on_move_to_trash_ctx = on_move_to_trash_ctx.clone();
                let grid_weak = this.downgrade();
                items.push(GlassMenuItem::new(
                    tr("viewer.tooltip.move_to_trash"),
                    GlassMenuItemKind::Danger,
                    move || {
                        let count = indices_for_trash.len();
                        let body = if count == 1 {
                            tr("trash.confirm_body_one")
                        } else {
                            tr("trash.confirm_body_many").replace("{count}", &count.to_string())
                        };
                        let dialog = adw::AlertDialog::builder()
                            .heading(tr("trash.confirm_title"))
                            .body(body)
                            .build();
                        dialog.add_css_class("glass-alert-dialog");
                        dialog.add_response("cancel", &tr("dialog.cancel"));
                        dialog.add_response("trash", &tr("dialog.trash"));
                        dialog
                            .set_response_appearance("trash", adw::ResponseAppearance::Destructive);
                        dialog.set_default_response(Some("cancel"));
                        dialog.set_close_response("cancel");

                        let indices2 = indices_for_trash.clone();
                        let on_move2 = on_move_to_trash_ctx.clone();
                        dialog.connect_response(None, move |_, response| {
                            if response == "trash" {
                                on_move2(indices2.clone());
                            }
                        });

                        if let Some(grid) = grid_weak.upgrade() {
                            dialog.present(&grid);
                        }
                    },
                ));
            }

            glass_context_menu::show(&context_overlay, flow_for_ctx.upcast_ref(), x, y, items);
        });

        flow.add_controller(gesture);
    }

    fn restore_rebuilt_scroll(&self, saved_scroll: f64) {
        // 重建后恢复滚动位置：用 idle 回调等下一帧 layout 完成后再设值，
        // 否则 adj.upper 仍为零，会被 clamp 吞掉。
        let pending_ratio = self.imp().pending_scroll_ratio.take();
        if pending_ratio.is_some() || saved_scroll > 0.0 {
            let weak = self.downgrade();
            gtk::glib::idle_add_local_once(move || {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let adj = this.imp().scroller.get().vadjustment();
                let restored = if let Some(ratio) = pending_ratio {
                    let scrollable = (adj.upper() - adj.page_size()).max(0.0);
                    scrollable * ratio.clamp(0.0, 1.0)
                } else {
                    saved_scroll.min(adj.upper() - adj.page_size())
                };
                if restored >= 0.0 {
                    this.imp().restoring_scroll.set(true);
                    adj.set_value(restored);
                    this.imp().restoring_scroll.set(false);
                }
            });
        }
    }
}
