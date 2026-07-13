//! Coalescing and residency bookkeeping for virtual media ranges.
//!
//! This module is intentionally GTK-free.  `VirtualMediaGrid` supplies the
//! visible range and performs the async repository request; the coordinator
//! decides whether it is already warm, must start now, or should replace an
//! in-flight request's pending target.

use std::cmp::{max, min};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaRange {
    pub start: u32,
    pub end: u32,
}

impl MediaRange {
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    pub fn is_empty(self) -> bool {
        self.start >= self.end
    }

    pub fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    pub fn contains(self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }

    pub fn covers(self, other: Self) -> bool {
        self.start <= other.start && self.end >= other.end
    }

    pub fn touches(self, other: Self) -> bool {
        self.start <= other.end && other.start <= self.end
    }

    pub fn union(self, other: Self) -> Self {
        Self::new(min(self.start, other.start), max(self.end, other.end))
    }

    pub fn clamp(self, total: u32) -> Self {
        Self::new(self.start.min(total), self.end.min(total))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeRequest {
    pub generation: u64,
    pub range: MediaRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestDisposition {
    Covered,
    Started(RangeRequest),
    Coalesced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Completion {
    pub apply_result: bool,
    pub next: Option<RangeRequest>,
}

/// Small coordinator rather than a per-row cache.  The full cache ownership
/// lives in `VirtualMediaModel`; this type only knows which logical ranges are
/// resident and preserves a single latest intent during rapid scrollbar drag.
#[derive(Debug, Default)]
pub struct RangeCoordinator {
    generation: u64,
    in_flight: Option<RangeRequest>,
    pending: Option<RangeRequest>,
    resident: Vec<MediaRange>,
}

impl RangeCoordinator {
    pub fn request(&mut self, requested: MediaRange) -> RequestDisposition {
        if requested.is_empty() || self.resident.iter().any(|range| range.covers(requested)) {
            return RequestDisposition::Covered;
        }
        if self
            .in_flight
            .is_some_and(|request| request.range.covers(requested))
            || self
                .pending
                .is_some_and(|request| request.range.covers(requested))
        {
            return RequestDisposition::Coalesced;
        }

        self.generation = self.generation.saturating_add(1);
        let request = RangeRequest {
            generation: self.generation,
            range: requested,
        };
        if self.in_flight.is_some() {
            self.pending = Some(request);
            RequestDisposition::Coalesced
        } else {
            self.in_flight = Some(request);
            RequestDisposition::Started(request)
        }
    }

    /// Record that a successful result has been installed into the model.
    pub fn mark_resident(&mut self, range: MediaRange) {
        if range.is_empty() {
            return;
        }
        self.resident.push(range);
        self.resident.sort_unstable_by_key(|range| range.start);
        let mut merged: Vec<MediaRange> = Vec::with_capacity(self.resident.len());
        for range in self.resident.drain(..) {
            if let Some(last) = merged.last_mut() {
                if last.touches(range) {
                    *last = last.union(range);
                } else {
                    merged.push(range);
                }
            } else {
                merged.push(range);
            }
        }
        self.resident = merged;
    }

    /// Retain only residency overlapping the current bounded cache window.
    /// The caller performs matching model eviction before or after this call.
    pub fn retain_resident_within(&mut self, keep: MediaRange) {
        self.resident = self
            .resident
            .iter()
            .filter_map(|range| {
                let clipped =
                    MediaRange::new(max(range.start, keep.start), min(range.end, keep.end));
                (!clipped.is_empty()).then_some(clipped)
            })
            .collect();
    }

    /// Invalidate all known data after a DB/listing generation change.  An
    /// in-flight result becomes stale because its generation can no longer be
    /// equal to `self.generation`.
    pub fn invalidate(&mut self) {
        self.generation = self.generation.saturating_add(1);
        self.pending = None;
        self.resident.clear();
    }

    pub fn finish(&mut self, generation: u64) -> Completion {
        let was_in_flight = self.in_flight.take();
        let apply_result = was_in_flight.is_some_and(|request| request.generation == generation)
            && generation == self.generation;
        let next = self.pending.take().inspect(|request| {
            self.in_flight = Some(*request);
        });
        Completion { apply_result, next }
    }

    pub fn current_generation(&self) -> u64 {
        self.generation
    }

    pub fn resident(&self) -> &[MediaRange] {
        &self.resident
    }

    pub fn in_flight(&self) -> Option<RangeRequest> {
        self.in_flight
    }
}

/// Convert an inclusive viewport plus overscan into a safe, bounded media
/// range.  The directional side receives twice as much overscan so quick
/// kinetic scrolling warms where the user is heading first.
pub fn expanded_visible_range(visible: MediaRange, total: u32, moving_forward: bool) -> MediaRange {
    if visible.is_empty() || total == 0 {
        return MediaRange::new(0, 0);
    }
    let overscan = visible.len().max(1);
    let before = if moving_forward {
        overscan
    } else {
        overscan.saturating_mul(2)
    };
    let after = if moving_forward {
        overscan.saturating_mul(2)
    } else {
        overscan
    };
    MediaRange::new(
        visible.start.saturating_sub(before),
        visible.end.saturating_add(after),
    )
    .clamp(total)
}

#[cfg(test)]
mod tests;
