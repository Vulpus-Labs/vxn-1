//! The main → audio-thread **topology channel** (ticket 0382).
//!
//! A port of vxn-1b's [`vxn1b_engine::topology`], not a redesign: the problem
//! is the same one, it was solved there in ticket 0338 and ADR 0003 §4, and a
//! second answer would only be a second thing to get wrong. What differs is the
//! shape of the table on either end — vxn-4 has one 48-slot matrix where vxn-1b
//! has two 16-slot ones, and no layer index on the wire.
//!
//! Matrix *topology* — source, dest, polarity, shape, scale source, scale
//! polarity, scale shape, enabled — is the part of a vxn-4 patch that is not a
//! scalar. It cannot ride the param atomics with the rest of the patch, and it
//! must not ride a mutex: a combo pick in a matrix editor would then route the
//! **audio thread** through a lock the editor holds, which is a priority
//! inversion that passes every test on an idle machine and drops out under
//! load. So it rides a single-producer / single-consumer ring of [`TopoMsg`]
//! records. The editor (main thread) pushes; the audio thread drains at the top
//! of `process` and applies each record straight onto the engine's own table.
//! Nothing on the audio side blocks, spins, or allocates.
//!
//! ## Two channels, not one
//!
//! Deliberately *not* "SPSC everywhere" (ADR 0003 §4, and the E052 design
//! note that repeats it for vxn-4):
//!
//! - **Values** — every scalar in [`crate::params`], which is to say almost the
//!   whole patch: operator fields, envelopes, the 64 authored PM depths, the
//!   sum-bus sends, the 48 slot depths and the patch trim. They stay in the
//!   idempotent atomic store. They are latest-wins, so a knob drag coalesces
//!   into one re-sync per block for free, and the faceplate needs to read them
//!   back off the main thread regardless.
//! - **Topology** rides this ring as [`TopoMsg::Edit`] — one field of one slot
//!   per record, so a single combo pick costs the audio thread one field write
//!   rather than a whole-patch rebuild.
//! - **Bulk** (preset load, `state.load`, adopting a factory entry) rides it as
//!   one [`TopoMsg::Snapshot`]: the whole 48-slot table in a single record,
//!   never decomposed into 48 slot edits. It travels *in* the ring purely so
//!   its ordering against pending edits is exact by construction, and it
//!   doubles as the overflow backstop below.
//!
//! `op-N-wave` is a value, not topology, even though a waveform selection reads
//! like one. It has a descriptor id and a range in [`crate::params`], so it has
//! an atomic; putting it on the ring as well would give one field two channels
//! and therefore an order to get wrong.
//!
//! ## Overflow is defined, not merely improbable
//!
//! Topology edits are human-rate and the audio thread drains the **whole** ring
//! every block, so [`TOPO_RING_SLOTS`] records can only pile up if the host
//! stops calling `process` while the editor is being driven. Unreachable by
//! argument is not the same as undefined, so: a push that finds the ring full
//! raises the sticky `resync` flag and drops the record. The producer then
//! publishes a full [`TopoMsg::Snapshot`] of the authoritative table as soon as
//! there is room ([`TopologyRing::resync_pending`] →
//! [`crate::shared::SharedParams::service_topology_resync`]), which subsumes
//! every dropped edit. While a resync is pending, individual edits are not
//! pushed at all — the snapshot that will carry them is taken from the table
//! *after* they were applied to it, so pushing them as well would be redundant
//! work whose only effect is to keep the ring full for longer.
//!
//! ## Depth does not travel here
//!
//! A slot's depth is a scalar with a descriptor id (`matrix-NN-depth`), so it
//! stays param-authoritative exactly as vxn-1b's does (ADR 0001 §5). Applying a
//! snapshot therefore writes the topology fields and **leaves the engine's
//! depths alone**; the param re-sync that always accompanies a snapshot
//! re-seeds them from the store. Copying them here as well would let whatever
//! the producer's mirror happened to hold briefly win over the atomics.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use vxn_core_matrix::curve::{Polarity, Shape};

use crate::matrix::{DestId, Matrix, SourceId};

/// Ring capacity in records. A power of two so the index wrap is a mask.
///
/// Sized in **bytes**, not in gestures. The `Snapshot` variant dominates
/// [`TopoMsg`] — vxn-4's 48-slot table is 576 bytes against vxn-1b's two
/// 16-slot ones at 384 — so every cell costs a snapshot whether or not it ever
/// holds one, and capacity buys headroom that nothing can consume. vxn-1b's 64
/// records come to ~25 kB per plugin instance; 32 of vxn-4's come to ~18 kB,
/// which is the same order for the same argument. 32 records is ~32 combo picks
/// between two `process` calls, two orders of magnitude past what a hand can
/// produce in a buffer period, and the overflow path above is defined and
/// tested for the case where it is not.
///
/// `the_ring_stays_the_size_this_argument_assumes` pins the record size, so a
/// wider slot or a longer table shows up as a test failure rather than as a
/// megabyte of ring per instance.
pub const TOPO_RING_SLOTS: usize = 32;

const RING_MASK: usize = TOPO_RING_SLOTS - 1;

// The mask *is* the wrap, so a non-power-of-two capacity would alias cells the
// `w - r >= TOPO_RING_SLOTS` guard believes are distinct — silent record
// corruption, not a test failure. Fail the build instead.
const _: () = assert!(TOPO_RING_SLOTS.is_power_of_two());

/// Which field of a matrix slot an [`Edit`](TopoMsg::Edit) names.
///
/// Exactly the slot's non-scalar columns. `depth` is absent on purpose: it is a
/// descriptor param and rides the atomics.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SlotField {
    Source,
    Dest,
    Polarity,
    Shape,
    ScaleSrc,
    ScalePolarity,
    ScaleShape,
    Enabled,
}

/// One field of one matrix slot, on the wire.
///
/// The value is a `u8` for every field, which holds because the widest of them
/// is [`DestId`] at 105 variants. That is close enough to the ceiling to be
/// worth naming: the day vxn-4 grows past 256 destinations this becomes a
/// silent truncation, and `every_dest_survives_the_wire` is the test that would
/// catch it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SlotEdit {
    pub slot: u8,
    pub field: SlotField,
    pub value: u8,
}

/// Filler for the ring's untouched cells. Never popped — a cell is only read
/// after the producer has written it — but the ring needs *some* initial value,
/// and an edit aimed at a slot index that cannot exist is the inertest one
/// available: [`apply_edit`] drops it on the floor.
const INERT: TopoMsg = TopoMsg::Edit(SlotEdit {
    slot: u8::MAX,
    field: SlotField::Enabled,
    value: 0,
});

/// One record on the topology channel.
///
/// `Copy` and heap-free by construction: the ring stores records by value, so
/// the audio thread's drain reads them out without touching the allocator.
///
/// The size difference between the arms is the whole point rather than an
/// oversight, which is why clippy's advice is declined here. Boxing `Snapshot`
/// would put a `Box` on the ring, and the pop that frees it runs on the **audio
/// thread** — trading ~18 kB of static footprint for an allocator call inside
/// `process`, which is the one thing this module exists to avoid. It would also
/// cost `Copy`, and `Copy` is what makes the cell read in [`TopologyRing::pop`]
/// a plain load rather than a move out of a shared cell.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TopoMsg {
    /// One field of one slot — what a matrix editor posts.
    Edit(SlotEdit),
    /// The whole table, applied wholesale: preset load, host state restore,
    /// adopting a factory entry, or a resync after an overflow.
    Snapshot(Matrix),
}

/// Apply one topology edit to a table. Out-of-range slot indices are ignored.
///
/// The single decode point for the wire `u8`, shared by the store's
/// main-thread table and the audio thread's engine-side apply, so the two can
/// never disagree about what a record means. Every decode degrades rather than
/// panics — `from_u8` falls back to the sentinel — because a record can arrive
/// from a preset file that a later build wrote.
pub fn apply_edit(table: &mut Matrix, edit: SlotEdit) {
    let Some(slot) = table.slots.get_mut(edit.slot as usize) else {
        return;
    };
    match edit.field {
        SlotField::Source => slot.source = SourceId::from_u8(edit.value),
        SlotField::Dest => slot.dest = DestId::from_u8(edit.value),
        SlotField::Polarity => slot.polarity = Polarity::from_u8(edit.value),
        SlotField::Shape => slot.shape = Shape::from_u8(edit.value),
        SlotField::ScaleSrc => slot.scale_src = SourceId::from_u8(edit.value),
        SlotField::ScalePolarity => slot.scale_polarity = Polarity::from_u8(edit.value),
        SlotField::ScaleShape => slot.scale_shape = Shape::from_u8(edit.value),
        SlotField::Enabled => slot.enabled = edit.value != 0,
    }
}

/// Overwrite `dst`'s **topology** from `src`, leaving every depth as it was.
///
/// See the module docs: depth is param-authoritative, so a snapshot carries
/// whatever depths the producer's mirror happened to hold and the accompanying
/// param re-sync is the authority.
pub fn apply_snapshot(dst: &mut Matrix, src: &Matrix) {
    for (d, s) in dst.slots.iter_mut().zip(src.slots.iter()) {
        let depth = d.depth;
        *d = vxn_core_matrix::slot::MatrixSlot { depth, ..*s };
    }
}

/// Single-producer / single-consumer ring of [`TopoMsg`], plus the sticky
/// resync flag its overflow policy needs.
///
/// **Discipline:** exactly one producer thread (the main thread — an editor
/// tick, `state.load`, a preset load) and exactly one consumer (the audio
/// thread, in `process`). Nothing here lets the producer touch the read cursor
/// or the consumer touch the write cursor, deliberately: even the one place
/// that wants to — adopting a table wholesale and discarding the records older
/// than it — goes through a *push* instead, a snapshot queued behind the stale
/// records that supersedes them without reaching across.
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
// published the matching `read`. With the single-producer / single-consumer
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
        TopoMsg::Edit(SlotEdit {
            slot,
            field: SlotField::Source,
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

    /// [`TOPO_RING_SLOTS`] is argued in bytes, so the bytes are worth pinning.
    /// A wider slot or a longer table would multiply straight through into the
    /// ring's footprint, per plugin instance, silently.
    #[test]
    fn the_ring_stays_the_size_this_argument_assumes() {
        use std::mem::size_of;
        assert_eq!(size_of::<Matrix>(), 576, "the snapshot arm changed size");
        assert!(
            size_of::<TopoMsg>() * TOPO_RING_SLOTS < 32 * 1024,
            "the ring is {} bytes per plugin instance",
            size_of::<TopoMsg>() * TOPO_RING_SLOTS
        );
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

    /// Every field lands in its own column. The scale VCA's polarity is the one
    /// worth pinning in both directions — an edit aimed at it must not land on
    /// the route's own polarity, and vice versa.
    #[test]
    fn each_field_lands_in_its_own_column() {
        let mut table = Matrix::default();
        let put = |t: &mut Matrix, field, value| {
            apply_edit(
                t,
                SlotEdit {
                    slot: 2,
                    field,
                    value,
                },
            )
        };

        put(&mut table, SlotField::ScalePolarity, Polarity::Abs as u8);
        assert_eq!(table.slots[2].scale_polarity, Polarity::Abs);
        assert_eq!(table.slots[2].polarity, Polarity::None);

        put(&mut table, SlotField::Polarity, Polarity::Bipolar as u8);
        assert_eq!(table.slots[2].polarity, Polarity::Bipolar);
        assert_eq!(table.slots[2].scale_polarity, Polarity::Abs);

        put(&mut table, SlotField::Source, SourceId::Macro3 as u8);
        put(&mut table, SlotField::ScaleSrc, SourceId::Macro7 as u8);
        put(&mut table, SlotField::Dest, DestId::Damp5 as u8);
        put(&mut table, SlotField::Shape, Shape::Exp as u8);
        put(&mut table, SlotField::Enabled, 1);
        let s = table.slots[2];
        assert_eq!(s.source, SourceId::Macro3);
        assert_eq!(s.scale_src, SourceId::Macro7);
        assert_eq!(s.dest, DestId::Damp5);
        assert_eq!(s.shape, Shape::Exp);
        assert!(s.enabled);
        assert_eq!(s.depth, 0.0, "depth is a param and never rides the ring");
    }

    /// The wire byte holds every destination vxn-4 has. 105 variants is close
    /// enough to 256 that a future family could silently truncate.
    #[test]
    fn every_dest_survives_the_wire() {
        for &d in DestId::ALL.iter() {
            let code = d as usize;
            assert!(code <= u8::MAX as usize, "{d:?} does not fit the wire byte");
            assert_eq!(DestId::from_u8(code as u8), d);
        }
        for &s in SourceId::ALL.iter() {
            assert_eq!(SourceId::from_u8(s as u8), s);
        }
    }

    #[test]
    fn apply_edit_ignores_an_out_of_range_slot() {
        let mut table = Matrix::default();
        let before = table;
        apply_edit(
            &mut table,
            SlotEdit {
                slot: 99,
                field: SlotField::Source,
                value: SourceId::Macro2 as u8,
            },
        );
        assert_eq!(table, before);
    }

    #[test]
    fn apply_snapshot_takes_topology_and_keeps_depth() {
        let mut dst = Matrix::default();
        dst.slots[3].depth = 0.75;
        let mut src = Matrix::default();
        src.slots[3].source = SourceId::Macro2;
        src.slots[3].dest = DestId::Damp0;
        src.slots[3].enabled = true;
        src.slots[3].depth = -1.0;

        apply_snapshot(&mut dst, &src);
        assert_eq!(dst.slots[3].source, SourceId::Macro2);
        assert_eq!(dst.slots[3].dest, DestId::Damp0);
        assert!(dst.slots[3].enabled);
        assert_eq!(dst.slots[3].depth, 0.75, "depth stays param-authoritative");
    }

    /// A record must survive a real thread hand-off, not just a same-thread
    /// push/pop: the two counters are the only synchronisation the audio thread
    /// has, and a same-thread test exercises neither ordering.
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
            let msg = edit((i % 48) as u8, (i % 251) as u8);
            while !ring.try_push(msg) {
                std::hint::spin_loop();
            }
        }
        let seen = consumer.join().expect("consumer thread");
        for (i, msg) in seen.iter().enumerate() {
            assert_eq!(*msg, edit((i % 48) as u8, (i % 251) as u8), "record {i}");
        }
    }
}
