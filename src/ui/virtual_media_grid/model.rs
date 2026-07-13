//! `gio::ListModel` projection for the Photos GridView.
//!
//! The model has one logical item per physical grid slot, but it only retains
//! `MediaItem`s for the bounded ranges that are currently warm.  `item()` is
//! deliberately synchronous and allocation-light: it creates a small boxed
//! slot snapshot and never starts a database or thumbnail operation.

use super::layout_index::{GridSlot, VirtualGridLayoutIndex};
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::section_model::SectionKey;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ops::Range;

/// Immutable state a GridView factory reads for one physical list position.
#[derive(Debug, Clone, PartialEq)]
pub enum GridSlotState {
    Filler {
        slot: u32,
        section: SectionKey,
    },
    Placeholder {
        slot: u32,
        media_offset: u32,
    },
    Ready {
        slot: u32,
        media_offset: u32,
        item: Box<MediaItem>,
    },
}

impl GridSlotState {
    pub fn slot(&self) -> u32 {
        match self {
            Self::Filler { slot, .. }
            | Self::Placeholder { slot, .. }
            | Self::Ready { slot, .. } => *slot,
        }
    }

    pub fn media_offset(&self) -> Option<u32> {
        match self {
            Self::Filler { .. } => None,
            Self::Placeholder { media_offset, .. } | Self::Ready { media_offset, .. } => {
                Some(*media_offset)
            }
        }
    }

    pub fn media_item(&self) -> Option<&MediaItem> {
        match self {
            Self::Ready { item, .. } => Some(item),
            Self::Filler { .. } | Self::Placeholder { .. } => None,
        }
    }
}

#[derive(Default)]
pub(super) struct ModelState {
    layout: VirtualGridLayoutIndex,
    layout_generation: u64,
    ready_by_offset: BTreeMap<u32, MediaItem>,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct VirtualMediaModel {
        pub(super) state: RefCell<ModelState>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VirtualMediaModel {
        const NAME: &'static str = "PvVirtualMediaModel";
        type Type = super::VirtualMediaModel;
        type ParentType = glib::Object;
        type Interfaces = (gio::ListModel,);
    }

    impl ObjectImpl for VirtualMediaModel {}

    impl gio::subclass::prelude::ListModelImpl for VirtualMediaModel {
        fn item_type(&self) -> glib::Type {
            glib::BoxedAnyObject::static_type()
        }

        fn n_items(&self) -> u32 {
            self.state.borrow().layout.slot_count()
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            let state = self.state.borrow();
            let slot = slot_state_at(&state, position)?;
            Some(glib::BoxedAnyObject::new(slot).upcast())
        }
    }
}

glib::wrapper! {
    pub struct VirtualMediaModel(ObjectSubclass<imp::VirtualMediaModel>)
        @implements gio::ListModel;
}

impl VirtualMediaModel {
    pub fn new(layout: VirtualGridLayoutIndex) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().state.borrow_mut().layout = layout;
        obj
    }

    pub fn layout(&self) -> VirtualGridLayoutIndex {
        self.imp().state.borrow().layout.clone()
    }

    pub fn layout_generation(&self) -> u64 {
        self.imp().state.borrow().layout_generation
    }

    /// Replaces the physical layout after a complete metadata generation has
    /// arrived.  This is the only operation allowed to change `n_items`; range
    /// results use local replacement signals instead.
    pub fn replace_layout(&self, layout: VirtualGridLayoutIndex, generation: u64) {
        let (old_slots, new_slots) = {
            let mut state = self.imp().state.borrow_mut();
            let old_slots = state.layout.slot_count();
            state.layout = layout;
            state.layout_generation = generation;
            state.ready_by_offset.clear();
            (old_slots, state.layout.slot_count())
        };
        self.items_changed(0, old_slots, new_slots);
    }

    /// Apply one repository range.  Only positions whose `MediaItem` actually
    /// changed receive replacement notifications; GTK can therefore rebind
    /// visible cells without losing the model or the scroll adjustment.
    pub fn replace_ready_range(&self, range: Range<u32>, items: Vec<MediaItem>) {
        if range.start >= range.end {
            return;
        }

        let mut changed_slots = Vec::new();
        {
            let mut state = self.imp().state.borrow_mut();
            let returned_end = range
                .start
                .saturating_add(items.len().min(u32::MAX as usize) as u32)
                .min(range.end);

            for offset in range.start..range.end {
                let incoming = if offset < returned_end {
                    items.get((offset - range.start) as usize)
                } else {
                    None
                };
                let changed = match incoming {
                    Some(item) => state
                        .ready_by_offset
                        .get(&offset)
                        .is_none_or(|existing| existing != item),
                    None => state.ready_by_offset.contains_key(&offset),
                };
                if !changed {
                    continue;
                }
                match incoming {
                    Some(item) => {
                        state.ready_by_offset.insert(offset, item.clone());
                    }
                    None => {
                        state.ready_by_offset.remove(&offset);
                    }
                }
                if let Some(slot) = state.layout.slot_for_media_offset(offset) {
                    changed_slots.push(slot);
                }
            }
        }
        self.emit_replacements(changed_slots);
    }

    /// Keep only a bounded logical media window in memory.  Existing GTK list
    /// items retain their own boxed slot snapshots while bound; later re-entry
    /// turns an evicted slot back into a placeholder and schedules a new range.
    pub fn evict_outside(&self, keep: Range<u32>) {
        let mut changed_slots = Vec::new();
        {
            let mut state = self.imp().state.borrow_mut();
            let evicted_offsets = state
                .ready_by_offset
                .keys()
                .copied()
                .filter(|offset| *offset < keep.start || *offset >= keep.end)
                .collect::<Vec<_>>();
            for offset in evicted_offsets {
                state.ready_by_offset.remove(&offset);
                if let Some(slot) = state.layout.slot_for_media_offset(offset) {
                    changed_slots.push(slot);
                }
            }
        }
        self.emit_replacements(changed_slots);
    }

    pub fn slot_state(&self, position: u32) -> Option<GridSlotState> {
        slot_state_at(&self.imp().state.borrow(), position)
    }

    pub fn ready_item_for_media_id(&self, media_id: MediaId) -> Option<MediaItem> {
        self.imp()
            .state
            .borrow()
            .ready_by_offset
            .values()
            .find(|item| item.id == media_id.get())
            .cloned()
    }

    pub fn ready_item_count(&self) -> usize {
        self.imp().state.borrow().ready_by_offset.len()
    }

    pub fn ready_media_ids(&self) -> Vec<MediaId> {
        self.imp()
            .state
            .borrow()
            .ready_by_offset
            .values()
            .map(|item| MediaId::from(item.id))
            .collect()
    }

    /// Rebind all currently warm media positions without changing their data.
    /// Selection is application-owned by stable `MediaId`, so a GridView cell
    /// needs this small notification when selection changes while it remains
    /// bound to the same slot object.
    pub fn refresh_ready_slots(&self) {
        let positions = {
            let state = self.imp().state.borrow();
            state
                .ready_by_offset
                .keys()
                .filter_map(|offset| state.layout.slot_for_media_offset(*offset))
                .collect()
        };
        self.emit_replacements(positions);
    }

    fn emit_replacements(&self, mut positions: Vec<u32>) {
        positions.sort_unstable();
        positions.dedup();
        let mut run_start = None;
        let mut previous: u32 = 0;

        for position in positions {
            match run_start {
                Some(start) if position == previous.saturating_add(1) => {
                    previous = position;
                    let _ = start;
                }
                Some(start) => {
                    let count = previous.saturating_sub(start).saturating_add(1);
                    self.items_changed(start, count, count);
                    run_start = Some(position);
                    previous = position;
                }
                None => {
                    run_start = Some(position);
                    previous = position;
                }
            }
        }
        if let Some(start) = run_start {
            let count = previous.saturating_sub(start).saturating_add(1);
            self.items_changed(start, count, count);
        }
    }
}

fn slot_state_at(state: &ModelState, position: u32) -> Option<GridSlotState> {
    match state.layout.slot_at(position)? {
        GridSlot::Filler { section } => Some(GridSlotState::Filler {
            slot: position,
            section: state.layout.section_span(section)?.key.clone(),
        }),
        GridSlot::MediaOffset(media_offset) => match state.ready_by_offset.get(&media_offset) {
            Some(item) => Some(GridSlotState::Ready {
                slot: position,
                media_offset,
                item: Box::new(item.clone()),
            }),
            None => Some(GridSlotState::Placeholder {
                slot: position,
                media_offset,
            }),
        },
    }
}

#[cfg(test)]
mod tests;
