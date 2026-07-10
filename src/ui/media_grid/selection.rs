use super::{DisplayedItem, MediaGrid};
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use gtk4 as gtk;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use gtk4::{glib, prelude::*};
use std::collections::HashSet;
use std::rc::Rc;

impl MediaGrid {
    /// Register a callback fired whenever the selected set changes. `PhotosPage`
    /// uses this to toggle the "Add to Album" toolbar button.
    pub fn connect_selection_changed<F: Fn() + 'static>(&self, f: F) {
        self.imp()
            .on_selection_changed
            .set(Rc::new(f))
            .ok()
            .expect("MediaGrid::connect_selection_changed called more than once");
    }

    /// Snapshot of currently-selected global indices. Order is unspecified.
    pub fn selected_ids(&self) -> Vec<MediaId> {
        let s = self.imp().selected.borrow();
        s.iter().copied().collect()
    }

    /// Snapshot of currently rendered global indices in grid order.
    pub fn displayed_indices(&self) -> Vec<u32> {
        self.imp()
            .displayed_items
            .borrow()
            .iter()
            .map(|item| item.window_index)
            .collect()
    }

    pub(super) fn displayed_item_for_child(
        &self,
        child: &gtk::FlowBoxChild,
    ) -> Option<DisplayedItem> {
        self.imp()
            .displayed_items
            .borrow()
            .iter()
            .find(|item| item.flow_child == *child)
            .cloned()
    }

    pub(super) fn media_item_for_id(&self, media_id: MediaId) -> Option<MediaItem> {
        let list = self.imp().media_list.borrow().as_ref().cloned()?;
        for index in 0..list.n_items() {
            let Some(obj) = list.item(index).and_downcast::<glib::BoxedAnyObject>() else {
                continue;
            };
            let item = obj.borrow::<MediaItem>();
            if item.id == media_id.get() {
                return Some((*item).clone());
            }
        }
        None
    }

    /// Select all rendered tiles and sync visible highlights.
    pub fn select_all(&self) {
        self.imp().is_multi_select_mode.set(true);
        // `select_child` below only marks a child `:selected` while the
        // FlowBox is in `Multiple`; flip every section first.
        self.apply_selection_mode();
        let mut next = HashSet::new();
        let items = self.imp().displayed_items.borrow().clone();
        for item in items {
            if let Some(parent) = item.flow_child.parent() {
                if let Ok(flow) = parent.downcast::<gtk::FlowBox>() {
                    flow.select_child(&item.flow_child);
                }
            }
            next.insert(item.media_id);
        }

        let mut changed = false;
        {
            let mut selected = self.imp().selected.borrow_mut();
            if *selected != next {
                *selected = next;
                changed = true;
            }
        }
        if changed {
            self.fire_selection_changed();
        }
    }

    /// Replace selection with stable media ids and sync any currently rendered
    /// children. The id set can be larger than the virtual GTK window; only
    /// visible children receive `flowboxchild:selected` state.
    pub fn select_ids(&self, ids: &[MediaId]) {
        self.imp().is_multi_select_mode.set(!ids.is_empty());
        self.apply_selection_mode();

        let next = ids.iter().copied().collect::<HashSet<_>>();
        let changed = {
            let mut selected = self.imp().selected.borrow_mut();
            if *selected == next {
                false
            } else {
                *selected = next;
                true
            }
        };
        self.sync_visible_selection();
        if changed {
            self.fire_selection_changed();
        }
    }

    /// Enable/disable explicit multi-select mode.
    /// Disabling clears selection for a clean single-select state.
    pub fn set_multi_select_mode(&self, enabled: bool) {
        self.imp().is_multi_select_mode.set(enabled);
        // Flip every section FlowBox to `Multiple` (enabled) or `None`
        // (disabled) so the checkmark can only appear while multi-select is
        // active. Must precede callers that rely on `select_child`.
        self.apply_selection_mode();
        if !enabled {
            self.clear_selection();
        }
    }

    /// Whether explicit multi-select mode is enabled.
    pub fn is_multi_select_mode(&self) -> bool {
        self.imp().is_multi_select_mode.get()
    }

    /// Sync every section FlowBox's `selection_mode` with the multi-select
    /// flag: `None` when off (so the FlowBox ignores GTK's built-in selection
    /// and no child can become `:selected` — keeping the `.thumb-checkmark`
    /// hidden), `Multiple` when on. Without this the per-section FlowBoxes
    /// stayed on `Multiple` permanently and GTK could leave a child
    /// `:selected` even when the user never entered multi-select, surfacing a
    /// stray checkmark on a thumbnail.
    pub(super) fn apply_selection_mode(&self) {
        let mode = if self.imp().is_multi_select_mode.get() {
            gtk::SelectionMode::Multiple
        } else {
            gtk::SelectionMode::None
        };
        let content = self.imp().content.get();
        let mut child = content.first_child();
        while let Some(c) = child {
            if let Some(flow) = c.downcast_ref::<gtk::FlowBox>() {
                flow.set_selection_mode(mode);
            }
            child = c.next_sibling();
        }
    }

    pub(super) fn sync_visible_selection(&self) {
        let selected = self.imp().selected.borrow();
        for item in self.imp().displayed_items.borrow().iter() {
            let Some(parent) = item.flow_child.parent() else {
                continue;
            };
            let Ok(flow) = parent.downcast::<gtk::FlowBox>() else {
                continue;
            };
            if selected.contains(&item.media_id) {
                flow.select_child(&item.flow_child);
            } else {
                flow.unselect_child(&item.flow_child);
            }
        }
    }

    /// Whether every currently rendered tile is selected.
    pub fn is_all_displayed_selected(&self) -> bool {
        let selected = self.imp().selected.borrow();
        let displayed = self.imp().displayed_items.borrow();
        if displayed.is_empty() {
            return false;
        }
        for item in displayed.iter() {
            if !selected.contains(&item.media_id) {
                return false;
            }
        }
        true
    }

    fn selected_ids_sorted(&self) -> Vec<MediaId> {
        let mut ids: Vec<MediaId> = self.imp().selected.borrow().iter().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// Clear the selection (both in the `selected` set AND on every visible
    /// `FlowBox`). Fires the `selection-changed` callback if the set was
    /// non-empty before.
    pub fn clear_selection(&self) {
        self.imp().is_multi_select_mode.set(false);
        let mut changed = false;
        {
            let mut s = self.imp().selected.borrow_mut();
            if !s.is_empty() {
                s.clear();
                changed = true;
            }
        }
        // Unselect all visible FlowBox children so the highlight follows.
        let content = self.imp().content.get();
        let mut child = content.first_child();
        while let Some(c) = child {
            if let Some(flow) = c.downcast_ref::<gtk::FlowBox>() {
                flow.unselect_all();
            }
            child = c.next_sibling();
        }
        // Drop to `None` so no child can re-enter `:selected` — and reveal the
        // checkmark — until multi-select is explicitly re-entered.
        self.apply_selection_mode();
        if changed {
            self.fire_selection_changed();
        }
    }

    pub(super) fn ensure_context_selection(
        &self,
        flow: &gtk::FlowBox,
        clicked_child: &gtk::FlowBoxChild,
        media_id: MediaId,
    ) -> Vec<MediaId> {
        let was_selected = self.imp().selected.borrow().contains(&media_id);
        if !was_selected {
            {
                let mut s = self.imp().selected.borrow_mut();
                s.clear();
                s.insert(media_id);
            }
            let content = self.imp().content.get();
            let mut section_child = content.first_child();
            while let Some(child) = section_child {
                if let Some(flow_box) = child.downcast_ref::<gtk::FlowBox>() {
                    flow_box.unselect_all();
                }
                section_child = child.next_sibling();
            }
            flow.select_child(clicked_child);
            self.fire_selection_changed();
        }
        self.selected_ids_sorted()
    }
}

impl MediaGrid {
    /// Fire the `selection-changed` callback (if registered). Called whenever
    /// `selected` is mutated.
    pub(super) fn fire_selection_changed(&self) {
        if let Some(cb) = self.imp().on_selection_changed.get() {
            cb();
        }
    }

    /// Toggle membership of `media_id` in the selected set, then toggle
    /// the visual highlight on `child` via its parent `FlowBox`. Fires
    /// `selection-changed`.
    pub(super) fn toggle_selection(
        &self,
        media_id: MediaId,
        child: &gtk::FlowBoxChild,
        flow: &gtk::FlowBox,
    ) {
        let now_selected = {
            let mut s = self.imp().selected.borrow_mut();
            if s.contains(&media_id) {
                s.remove(&media_id);
                false
            } else {
                s.insert(media_id);
                true
            }
        };
        if now_selected {
            flow.select_child(child);
        } else {
            flow.unselect_child(child);
        }
        self.fire_selection_changed();
    }
}

#[cfg(test)]
mod tests;
