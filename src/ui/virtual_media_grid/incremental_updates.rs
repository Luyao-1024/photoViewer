//! Incremental grid admission. Shared-list changes are only a seed; canonical
//! ranks and section counts are resolved off the GTK thread so unaffected
//! sections keep their GObjects and painted thumbnails.

use super::*;

/// What the event that queued an item proves about its relation to the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IncrementalItemKind {
    /// The item was not part of the query before this event.
    Inserted,
    /// The item is already part of the query and only its fields changed.
    Updated,
    /// A bounded upsert; whether it is new is resolved from the count delta.
    Upsert,
}

impl IncrementalItemKind {
    /// Pending events for one identity coalesce to the strongest claim:
    /// an insertion outranks an unknown upsert, which outranks an update.
    fn merge(self, newer: Self) -> Self {
        fn rank(kind: IncrementalItemKind) -> u8 {
            match kind {
                IncrementalItemKind::Updated => 0,
                IncrementalItemKind::Upsert => 1,
                IncrementalItemKind::Inserted => 2,
            }
        }
        if rank(newer) > rank(self) {
            newer
        } else {
            self
        }
    }
}

/// Resolve unknown upserts against the authoritative count delta.
///
/// `None` means the change set cannot be explained without moving an existing
/// item (a matching sort-key change or a concurrent unseen insertion), so the
/// caller must fall back to a full metadata reload. Only pure insertions are
/// admitted here because `refresh_ready_item` cannot relocate an existing
/// media offset.
fn resolve_inserted_kinds(
    old_total: u32,
    total: u32,
    kinds: &[IncrementalItemKind],
) -> Option<Vec<bool>> {
    if total < old_total {
        return None;
    }
    let delta = total - old_total;
    let inserted = kinds
        .iter()
        .filter(|kind| **kind == IncrementalItemKind::Inserted)
        .count() as u32;
    let unknown = kinds
        .iter()
        .filter(|kind| **kind == IncrementalItemKind::Upsert)
        .count() as u32;
    if delta < inserted || delta > inserted.saturating_add(unknown) {
        return None;
    }
    if delta - inserted != unknown {
        return None;
    }
    Some(
        kinds
            .iter()
            .map(|kind| *kind != IncrementalItemKind::Updated)
            .collect(),
    )
}

impl VirtualMediaGrid {
    /// Whether an updated item changes anything a bound tile renders.
    ///
    /// A cloud row that gains its local original keeps the same URI, timestamp
    /// and thumbnail bytes; rebinding it would blank and repaint a thumbnail
    /// that did not change.
    fn needs_thumbnail_rebind(&self, resident: &MediaItem, item: &MediaItem) -> bool {
        self.thumbnail_uri_for(resident) != self.thumbnail_uri_for(item)
            || thumbnail_request_mtime(resident) != thumbnail_request_mtime(item)
            || resident.mime_type != item.mime_type
            || resident.media_attributes != item.media_attributes
            || resident.video_duration_secs != item.video_duration_secs
            || resident.is_favorite != item.is_favorite
            || resident.can_display() != item.can_display()
    }

    pub(crate) fn begin_incremental_item_update(
        &self,
        items: &[MediaItem],
        kind: IncrementalItemKind,
    ) {
        self.imp().suppress_incremental_projection.set(true);
        if !self.imp().active.get() {
            self.schedule_metadata_reload("incremental_inactive");
            return;
        }
        if !self.imp().metadata_ready.get() {
            self.schedule_metadata_reload("incremental_metadata_not_ready");
            return;
        }
        if self.imp().metadata_loading.get() {
            self.schedule_metadata_reload("incremental_metadata_loading");
            return;
        }
        for item in items {
            let mut pending = self.imp().pending_incremental_items.borrow_mut();
            pending
                .entry(item.id)
                .and_modify(|(snapshot, pending_kind)| {
                    *snapshot = item.clone();
                    *pending_kind = pending_kind.merge(kind);
                })
                .or_insert((item.clone(), kind));
        }
        self.schedule_incremental_item_update();
    }

    pub(crate) fn end_incremental_item_update(&self) {
        self.imp().suppress_incremental_projection.set(false);
    }

    fn schedule_incremental_item_update(&self) {
        if self.imp().incremental_update_loading.get()
            || self.imp().incremental_update_source.borrow().is_some()
        {
            return;
        }
        let weak = self.downgrade();
        let source = glib::timeout_add_local_once(Duration::from_millis(120), move || {
            let Some(grid) = weak.upgrade() else {
                return;
            };
            grid.imp().incremental_update_source.borrow_mut().take();
            grid.load_incremental_item_update();
        });
        *self.imp().incremental_update_source.borrow_mut() = Some(source);
    }

    fn load_incremental_item_update(&self) {
        let pending = std::mem::take(&mut *self.imp().pending_incremental_items.borrow_mut());
        if pending.is_empty() {
            return;
        }
        if !self.imp().active.get() {
            self.schedule_metadata_reload("incremental_inactive");
            return;
        }
        if self.imp().metadata_loading.get() {
            self.schedule_metadata_reload("incremental_loading");
            return;
        }
        let model = self.model();
        // A thumbnail-only update cannot move a media offset or a section
        // count: the sort key was fixed when the row entered the query and the
        // row is still in it. Rebind resident slots in memory; unloaded slots
        // pick up the committed snapshot on their next page load.
        let mut refreshed = 0;
        let mut admissions = Vec::new();
        for (id, (item, kind)) in pending {
            if kind == IncrementalItemKind::Updated {
                let Some(offset) = model.ready_offset_for_media_id(MediaId::from(id)) else {
                    continue;
                };
                let rebind = model
                    .ready_item_for_media_id(MediaId::from(id))
                    .is_none_or(|resident| self.needs_thumbnail_rebind(&resident, &item));
                if rebind {
                    self.loader().invalidate_cached_thumbnail(
                        &self.thumbnail_uri_for(&item),
                        Some(thumbnail_request_mtime(&item)),
                    );
                    model.refresh_ready_item(offset, item);
                    refreshed += 1;
                } else {
                    model.update_ready_item_in_place(offset, item);
                }
            } else {
                admissions.push((id, item, kind));
            }
        }
        if refreshed > 0 {
            self.notify_view_changed();
        }
        if admissions.is_empty() {
            tracing::debug!(target: crate::core::log_targets::BROWSING,
                refreshed, "virtual_grid_resident_updates_refreshed");
            return;
        }
        self.imp().incremental_update_loading.set(true);
        let generation = self.imp().metadata_generation.get();
        let pool = self.loader().pool().clone();
        let query = self.query();
        let mode = self.mode();
        // A changed photo that is not part of this grid's query cannot move a
        // resident offset. Only one that was already resident can have left the
        // query, which a reload is needed to reflect.
        let resident_before = admissions
            .iter()
            .map(|(id, _, _)| *id)
            .filter(|id| model.ready_item_for_media_id(MediaId::from(*id)).is_some())
            .collect::<Vec<_>>();
        let expected_items = admissions.len();
        let captured = admissions.clone();
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                let repo = MediaRepository::new(pool);
                // The whole batch is ordered in one walk instead of one COUNT
                // per photo, and the walk stops at the last admitted row.
                let ids = admissions.iter().map(|(id, _, _)| *id).collect::<Vec<_>>();
                let ranks = repo.positions_in_order(query.clone(), &ids)?;
                if ranks.is_empty() {
                    // Every admission belongs to another query. No insert can
                    // move this layout, so its section counts stay as they are.
                    return Ok::<_, crate::core::error::AppError>(None);
                }
                // Every row the query matches falls into exactly one date
                // group, so the aggregation is the authoritative total. A
                // second full COUNT would rescan the same view per batch.
                let groups = repo.date_groups_for_query(query.clone())?;
                let total = groups
                    .iter()
                    .fold(0u32, |total, (_, _, _, count)| total.saturating_add(*count));
                let mut ranked = Vec::with_capacity(ranks.len());
                for (id, item, kind) in admissions {
                    if let Some(rank) = ranks.get(&id) {
                        ranked.push((*rank, item, kind));
                    }
                }
                ranked.sort_by_key(|(rank, _, _)| *rank);
                Ok(Some((
                    total,
                    counts_from_date_groups(&groups, mode),
                    counts_from_date_groups(&groups, GroupBy::Day),
                    ranked,
                )))
            })
            .await;
            let Some(grid) = weak.upgrade() else {
                return;
            };
            grid.imp().incremental_update_loading.set(false);
            if !grid.imp().pending_incremental_items.borrow().is_empty() {
                // Coalesce events received during the query. Keep the strongest
                // insertion claim and the latest item snapshot for each identity.
                let mut pending = grid.imp().pending_incremental_items.borrow_mut();
                for (id, item, kind) in captured {
                    pending
                        .entry(id)
                        .and_modify(|(_, value)| *value = value.merge(kind))
                        .or_insert((item, kind));
                }
                drop(pending);
                grid.schedule_incremental_item_update();
                return;
            }
            let applied = match result {
                Ok(Ok(Some((total, counts, day_counts, ranked))))
                    if grid.imp().metadata_generation.get() == generation =>
                {
                    grid.apply_incremental_item_update(
                        total,
                        counts,
                        day_counts,
                        ranked,
                        expected_items,
                        &resident_before,
                    )
                }
                Ok(Ok(None)) => true,
                _ => false,
            };
            if !applied {
                tracing::debug!(target: crate::core::log_targets::BROWSING,
                    "virtual_grid_incremental_items_fallback");
                grid.schedule_metadata_reload("incremental_fallback");
            }
        });
    }

    fn apply_incremental_item_update(
        &self,
        total: u32,
        counts: HashMap<SectionKey, u32>,
        day_counts: HashMap<SectionKey, u32>,
        ranked: Vec<(u32, MediaItem, IncrementalItemKind)>,
        expected_items: usize,
        resident_before: &[i64],
    ) -> bool {
        if !self.imp().active.get() {
            return false;
        }
        if ranked.len() < expected_items {
            let matched = ranked
                .iter()
                .map(|(_, item, _)| item.id)
                .collect::<HashSet<_>>();
            if resident_before.iter().any(|id| !matched.contains(id)) {
                return false;
            }
            if ranked.is_empty() {
                // Every changed photo belongs to another query (another album,
                // or a row this grid never shows). The shared list already
                // carries them; reloading here would rebind every painted tile
                // for a page whose content did not change.
                return true;
            }
        }
        let model = self.model();
        let old_total = model.layout().media_count();
        // A filtered store can replay an item the grid has already applied at
        // its canonical rank (the album page refreshes its store per media
        // event). Counting the duplicate again would make the count delta
        // unprovable and force a full reload, so resident claims are dropped.
        let ranked = ranked
            .into_iter()
            .filter(|(rank, item, kind)| {
                *kind == IncrementalItemKind::Updated
                    || model.ready_offset_for_media_id(MediaId::from(item.id)) != Some(*rank)
            })
            .collect::<Vec<_>>();
        if ranked.is_empty() {
            return true;
        }
        let kinds = ranked.iter().map(|(_, _, kind)| *kind).collect::<Vec<_>>();
        let Some(inserted_flags) = resolve_inserted_kinds(old_total, total, &kinds) else {
            return false;
        };
        let ranked = ranked
            .into_iter()
            .zip(inserted_flags)
            .map(|((rank, item, _), inserted)| (rank, item, inserted))
            .collect::<Vec<_>>();

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
            total, inserted, updated, "virtual_grid_incremental_items_applied");
        true
    }
}

#[cfg(test)]
mod tests;
