//! The UI → audio-thread **topology channel** (ticket 0338, ADR 0003 §4).
//!
//! Matrix *topology* — source, dest, polarity, shape, scale source, scale bend,
//! enabled — is the one part of a VXN1b patch that is neither a CLAP param nor
//! rare enough to justify a lock. It used to live behind a `Mutex` in
//! [`crate::shared`] with a reload flag beside it, which meant every combo pick
//! in the matrix overlay routed the **audio thread** through that lock: a
//! priority inversion waiting for the editor thread to be preempted at the
//! wrong moment.
//!
//! This module is the replacement: a single-producer / single-consumer ring of
//! [`TopoMsg`] records. The editor (main thread) pushes, the audio thread drains
//! at the top of `process` and applies each record straight onto the engine's
//! tables. Nothing on the audio side blocks, spins, or allocates.
//!
//! ## Two channels, not one
//!
//! Deliberately *not* "SPSC everywhere" (ADR 0003 §4):
//!
//! - **Values** (depths and every other CLAP param) stay in the idempotent
//!   atomic store. They are latest-wins, they coalesce a knob drag for free,
//!   and the host needs `get_value` off the main thread regardless.
//! - **Topology** rides this ring, as [`TopoMsg::Edit`] — one field per record,
//!   so a single combo pick costs the audio thread one field write rather than
//!   a whole-patch re-sync.
//! - **Bulk** (preset load, host state restore, copy/reset layer) rides it as
//!   one [`TopoMsg::Snapshot`]: the whole two-layer table in a single record,
//!   never decomposed into 32 slot edits. It travels *in* the ring purely so
//!   that its ordering against pending edits is exact by construction; it is
//!   still one snapshot applied wholesale, and it doubles as the overflow
//!   backstop below.
//!
//! ## Overflow is defined, not merely improbable
//!
//! Topology edits are human-rate and the audio thread drains the **whole** ring
//! every block, so [`TOPO_RING_SLOTS`] records can only pile up if the host
//! stops calling `process` while the editor is being driven. Unreachable by
//! argument is not the same as undefined, so: a push that finds the ring full
//! raises the sticky `resync` flag and drops the record. The producer then
//! re-publishes a full [`TopoMsg::Snapshot`] of the authoritative table as soon
//! as there is room ([`TopologyRing::resync_pending`] →
//! `SharedParams::service_topology_resync`), which subsumes every dropped edit.
//! While a resync is pending, individual edits are not pushed at all — the
//! snapshot that will carry them is taken from the table *after* they were
//! applied to it, so pushing them as well would only be redundant work.
//!
//! ## Depth does not travel here
//!
//! Slot depth is a CLAP param (`matrix_slot{n}_depth`) and stays
//! param-authoritative (ADR 0001 §5 / 0205). Applying a snapshot therefore
//! writes the topology fields and **leaves the engine's depths alone**; the
//! param re-sync that always accompanies a snapshot re-seeds them from the
//! store.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::engine::{MatrixEdit, MatrixField};
use crate::matrix::{DestId, MatrixSlot, MatrixTable, Polarity, Shape, SourceId};
use crate::params::Layer;

/// Ring capacity in records. A power of two so the index wrap is a mask.
///
/// Sized in **bytes**, not in gestures: the `Snapshot` variant dominates
/// [`TopoMsg`] (two 16-slot tables), so a web-sized 1024-slot ring would cost
/// ~400 kB per plugin instance to buy headroom nothing can consume. 64 records
/// is ~64 combo picks between two `process` calls — a couple of orders of
/// magnitude past what a hand can produce in a buffer period — and the overflow
/// path below is defined and tested for the case where it is not.
pub const TOPO_RING_SLOTS: usize = 64;

const RING_MASK: usize = TOPO_RING_SLOTS - 1;

// The mask *is* the wrap, so a non-power-of-two capacity would alias cells the
// `w - r >= TOPO_RING_SLOTS` guard believes are distinct — silent record
// corruption, not a test failure. Fail the build instead.
const _: () = assert!(TOPO_RING_SLOTS.is_power_of_two());

/// Filler for the ring's untouched cells. Never popped — a cell is only read
/// after the producer has written it — but the ring needs *some* initial value,
/// and an edit aimed at a slot index that cannot exist is the inertest one
/// available: [`apply_edit`] drops it on the floor.
const INERT: TopoMsg = TopoMsg::Edit(MatrixEdit {
    layer: Layer::L1,
    slot: u8::MAX,
    field: MatrixField::Enabled,
    value: 0,
});

/// One record on the topology channel.
///
/// `Copy` and heap-free by construction: the ring stores records by value, so
/// the audio thread's drain reads them out without touching the allocator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TopoMsg {
    /// One field of one slot on one layer — what the matrix overlay posts.
    Edit(MatrixEdit),
    /// Both layers' topology, applied wholesale: preset load, host state
    /// restore, copy/reset layer, or a resync after an overflow.
    Snapshot([MatrixTable; 2]),
}

/// Apply one UI topology edit to a table. Out-of-range slot indices are ignored.
///
/// The single decode point for the wire `u8`, shared by the store's
/// main-thread table and the audio thread's engine-side apply, so the two can
/// never disagree about what a record means.
pub fn apply_edit(table: &mut MatrixTable, edit: MatrixEdit) {
    let Some(slot) = table.slots.get_mut(edit.slot as usize) else {
        return;
    };
    match edit.field {
        MatrixField::Source => slot.source = SourceId::from_u8(edit.value),
        MatrixField::Dest => slot.dest = DestId::from_u8(edit.value),
        MatrixField::Polarity => slot.polarity = Polarity::from_u8(edit.value),
        MatrixField::Shape => slot.shape = Shape::from_u8(edit.value),
        MatrixField::ScaleSrc => slot.scale_src = SourceId::from_u8(edit.value),
        MatrixField::ScalePolarity => slot.scale_polarity = Polarity::from_u8(edit.value),
        MatrixField::ScaleShape => slot.scale_shape = Shape::from_u8(edit.value),
        MatrixField::Enabled => slot.enabled = edit.value != 0,
    }
}

/// Overwrite `dst`'s **topology** from `src`, leaving every depth as it was.
///
/// Depth is param-authoritative (0205); a snapshot carries whatever depths the
/// producer's table happened to hold, and the accompanying param re-sync is the
/// authority. Copying them here would let a stale mirror briefly win.
pub fn apply_snapshot(dst: &mut MatrixTable, src: &MatrixTable) {
    for (d, s) in dst.slots.iter_mut().zip(src.slots.iter()) {
        let depth = d.depth;
        *d = MatrixSlot { depth, ..*s };
    }
}

/// Single-producer / single-consumer ring of [`TopoMsg`], plus the sticky
/// resync flag its overflow policy needs.
///
/// **Discipline:** exactly one producer thread (the CLAP main thread — the
/// editor's controller tick, `state.load`, preset load) and exactly one
/// consumer (the audio thread, in `process`). Nothing here lets the producer
/// touch the read cursor or the consumer touch the write cursor, deliberately:
/// even the one place that wants to (`activate`, discarding records older than
/// the state it just adopted) goes through a *push* instead — a snapshot queued
/// behind the stale records supersedes them without reaching across.
#[derive(Debug)]
pub struct TopologyRing {
    /// Records. Written by the producer only under `write`'s claim, read by the
    /// consumer only under `read`'s, so the two never touch the same cell.
    slots: Box<[UnsafeCell<TopoMsg>]>,
    /// Monotonic push count (producer-owned). Wraps; only the difference and
    /// the low bits are ever used.
    write: AtomicUsize,
    /// Monotonic pop count (consumer-owned).
    read: AtomicUsize,
    /// A push was dropped (or is being deliberately withheld) and a full
    /// snapshot is owed. Producer-owned; the consumer never reads it.
    resync: AtomicBool,
}

// SAFETY: the `UnsafeCell` cells are the only non-`Sync` part. A cell is
// written solely by the producer, before `write` is published with `Release`,
// and read solely by the consumer, after it has observed that `write` with
// `Acquire` — and the producer will not reclaim a cell until the consumer has
// published the matching `read`. With the single-producer/single-consumer
// discipline documented above, no two threads ever access the same cell, and
// every cross-thread hand-off is ordered by the two counters.
unsafe impl Sync for TopologyRing {}

impl Default for TopologyRing {
    fn default() -> Self {
        Self::new()
    }
}

impl TopologyRing {
    /// An empty ring with no resync owed.
    pub fn new() -> Self {
        let slots = (0..TOPO_RING_SLOTS)
            .map(|_| UnsafeCell::new(INERT))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            slots,
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            resync: AtomicBool::new(false),
        }
    }

    /// Records queued but not yet drained.
    #[inline]
    pub fn len(&self) -> usize {
        self.write
            .load(Ordering::Relaxed)
            .wrapping_sub(self.read.load(Ordering::Relaxed))
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether a full snapshot is owed because a push was dropped.
    #[inline]
    pub fn resync_pending(&self) -> bool {
        self.resync.load(Ordering::Relaxed)
    }

    /// Raise the resync flag. Producer thread.
    #[inline]
    pub fn request_resync(&self) {
        self.resync.store(true, Ordering::Relaxed);
    }

    /// Lower the resync flag — the owed snapshot is queued. Producer thread.
    #[inline]
    pub fn clear_resync(&self) {
        self.resync.store(false, Ordering::Relaxed);
    }

    /// Push a record. Producer thread. `false` means the ring was full and the
    /// record was dropped — the caller owes a resync.
    pub fn try_push(&self, msg: TopoMsg) -> bool {
        let w = self.write.load(Ordering::Relaxed);
        let r = self.read.load(Ordering::Acquire);
        if w.wrapping_sub(r) >= TOPO_RING_SLOTS {
            return false;
        }
        // SAFETY: cell `w & RING_MASK` is outside the consumer's unread span
        // (checked above) and this is the only producer.
        unsafe { *self.slots[w & RING_MASK].get() = msg };
        self.write.store(w.wrapping_add(1), Ordering::Release);
        true
    }

    /// Pop the oldest record. Consumer (audio) thread. Wait-free: one relaxed
    /// load, one acquire load, one copy, one release store.
    pub fn pop(&self) -> Option<TopoMsg> {
        let r = self.read.load(Ordering::Relaxed);
        if r == self.write.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: cell `r & RING_MASK` was published by the producer's
        // `Release` store to `write`, which the `Acquire` load above observed,
        // and the producer cannot reclaim it until the store below.
        let msg = unsafe { *self.slots[r & RING_MASK].get() };
        self.read.store(r.wrapping_add(1), Ordering::Release);
        Some(msg)
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(slot: u8, value: u8) -> TopoMsg {
        TopoMsg::Edit(MatrixEdit {
            layer: Layer::L1,
            slot,
            field: MatrixField::Source,
            value,
        })
    }

    #[test]
    fn pushes_pop_in_order() {
        let ring = TopologyRing::new();
        assert!(ring.is_empty());
        for i in 0..8u8 {
            assert!(ring.try_push(edit(i, i)));
        }
        assert_eq!(ring.len(), 8);
        for i in 0..8u8 {
            assert_eq!(ring.pop(), Some(edit(i, i)));
        }
        assert_eq!(ring.pop(), None);
        assert!(ring.is_empty());
    }

    #[test]
    fn wraps_past_capacity_once_drained() {
        let ring = TopologyRing::new();
        // Three full laps, drained one record at a time: the index wrap must
        // not lose or duplicate a record.
        for i in 0..(3 * TOPO_RING_SLOTS) {
            let v = (i % 251) as u8;
            assert!(ring.try_push(edit(v, v)));
            assert_eq!(ring.pop(), Some(edit(v, v)));
        }
    }

    #[test]
    fn a_full_ring_refuses_the_push() {
        let ring = TopologyRing::new();
        for i in 0..TOPO_RING_SLOTS {
            assert!(ring.try_push(edit(i as u8, 1)), "record {i} should fit");
        }
        assert!(!ring.try_push(edit(0, 1)), "capacity + 1 must be refused");
        assert_eq!(ring.len(), TOPO_RING_SLOTS);
        // Draining one makes exactly one slot available again.
        assert!(ring.pop().is_some());
        assert!(ring.try_push(edit(0, 1)));
        assert!(!ring.try_push(edit(0, 1)));
    }

    #[test]
    fn the_resync_flag_is_sticky_until_cleared() {
        let ring = TopologyRing::new();
        assert!(!ring.resync_pending());
        ring.request_resync();
        assert!(ring.resync_pending());
        // Pushing and draining does not clear it — only the producer does, once
        // it has actually queued the snapshot it owes.
        assert!(ring.try_push(edit(1, 1)));
        assert!(ring.pop().is_some());
        assert!(ring.resync_pending());
        ring.clear_resync();
        assert!(!ring.resync_pending());
    }

    /// The scale VCA's polarity is its own field on the wire (0341), decoded
    /// into its own column. Both halves of that are worth pinning: an edit
    /// aimed at it must not land on the route's own polarity, and an edit aimed
    /// at the route must not land on the VCA's.
    #[test]
    fn a_scale_polarity_edit_lands_on_its_own_column() {
        let mut table = MatrixTable::default();
        apply_edit(
            &mut table,
            MatrixEdit {
                layer: Layer::L1,
                slot: 2,
                field: MatrixField::ScalePolarity,
                value: Polarity::Abs as u8,
            },
        );
        assert_eq!(table.slots[2].scale_polarity, Polarity::Abs);
        assert_eq!(table.slots[2].polarity, Polarity::None);

        apply_edit(
            &mut table,
            MatrixEdit {
                layer: Layer::L1,
                slot: 2,
                field: MatrixField::Polarity,
                value: Polarity::Bipolar as u8,
            },
        );
        assert_eq!(table.slots[2].polarity, Polarity::Bipolar);
        assert_eq!(table.slots[2].scale_polarity, Polarity::Abs);
    }

    #[test]
    fn apply_edit_ignores_an_out_of_range_slot() {
        let mut table = MatrixTable::default();
        let before = table;
        apply_edit(
            &mut table,
            MatrixEdit {
                layer: Layer::L1,
                slot: 99,
                field: MatrixField::Source,
                value: SourceId::Lfo2 as u8,
            },
        );
        assert_eq!(table, before);
    }

    #[test]
    fn apply_snapshot_takes_topology_and_keeps_depth() {
        let mut dst = MatrixTable::default();
        dst.slots[3].depth = 0.75;
        let mut src = MatrixTable::default();
        src.slots[3].source = SourceId::Lfo2;
        src.slots[3].dest = DestId::Cutoff;
        src.slots[3].enabled = true;
        src.slots[3].depth = -1.0;

        apply_snapshot(&mut dst, &src);
        assert_eq!(dst.slots[3].source, SourceId::Lfo2);
        assert_eq!(dst.slots[3].dest, DestId::Cutoff);
        assert!(dst.slots[3].enabled);
        assert_eq!(dst.slots[3].depth, 0.75, "depth stays param-authoritative");
    }

    /// A record must survive a real thread hand-off, not just a same-thread
    /// push/pop: the counters are the only synchronisation the audio thread has.
    #[test]
    fn records_cross_a_thread_boundary_in_order() {
        use std::sync::Arc;

        let ring = Arc::new(TopologyRing::new());
        let consumer = {
            let ring = Arc::clone(&ring);
            std::thread::spawn(move || {
                let mut seen = Vec::with_capacity(1000);
                while seen.len() < 1000 {
                    if let Some(msg) = ring.pop() {
                        seen.push(msg);
                    } else {
                        std::hint::spin_loop();
                    }
                }
                seen
            })
        };
        for i in 0..1000usize {
            let msg = edit((i % 16) as u8, (i % 251) as u8);
            while !ring.try_push(msg) {
                std::hint::spin_loop();
            }
        }
        let seen = consumer.join().expect("consumer thread");
        for (i, msg) in seen.iter().enumerate() {
            assert_eq!(*msg, edit((i % 16) as u8, (i % 251) as u8), "record {i}");
        }
    }
}
