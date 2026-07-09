use crate::core::i18n::tr;
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::repository::MediaRepository;
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize, TIER_BOOST};
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita::prelude::NavigationPageExt;

use super::stage::viewer_preview_thumbnail_size;
use super::{NavDelta, ViewerPage, NAV_POP};

/// Upper bound the deferred switch will wait for the target preview thumbnail
/// before switching anyway. The thumbnail is almost always warm (prefetched by
/// `prefetch_neighbors`), so this is a pure safety net against a stuck
/// generation/queue — the current frame is held meanwhile, never a spinner.
const NAV_READY_TIMEOUT_MS: u64 = 400;

pub(super) fn find_media_index_by_id(list: &gio::ListStore, item_id: i64) -> Option<u32> {
    for idx in 0..list.n_items() {
        let Some(obj) = list.item(idx) else {
            continue;
        };
        let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
            continue;
        };
        if boxed.borrow::<MediaItem>().id == item_id {
            return Some(idx);
        }
    }
    None
}

pub(super) fn next_index_after_deleted_item(deleted_index: u32, remaining_len: u32) -> Option<u32> {
    if remaining_len == 0 {
        None
    } else {
        Some(deleted_index.min(remaining_len - 1))
    }
}

pub(super) fn index_for_media_id(media_list: &gio::ListStore, media_id: MediaId) -> Option<u32> {
    for index in 0..media_list.n_items() {
        let Some(obj) = media_list.item(index) else {
            continue;
        };
        let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
            continue;
        };
        if boxed.borrow::<MediaItem>().id == media_id.get() {
            return Some(index);
        }
    }
    None
}

impl ViewerPage {
    pub(super) fn fire_nav(&self, delta: NavDelta) {
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_DEBUG fire_nav delta={} index={} details_revealed={} editor_revealed={} fullscreen_preview_open={} can_pop={} root_present={} header_visible={} bottom_visible={}",
            delta,
            self.imp().current_index.get(),
            self.imp().details_split_view.get().shows_sidebar(),
            self.imp().editor_split_view.get().shows_sidebar(),
            self.imp().fullscreen_preview_window.borrow().is_some(),
            self.can_pop(),
            self.root().is_some(),
            self.imp().header_bar.get().is_visible(),
            self.imp().viewer_bottom_stack.get().is_visible()
        );
        let cb = self.imp().nav_cb.borrow().clone();
        if let Some(cb) = cb {
            cb(delta);
        }
    }

    #[tracing::instrument(name = "viewer:navigate", skip(self))]
    pub(super) fn navigate_by_delta(&self, delta: NavDelta) {
        if delta == NAV_POP {
            self.fire_nav(delta);
            return;
        }

        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            self.fire_nav(delta);
            return;
        };
        let Some(query) = self.imp().media_query.borrow().clone() else {
            self.fire_nav(delta);
            return;
        };
        let current_id = self.imp().current_media_id.get();
        if current_id == 0 {
            self.fire_nav(delta);
            return;
        }

        // Bump the nav token so any in-flight prefetch / thumbnail-wait from a
        // previous press is discarded: latest press wins, rapid presses chain.
        let token = {
            let t = self.imp().nav_token.get() + 1;
            self.imp().nav_token.set(t);
            t
        };

        // Fast path: the ±1 neighbour was prefetched (item + Medium thumb
        // warmed) during the previous show_at. Skip the DB query entirely.
        if delta == 1 || delta == -1 {
            if let Some(item) = self.take_cached_neighbor(delta) {
                let neighbor_id = item.id;
                let index = self.ensure_media_item_in_window(item.clone());
                tracing::debug!(
                    target: crate::core::log_targets::VIEWER,
                    "VIEWER_SWITCH nav cache_hit delta={} target_index={} neighbor_id={}",
                    delta,
                    index,
                    neighbor_id
                );
                // Optimistically advance logical position so a consecutive
                // press chains from here; `current_index` (what the title,
                // favorite, filmstrip, editor all read) stays synced to the
                // display via show_at.
                self.imp().current_media_id.set(neighbor_id);
                self.switch_when_thumb_ready(index, item, token);
                return;
            }
        }

        let weak = self.downgrade();
        let (tx, rx) = tokio::sync::oneshot::channel();
        // `viewer:nav_db_query` spans the async neighbour-lookup wait, the
        // trace replacement for the old `db_query_ms` timing log.
        let nav_db_span = tracing::info_span!("viewer:nav_db_query", token);
        gio::spawn_blocking(move || {
            let repo = MediaRepository::new(pool);
            let result = repo.neighbor(query, MediaId::from(current_id), delta);
            let _ = tx.send(result);
        });
        glib::spawn_future_local(async move {
            let _nav_db_span = nav_db_span.enter();
            let result = match rx.await {
                Ok(r) => r,
                Err(_) => return,
            };
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.imp().nav_token.get() != token {
                return; // a newer press superseded this one
            }
            match result {
                Ok(Some(neighbor)) => {
                    let item = neighbor.item;
                    let neighbor_id = item.id;
                    let index = this.ensure_media_item_in_window(item.clone());
                    tracing::debug!(
                        target: crate::core::log_targets::VIEWER,
                        "VIEWER_SWITCH nav resolved delta={} target_index={} neighbor_id={}",
                        delta,
                        index,
                        neighbor_id
                    );
                    this.imp().current_media_id.set(neighbor_id);
                    this.switch_when_thumb_ready(index, item, token);
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!("ViewerPage: repository navigation failed: {err}");
                    this.fire_nav(delta);
                }
            }
        });
    }

    /// Consume the cached ±1 neighbour for `delta` if it was prefetched for
    /// the current item. Returns `None` for non-±1 deltas or a stale/empty
    /// cache so the caller falls back to a DB neighbour query.
    fn take_cached_neighbor(&self, delta: NavDelta) -> Option<MediaItem> {
        if self.imp().cached_neighbor_for_id.get() != self.imp().current_media_id.get() {
            return None;
        }
        let item = if delta == 1 {
            self.imp().cached_next_item.borrow().clone()
        } else {
            self.imp().cached_prev_item.borrow().clone()
        };
        // Invalidate so a second identical press does not reuse the entry;
        // prefetch_neighbors repopulates after the next show_at.
        if item.is_some() {
            self.imp().cached_neighbor_for_id.set(0);
        }
        item
    }

    /// Defer the visual switch (`show_at`) for `item` at `index` until the
    /// target's Medium preview thumbnail is ready, so the current frame stays
    /// on screen and no loading animation ever appears. `token` is the nav
    /// token from the originating press; if a newer press bumps it, this
    /// wait is abandoned. A `NAV_READY_TIMEOUT_MS` fallback guarantees we
    /// never hang on the old frame; if there is no loader or the item is a
    /// video, we switch immediately (show_at keeps the old frame until its
    /// own preview/stream lands).
    fn switch_when_thumb_ready(&self, index: u32, item: MediaItem, token: u64) {
        let item_id = item.id;
        let Some(loader) = self.imp().loader.borrow().as_ref().cloned() else {
            self.settle_nav_switch(index, token, "no_loader");
            return;
        };
        if item.is_video() {
            self.settle_nav_switch(index, token, "video");
            return;
        }

        let fs_mtime = std::fs::metadata(&item.path)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        let item_mtime = fs_mtime.unwrap_or_else(|| std::time::SystemTime::from(item.file_mtime));
        let size = viewer_preview_thumbnail_size();
        let (tx, rx) = tokio::sync::oneshot::channel();
        loader.request_for_media(
            item.id,
            item.uri.clone(),
            size,
            Some(item_mtime),
            tx,
            TIER_BOOST,
        );

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            match rx.await {
                Ok(loaded) => {
                    let Some(this) = weak.upgrade() else {
                        return;
                    };
                    tracing::debug!(
                        target: crate::core::log_targets::VIEWER,
                        "VIEWER_SWITCH ready_before_switch token={} item_id={} texture={}x{}",
                        token,
                        item_id,
                        loaded.texture.width(),
                        loaded.texture.height()
                    );
                    this.settle_nav_switch(index, token, "thumb_ready");
                }
                Err(_) => {
                    // Sender dropped (queue full / generation failed): switch
                    // anyway rather than holding the old frame forever.
                    if let Some(this) = weak.upgrade() {
                        this.settle_nav_switch(index, token, "thumb_send_failed");
                    }
                }
            }
        });

        // Timeout safety net. settle_nav_switch's settled guard makes this a
        // no-op if the thumbnail already landed.
        let weak = self.downgrade();
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(NAV_READY_TIMEOUT_MS),
            move || {
                if let Some(this) = weak.upgrade() {
                    this.settle_nav_switch(index, token, "timeout");
                }
            },
        );
    }

    /// Perform the deferred `show_at` exactly once per nav token. The
    /// thumb-ready, timeout, and error fallback paths all funnel here; after
    /// the first settles, later callers for the same token (or a stale token)
    /// are no-ops.
    fn settle_nav_switch(&self, index: u32, token: u64, reason: &str) {
        if self.imp().nav_token.get() != token {
            return; // superseded by a newer press
        }
        if self.imp().nav_settled_token.get() == token {
            return; // already settled
        }
        self.imp().nav_settled_token.set(token);
        tracing::debug!(
            target: crate::core::log_targets::VIEWER,
            "VIEWER_SWITCH nav_settled token={} index={} reason={}",
            token,
            index,
            reason
        );
        self.show_at(index);
    }

    fn ensure_media_item_in_window(&self, item: MediaItem) -> u32 {
        let Some(list) = self.imp().media_list.borrow().as_ref().cloned() else {
            return 0;
        };
        if let Some(index) = find_media_index_by_id(&list, item.id) {
            return index;
        }
        let index = list.n_items();
        list.append(&glib::BoxedAnyObject::new(item));
        index
    }

    /// Warm the Medium preview thumbnail for `item` without using the result.
    /// The loader populates its mem + disk cache as a side effect of
    /// servicing the request, so a subsequent viewer preview request for the
    /// same item becomes a mem-cache hit. The reply sender is dropped on
    /// purpose — we only want the cache populated, not the texture.
    fn warm_medium_thumbnail(loader: &ThumbnailLoader, item: &MediaItem) {
        let fs_mtime = std::fs::metadata(&item.path)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        let item_mtime = fs_mtime.unwrap_or_else(|| std::time::SystemTime::from(item.file_mtime));
        let (_tx, _drop_rx) = tokio::sync::oneshot::channel();
        loader.request_for_media(
            item.id,
            item.uri.clone(),
            ThumbnailSize::Medium,
            Some(item_mtime),
            _tx,
            TIER_BOOST,
        );
        // `_drop_rx` is dropped here: the worker still runs, still caches the
        // generated thumbnail, and the reply send silently fails.
    }

    /// Prefetch the ±1 neighbour items and warm their Medium preview
    /// thumbnails. The cached items let the next `navigate_by_delta` skip the
    /// DB neighbour query (`db_query_ms`), and the warmed thumbnails make the
    /// next switch's preview a mem-cache hit instead of a disk-cache read.
    /// Fire-and-forget; stale results are dropped when `current_media_id` no
    /// longer matches the item we prefetched for.
    pub(super) fn prefetch_neighbors(&self) {
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        let Some(query) = self.imp().media_query.borrow().clone() else {
            return;
        };
        let Some(loader) = self.imp().loader.borrow().as_ref().cloned() else {
            return;
        };
        let for_id = self.imp().current_media_id.get();
        if for_id == 0 {
            return;
        }
        // Reserve the cache slot synchronously so a fast subsequent press
        // sees `cached_neighbor_for_id == current_media_id` and reads whatever
        // has resolved so far (falling back to a DB query for a direction
        // whose item is still `None`).
        self.imp().cached_neighbor_for_id.set(for_id);
        *self.imp().cached_next_item.borrow_mut() = None;
        *self.imp().cached_prev_item.borrow_mut() = None;

        for delta in [1i32, -1i32] {
            let pool = pool.clone();
            let query = query.clone();
            let loader = loader.clone();
            let weak = self.downgrade();
            let (tx, rx) = tokio::sync::oneshot::channel();
            gio::spawn_blocking(move || {
                let repo = MediaRepository::new(pool);
                let _ = tx.send(repo.neighbor(query, MediaId::from(for_id), delta));
            });
            glib::spawn_future_local(async move {
                let neighbor = match rx.await {
                    Ok(Ok(Some(n))) => n,
                    _ => return,
                };
                let item = neighbor.item;
                let Some(this) = weak.upgrade() else {
                    return;
                };
                // Stale if the user already navigated away from `for_id`.
                if this.imp().current_media_id.get() != for_id
                    || this.imp().cached_neighbor_for_id.get() != for_id
                {
                    return;
                }
                Self::warm_medium_thumbnail(&loader, &item);
                match delta {
                    1 => *this.imp().cached_next_item.borrow_mut() = Some(item),
                    -1 => *this.imp().cached_prev_item.borrow_mut() = Some(item),
                    _ => {}
                }
            });
        }
    }

    /// Wire the `<` / `>` viewer navigation buttons.
    pub(super) fn setup_nav_buttons(&self) {
        let imp = self.imp();
        imp.prev_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.previous")));
        imp.next_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.next")));

        let weak = self.downgrade();
        imp.prev_btn.get().connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.navigate_by_delta(-1);
            }
        });
        let weak = self.downgrade();
        imp.next_btn.get().connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.navigate_by_delta(1);
            }
        });
    }

    pub(super) fn setup_navigation_pop_action(&self) {
        let action_group = gio::SimpleActionGroup::new();
        let pop_action = gio::SimpleAction::new("pop", None);
        let weak = self.downgrade();
        pop_action.connect_activate(move |_, _| {
            let Some(this) = weak.upgrade() else { return };
            let details_split_view = this.imp().details_split_view.get();
            let editor_split_view = this.imp().editor_split_view.get();
            tracing::debug!(
                target: crate::core::log_targets::VIEWER,
                "VIEWER_DEBUG navigation_pop_action index={} details_revealed={} editor_revealed={} fullscreen_preview_open={} can_pop={} root_present={} header_visible={} bottom_visible={}",
                this.imp().current_index.get(),
                details_split_view.shows_sidebar(),
                editor_split_view.shows_sidebar(),
                this.imp().fullscreen_preview_window.borrow().is_some(),
                this.can_pop(),
                this.root().is_some(),
                this.imp().header_bar.get().is_visible(),
                this.imp().viewer_bottom_stack.get().is_visible()
            );
            if editor_split_view.shows_sidebar() {
                this.stop_editing();
            } else if details_split_view.shows_sidebar() {
                this.set_details_revealed(false, "navigation.pop");
            } else if !this.can_pop() {
                tracing::debug!(
                    target: crate::core::log_targets::VIEWER,
                    "ViewerPage: ignoring navigation.pop while pop is guarded"
                );
            } else {
                this.fire_nav(NAV_POP);
            }
        });
        action_group.add_action(&pop_action);
        self.insert_action_group("navigation", Some(&action_group));
    }

    /// Resolve the `MediaItem` at the current index out of the
    /// `BoxedAnyObject<MediaItem>` store. Returns `None` if the index is
    /// out of range or the item can't be downcast.
    pub(super) fn current_media_item(&self) -> Option<MediaItem> {
        if self.imp().current_media_id.get() != 0 && self.sync_current_index_to_media_id().is_none()
        {
            return None;
        }
        let list = self.imp().media_list.borrow();
        let list = list.as_ref()?;
        let idx = self.imp().current_index.get();
        crate::ui::media_list::media_item_at(list, idx)
    }

    pub(super) fn sync_current_index_to_media_id(&self) -> Option<u32> {
        let current_id = self.imp().current_media_id.get();
        if current_id == 0 {
            return Some(self.imp().current_index.get());
        }
        let list = self.imp().media_list.borrow();
        let list = list.as_ref()?;
        let index = find_media_index_by_id(list, current_id)?;
        self.imp().current_index.set(index);
        Some(index)
    }

    /// Warm the OS page cache for the neighbour at `current + offset` so the
    /// next original decode does not stall on disk I/O. We only `read` the
    /// file bytes (cheap) rather than fully decoding it: a full
    /// `load_oriented_pixbuf` per neighbour used to fire two concurrent HEIC
    /// decodes on every switch, stealing CPU from the current decode and
    /// inflating `switch_to_orig_ms`. The neighbour's *thumbnail* is warmed
    /// separately by `prefetch_neighbors`, which is the cheaper and more
    /// important cache for the perceived switch latency.
    pub(super) fn preload_neighbor_pages(&self, offset: i32) {
        let cur = self.imp().current_index.get() as i32;
        let target = cur + offset;
        let path = {
            let list = self.imp().media_list.borrow();
            let list = match list.as_ref() {
                Some(l) => l,
                None => return,
            };
            if target < 0 {
                return;
            }
            let target_u = target as u32;
            if target_u >= list.n_items() {
                return;
            }
            let Some(obj) = list.item(target_u) else {
                return;
            };
            let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let item = boxed.borrow::<MediaItem>();
            if item.is_video() {
                return;
            }
            super::stage::strip_file_uri(&item.uri)
        };
        gio::spawn_blocking(move || {
            // `read` pulls the file into the OS page cache; the bytes are
            // dropped immediately. Deliberately not decoding — the next
            // switch decodes on demand.
            let _ = std::fs::read(&path);
        });
    }
}
