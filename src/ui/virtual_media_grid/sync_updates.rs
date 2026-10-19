//! Incremental cloud-download admission. Shared-list changes are only a seed;
//! canonical ranks and section counts are resolved off the GTK thread.
use super::*;

impl VirtualMediaGrid {
    pub(crate) fn begin_sync_item_update(&self, items: &[MediaItem], inserted: bool) {
        self.imp().suppress_sync_projection.set(true);
        if !self.imp().active.get()
            || !self.imp().metadata_ready.get()
            || self.imp().metadata_loading.get()
        {
            self.schedule_metadata_reload();
            return;
        }
        for item in items {
            let mut pending = self.imp().pending_sync_items.borrow_mut();
            let old_inserted = pending.get(&item.id).is_some_and(|(_, inserted)| *inserted);
            pending.insert(item.id, (item.clone(), inserted || old_inserted));
        }
        self.schedule_sync_item_update();
    }

    pub(crate) fn end_sync_item_update(&self) {
        self.imp().suppress_sync_projection.set(false);
    }

    fn schedule_sync_item_update(&self) {
        if self.imp().sync_update_loading.get() || self.imp().sync_update_source.borrow().is_some()
        {
            return;
        }
        let weak = self.downgrade();
        let source = glib::timeout_add_local_once(Duration::from_millis(120), move || {
            let Some(grid) = weak.upgrade() else {
                return;
            };
            grid.imp().sync_update_source.borrow_mut().take();
            grid.load_sync_item_update();
        });
        *self.imp().sync_update_source.borrow_mut() = Some(source);
    }

    fn load_sync_item_update(&self) {
        let pending = std::mem::take(&mut *self.imp().pending_sync_items.borrow_mut());
        if pending.is_empty() {
            return;
        }
        if !self.imp().active.get() || self.imp().metadata_loading.get() {
            self.schedule_metadata_reload();
            return;
        }
        self.imp().sync_update_loading.set(true);
        let generation = self.imp().metadata_generation.get();
        let pool = self.loader().pool().clone();
        let query = self.query();
        let mode = self.mode();
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let captured = pending.clone();
            let result = gio::spawn_blocking(move || {
                let repo = MediaRepository::new(pool);
                let total = repo.count(query.clone())?;
                let groups = repo.date_groups_for_query(query.clone())?;
                let mut ranked = Vec::new();
                for (id, (item, inserted)) in pending {
                    if let Some((rank, _)) = repo.position(query.clone(), MediaId::from(id))? {
                        ranked.push((rank.saturating_sub(1), item, inserted));
                    }
                }
                ranked.sort_by_key(|(rank, _, _)| *rank);
                Ok::<_, crate::core::error::AppError>((
                    total,
                    counts_from_date_groups(&groups, mode),
                    counts_from_date_groups(&groups, GroupBy::Day),
                    ranked,
                ))
            })
            .await;
            let Some(grid) = weak.upgrade() else {
                return;
            };
            grid.imp().sync_update_loading.set(false);
            if !grid.imp().pending_sync_items.borrow().is_empty() {
                // Coalesce events received during the query. Keep the earliest
                // insertion fact and the latest item snapshot for each identity.
                let mut pending = grid.imp().pending_sync_items.borrow_mut();
                for (id, (item, inserted)) in captured {
                    pending
                        .entry(id)
                        .and_modify(|(_, value)| *value |= inserted)
                        .or_insert((item, inserted));
                }
                drop(pending);
                grid.schedule_sync_item_update();
                return;
            }
            let applied = match result {
                Ok(Ok((total, counts, day_counts, ranked)))
                    if grid.imp().metadata_generation.get() == generation =>
                {
                    grid.apply_sync_item_update(total, counts, day_counts, ranked)
                }
                _ => false,
            };
            if !applied {
                tracing::debug!(target: crate::core::log_targets::BROWSING,
                    "virtual_grid_sync_items_fallback");
                grid.schedule_metadata_reload();
            }
        });
    }

    fn apply_sync_item_update(
        &self,
        total: u32,
        counts: HashMap<SectionKey, u32>,
        day_counts: HashMap<SectionKey, u32>,
        ranked: Vec<(u32, MediaItem, bool)>,
    ) -> bool {
        if !self.imp().active.get() {
            return false;
        }
        let model = self.model();
        let mut updated_counts = model
            .layout()
            .sections()
            .iter()
            .map(|span| (span.key.clone(), span.media_len))
            .collect::<HashMap<_, _>>();
        let inserted = ranked.iter().filter(|(_, _, inserted)| *inserted).count() as u32;
        if model.layout().media_count().checked_add(inserted) != Some(total) {
            return false;
        }
        let canonical_layout =
            VirtualGridLayoutIndex::new(&counts, self.viewport_metrics().columns());
        let mut changes = Vec::new();
        for (rank, item, inserted) in &ranked {
            if !inserted {
                continue;
            }
            // The repository owns date grouping (including local time). The
            // item's UTC date can differ near midnight; use its canonical rank
            // to identify the destination section instead of regrouping it.
            let Some(key) = canonical_layout
                .sections()
                .iter()
                .find(|span| span.media_start <= *rank && *rank < span.media_end())
                .map(|span| span.key.clone())
            else {
                return false;
            };
            *updated_counts.entry(key.clone()).or_default() += 1;
            let layout =
                VirtualGridLayoutIndex::new(&updated_counts, self.viewport_metrics().columns());
            if !layout.sections().iter().any(|span| {
                span.key == key && span.media_start <= *rank && *rank < span.media_end()
            }) {
                return false;
            }
            changes.push((layout, *rank, item.clone()));
        }
        if updated_counts != counts {
            return false;
        }
        let updated = ranked.len().saturating_sub(inserted as usize);
        for (_, item, _) in &ranked {
            self.loader().invalidate_cached_thumbnail(
                &self.thumbnail_uri_for(item),
                Some(thumbnail_request_mtime(item)),
            );
        }
        self.imp().range.borrow_mut().invalidate();
        for (layout, rank, item) in changes {
            model.insert_ready_item(layout, rank, item);
        }
        for (rank, item, inserted) in ranked {
            if !inserted {
                model.refresh_ready_item(rank, item);
            }
        }
        for range in model.ready_ranges() {
            self.imp()
                .range
                .borrow_mut()
                .mark_resident(MediaRange::new(range.start, range.end));
        }
        self.imp().live_total.set(total);
        self.imp().metadata_counts.replace(Some(counts));
        self.imp()
            .visible_date_layout
            .replace(Some(VirtualGridLayoutIndex::new(&day_counts, 1)));
        self.schedule_visible_range_after_layout();
        self.notify_view_changed();
        tracing::debug!(target: crate::core::log_targets::BROWSING,
            total, inserted, updated, "virtual_grid_sync_items_applied");
        true
    }
}
