//! Section-aware physical slot indexing for the virtual media grid.
//!
//! The index keeps one [`SectionSpan`] per date section instead of one entry per
//! media item. That makes physical-slot lookups cheap without making a
//! full-library `Vec` of media or filler slots.

use crate::core::section_model::SectionKey;
use std::cmp::Reverse;
use std::collections::HashMap;

/// A contiguous date section in both logical media offsets and physical grid
/// slots.
///
/// `media_start..media_end()` contains real media offsets. `slot_start..
/// slot_end()` contains those media slots followed by any trailing filler
/// slots needed to end the section on a row boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionSpan {
    pub key: SectionKey,
    pub media_start: u32,
    pub media_len: u32,
    pub slot_start: u32,
    pub slot_len: u32,
}

impl SectionSpan {
    /// The exclusive end of this section's logical media range.
    pub fn media_end(&self) -> u32 {
        self.media_start.saturating_add(self.media_len)
    }

    /// The exclusive end of this section's physical slot range.
    pub fn slot_end(&self) -> u32 {
        self.slot_start.saturating_add(self.slot_len)
    }

    /// The number of trailing non-media slots in this section.
    pub fn filler_len(&self) -> u32 {
        self.slot_len.saturating_sub(self.media_len)
    }
}

/// What occupies a physical grid slot.
///
/// The `section` in [`GridSlot::Filler`] is an index into
/// [`VirtualGridLayoutIndex::sections`]. Keeping that index rather than a
/// cloned key lets callers retrieve both the section key and its final media
/// offset without another search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridSlot {
    MediaOffset(u32),
    Filler { section: usize },
}

/// A compact, immutable mapping between global media offsets and physical grid
/// slots.
///
/// Sections are sorted newest-first from authoritative date counts. Every
/// non-empty section occupies complete rows, so its final row is padded with
/// filler slots when necessary. The index stores `O(section_count)` metadata;
/// all lookup helpers below binary-search that metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualGridLayoutIndex {
    columns: u32,
    sections: Vec<SectionSpan>,
    media_count: u32,
    slot_count: u32,
}

impl Default for VirtualGridLayoutIndex {
    fn default() -> Self {
        Self {
            columns: 1,
            sections: Vec::new(),
            media_count: 0,
            slot_count: 0,
        }
    }
}

impl VirtualGridLayoutIndex {
    /// Builds an index from full-library, authoritative section counts.
    ///
    /// The count map has no iteration order, so keys are sorted newest-first
    /// before spans are built. Zero-count sections are omitted. `columns == 0`
    /// is normalized to one column so callers can safely build an index before
    /// a grid has reported its first allocation.
    ///
    /// GTK list-model positions are `u32`. If pathological input would exceed
    /// that representable range, this keeps the newest-first representable
    /// prefix rather than overflowing starts or lengths.
    pub fn new(section_counts: &HashMap<SectionKey, u32>, columns: u32) -> Self {
        let columns = columns.max(1);
        let mut ordered_sections: Vec<(SectionKey, u32)> = section_counts
            .iter()
            .filter(|(_, count)| **count > 0)
            .map(|(key, count)| (key.clone(), *count))
            .collect();
        ordered_sections.sort_unstable_by_key(|(key, _)| Reverse(section_sort_key(key)));

        let mut sections = Vec::with_capacity(ordered_sections.len());
        let mut media_start = 0;
        let mut slot_start = 0;

        for (key, count) in ordered_sections {
            let remaining_media = u32::MAX.saturating_sub(media_start);
            let remaining_slots = u32::MAX.saturating_sub(slot_start);
            let media_len = count.min(remaining_media).min(remaining_slots);
            if media_len == 0 {
                break;
            }

            let slot_len = aligned_slot_len(media_len, columns).min(remaining_slots);
            debug_assert!(slot_len >= media_len);

            sections.push(SectionSpan {
                key,
                media_start,
                media_len,
                slot_start,
                slot_len,
            });

            media_start = media_start.saturating_add(media_len);
            slot_start = slot_start.saturating_add(slot_len);
        }

        Self {
            columns,
            sections,
            media_count: media_start,
            slot_count: slot_start,
        }
    }

    /// The effective column count. A requested zero column count becomes one.
    pub fn columns(&self) -> u32 {
        self.columns
    }

    /// Number of non-empty date sections represented by this index.
    pub fn section_count(&self) -> usize {
        self.sections.len()
    }

    /// Section spans in newest-first order.
    pub fn sections(&self) -> &[SectionSpan] {
        &self.sections
    }

    /// Returns a section span by its stable index within [`Self::sections`].
    pub fn section_span(&self, section: usize) -> Option<&SectionSpan> {
        self.sections.get(section)
    }

    /// Number of logical media offsets represented by this index.
    pub fn media_count(&self) -> u32 {
        self.media_count
    }

    /// Number of physical slots, suitable for a virtual `gio::ListModel`.
    pub fn slot_count(&self) -> u32 {
        self.slot_count
    }

    /// Resolves a physical slot to a media offset or a trailing filler slot.
    pub fn slot_at(&self, position: u32) -> Option<GridSlot> {
        let section = self.span_index_for_slot(position)?;
        let span = &self.sections[section];
        let offset_in_section = position - span.slot_start;

        if offset_in_section < span.media_len {
            Some(GridSlot::MediaOffset(
                span.media_start.saturating_add(offset_in_section),
            ))
        } else {
            Some(GridSlot::Filler { section })
        }
    }

    /// Resolves a media offset to its physical slot.
    pub fn slot_for_media_offset(&self, offset: u32) -> Option<u32> {
        let section = self.span_index_for_media_offset(offset)?;
        let span = &self.sections[section];
        Some(
            span.slot_start
                .saturating_add(offset.saturating_sub(span.media_start)),
        )
    }

    /// Resolves an offset to its slot, clamping offsets past the library to the
    /// oldest available media item. This is useful when restoring an anchor
    /// after its former item was removed.
    pub fn slot_for_media_offset_clamped(&self, offset: u32) -> Option<u32> {
        self.media_count
            .checked_sub(1)
            .and_then(|last_offset| self.slot_for_media_offset(offset.min(last_offset)))
    }

    /// Returns the section span containing a physical slot.
    pub fn span_for_slot(&self, position: u32) -> Option<&SectionSpan> {
        self.span_index_for_slot(position)
            .and_then(|section| self.sections.get(section))
    }

    /// Returns the section span containing a logical media offset.
    pub fn span_for_media_offset(&self, offset: u32) -> Option<&SectionSpan> {
        self.span_index_for_media_offset(offset)
            .and_then(|section| self.sections.get(section))
    }

    /// Returns the date section containing a physical slot, including filler
    /// slots at the end of that section.
    pub fn section_for_slot(&self, position: u32) -> Option<&SectionKey> {
        self.span_for_slot(position).map(|span| &span.key)
    }

    /// Returns the date section containing a logical media offset.
    pub fn section_for_media_offset(&self, offset: u32) -> Option<&SectionKey> {
        self.span_for_media_offset(offset).map(|span| &span.key)
    }

    /// Returns a media offset only when `position` is a real media slot.
    pub fn media_offset_at_slot(&self, position: u32) -> Option<u32> {
        match self.slot_at(position) {
            Some(GridSlot::MediaOffset(offset)) => Some(offset),
            Some(GridSlot::Filler { .. }) | None => None,
        }
    }

    /// Returns an anchor media offset for a physical slot.
    ///
    /// A real media slot returns its own offset. A filler slot returns the last
    /// media offset in the same section, which is the date-pill projection
    /// needed when the viewport top lands in an incomplete final row.
    pub fn anchor_media_offset_for_slot(&self, position: u32) -> Option<u32> {
        let span = self.span_for_slot(position)?;
        let offset_in_section = position - span.slot_start;

        if offset_in_section < span.media_len {
            Some(span.media_start.saturating_add(offset_in_section))
        } else {
            span.media_end().checked_sub(1)
        }
    }

    fn span_index_for_slot(&self, position: u32) -> Option<usize> {
        if position >= self.slot_count {
            return None;
        }

        let index = self
            .sections
            .partition_point(|span| span.slot_end() <= position);
        let span = self.sections.get(index)?;
        if span.slot_start <= position && position < span.slot_end() {
            Some(index)
        } else {
            None
        }
    }

    fn span_index_for_media_offset(&self, offset: u32) -> Option<usize> {
        if offset >= self.media_count {
            return None;
        }

        let index = self
            .sections
            .partition_point(|span| span.media_end() <= offset);
        let span = self.sections.get(index)?;
        if span.media_start <= offset && offset < span.media_end() {
            Some(index)
        } else {
            None
        }
    }
}

fn aligned_slot_len(media_len: u32, columns: u32) -> u32 {
    let remainder = media_len % columns;
    if remainder == 0 {
        media_len
    } else {
        media_len.saturating_add(columns - remainder)
    }
}

fn section_sort_key(key: &SectionKey) -> (i64, u32, u32, bool, bool, bool) {
    (
        key.year.map(i64::from).unwrap_or(i64::MIN),
        key.month.unwrap_or(0),
        key.day.unwrap_or(0),
        key.year.is_some(),
        key.month.is_some(),
        key.day.is_some(),
    )
}

#[cfg(test)]
mod tests;
