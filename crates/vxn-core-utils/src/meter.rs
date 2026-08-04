//! Lock-free metering bus (ticket 0240).
//!
//! The audio→UI path for level meters. A [`MeterBus`] is a fixed table of `f32`
//! slots held as `AtomicU32` bit patterns: the audio thread folds each block's
//! extreme into a slot, and the UI thread reads-and-clears it once per frame.
//!
//! ## Why peak-since-last-read, not last-block-peak
//!
//! The two threads run at unrelated rates — audio blocks are typically 64–512
//! samples (0.1–10 ms) while the editor ticks at ~60 Hz (16 ms). A slot that
//! simply *stored* each block's peak would be overwritten several times between
//! UI reads, so a transient landing in any block but the last one before a tick
//! would never be displayed. Accumulating the extreme and clearing on read makes
//! the published value "the loudest thing that happened since you last looked",
//! which is exactly what a peak meter should show and is correct for **any**
//! ratio of block rate to frame rate.
//!
//! Averaging instead (RMS-style) would smear attacks and needs a window length
//! the audio thread would have to own; the ballistics belong in the view, where
//! they can be tuned without touching DSP. The bus publishes raw extremes only.
//!
//! ## Cost
//!
//! One relaxed atomic read-modify-write per channel per block. No allocation, no
//! locks, no queue (and so no queue-full policy to get wrong): metering is
//! latest-value-wins, and a dropped intermediate value is *by construction*
//! folded into the one that replaces it.
//!
//! ## Two accumulation modes
//!
//! - [`MeterBus::publish_peak`] — atomic **max**, for levels. Slots rest at
//!   `0.0`, so a silent interval reads exactly `0.0` and the view's decay drives
//!   the bar down instead of a stale value sticking.
//! - [`MeterBus::publish_reduction`] — atomic **min**, for gain reduction in dB
//!   (most negative = deepest). Slots also rest at `0.0` — "no reduction" — so a
//!   bypassed compressor that never publishes reads as no reduction, which is
//!   the truthful display.

use std::sync::atomic::{AtomicU32, Ordering};

/// Named slots in a [`MeterBus`]. Discriminant = slot index; stereo taps take
/// two consecutive slots (`L` then `R`) so a pair reads as `tap as usize` and
/// `tap as usize + 1`.
///
/// The table is sized once at [`MeterTap::COUNT`]; adding a tap is additive (a
/// new variant before `COUNT` is computed), and consumers that don't publish to
/// a slot simply leave it resting at `0.0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum MeterTap {
    /// Layer 1 post-fader, left channel (`+1` = right).
    Layer1L = 0,
    Layer1R = 1,
    /// Layer 2 post-fader, left channel (`+1` = right).
    Layer2L = 2,
    Layer2R = 3,
    /// Dynamics slot input, left channel (`+1` = right).
    DynamicsInL = 4,
    DynamicsInR = 5,
    /// Dynamics slot output, left channel (`+1` = right) — post comp/sat and
    /// post the bypass crossfade, so it reads what the slot actually hands on.
    DynamicsOutL = 6,
    DynamicsOutR = 7,
    /// Dynamics gain reduction in dB — **one** slot, not two: the kernel runs a
    /// single stereo-linked detector, so a second channel would duplicate it.
    /// Published with [`MeterBus::publish_reduction`].
    DynamicsGr = 8,
    /// Master output, left channel (`+1` = right). Post master volume.
    MasterL = 9,
    MasterR = 10,
}

impl MeterTap {
    /// Number of slots a [`MeterBus`] holds.
    pub const COUNT: usize = MeterTap::MasterR as usize + 1;

    /// Slot index.
    #[inline]
    pub const fn idx(self) -> usize {
        self as usize
    }
}

/// A fixed table of atomically-published meter values.
///
/// Shared between the audio thread (publisher) and the UI thread (drain) behind
/// an `Arc`. Every operation is wait-free.
#[derive(Debug)]
pub struct MeterBus {
    slots: [AtomicU32; MeterTap::COUNT],
}

impl Default for MeterBus {
    fn default() -> Self {
        Self::new()
    }
}

impl MeterBus {
    /// A bus with every slot at rest (`0.0`).
    pub fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }

    /// Fold `value` into `tap` keeping the **larger** magnitude — the level-meter
    /// path. `value` is treated as a magnitude; negatives and non-finite inputs
    /// are ignored so a NaN in the audio path can never poison the display.
    ///
    /// Relaxed ordering throughout: the slot carries no companion data, so there
    /// is nothing to order it against. A `fetch_update` CAS loop is used rather
    /// than a plain store because two publishers into one slot (or a drain
    /// racing a publish) must not lose the larger value.
    #[inline]
    pub fn publish_peak(&self, tap: MeterTap, value: f32) {
        if !value.is_finite() || value <= 0.0 {
            return;
        }
        let slot = &self.slots[tap.idx()];
        let _ = slot.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |bits| {
            let current = f32::from_bits(bits);
            (value > current).then(|| value.to_bits())
        });
    }

    /// Fold `db` into `tap` keeping the **more negative** value — the gain-
    /// reduction path. `0.0` means "no reduction"; positive values are ignored
    /// (makeup gain is not reduction), as are non-finite ones.
    #[inline]
    pub fn publish_reduction(&self, tap: MeterTap, db: f32) {
        if !db.is_finite() || db >= 0.0 {
            return;
        }
        let slot = &self.slots[tap.idx()];
        let _ = slot.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |bits| {
            let current = f32::from_bits(bits);
            (db < current).then(|| db.to_bits())
        });
    }

    /// Read `tap` and reset it to rest. The UI thread's per-frame drain.
    ///
    /// Clearing here is what makes the accumulation window "since the last
    /// frame" rather than "since the plugin loaded" — without it a peak would
    /// latch forever.
    #[inline]
    pub fn take(&self, tap: MeterTap) -> f32 {
        f32::from_bits(self.slots[tap.idx()].swap(0, Ordering::Relaxed))
    }

    /// Read `tap` without clearing. Tests and diagnostics; the UI drain uses
    /// [`Self::take`].
    #[inline]
    pub fn peek(&self, tap: MeterTap) -> f32 {
        f32::from_bits(self.slots[tap.idx()].load(Ordering::Relaxed))
    }

    /// Drain every slot into `out` in [`MeterTap`] index order, resetting each.
    /// One pass per UI frame, so a frame's values are mutually consistent
    /// (each is "since the previous frame") without holding a lock.
    #[inline]
    pub fn drain_into(&self, out: &mut [f32; MeterTap::COUNT]) {
        for (i, slot) in self.slots.iter().enumerate() {
            out[i] = f32::from_bits(slot.swap(0, Ordering::Relaxed));
        }
    }

    /// Reset every slot. Used on `reset()` / deactivate so a re-activated
    /// plugin doesn't display a peak from its previous life.
    #[inline]
    pub fn clear(&self) {
        for slot in &self.slots {
            slot.store(0, Ordering::Relaxed);
        }
    }

    /// Fold a stereo block's peaks into a consecutive `(L, R)` tap pair in one
    /// call — the shape every level tap uses. `left`/`right` are scanned for
    /// their maximum absolute sample.
    ///
    /// `tap` must be the **left** slot of a pair; the right is `tap + 1`.
    #[inline]
    pub fn publish_block_peak(&self, tap: MeterTap, left: &[f32], right: &[f32]) {
        let peak = |b: &[f32]| b.iter().fold(0.0f32, |a, &s| a.max(s.abs()));
        let l = tap.idx();
        self.publish_peak_at(l, peak(left));
        self.publish_peak_at(l + 1, peak(right));
    }

    /// [`Self::publish_peak`] addressed by raw slot index — the pair helper's
    /// inner step, so it can reach `tap + 1` without a reverse index → enum map.
    #[inline]
    fn publish_peak_at(&self, index: usize, value: f32) {
        if !value.is_finite() || value <= 0.0 || index >= MeterTap::COUNT {
            return;
        }
        let _ = self.slots[index].fetch_update(Ordering::Relaxed, Ordering::Relaxed, |bits| {
            let current = f32::from_bits(bits);
            (value > current).then(|| value.to_bits())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_holds_the_max_across_many_publishes() {
        // The core contract: several audio blocks between two UI reads must
        // surface the loudest of them, not the last.
        let bus = MeterBus::new();
        for v in [0.2, 0.9, 0.4, 0.1] {
            bus.publish_peak(MeterTap::MasterL, v);
        }
        assert_eq!(bus.take(MeterTap::MasterL), 0.9);
    }

    #[test]
    fn take_clears_the_slot() {
        let bus = MeterBus::new();
        bus.publish_peak(MeterTap::MasterL, 0.5);
        assert_eq!(bus.take(MeterTap::MasterL), 0.5);
        // Second read with no publishes in between reads silence, so the view's
        // decay takes over instead of a stale value latching.
        assert_eq!(bus.take(MeterTap::MasterL), 0.0);
    }

    #[test]
    fn slots_are_independent() {
        let bus = MeterBus::new();
        bus.publish_peak(MeterTap::Layer1L, 0.7);
        bus.publish_peak(MeterTap::Layer2R, 0.3);
        assert_eq!(bus.peek(MeterTap::Layer1L), 0.7);
        assert_eq!(bus.peek(MeterTap::Layer1R), 0.0);
        assert_eq!(bus.peek(MeterTap::Layer2R), 0.3);
        assert_eq!(bus.peek(MeterTap::MasterL), 0.0);
    }

    #[test]
    fn non_finite_and_negative_peaks_are_ignored() {
        // One NaN in the audio path must not poison the display — the engine's
        // finite-guard replaces NaN with silence, but the bus refuses it too.
        let bus = MeterBus::new();
        bus.publish_peak(MeterTap::MasterL, 0.4);
        bus.publish_peak(MeterTap::MasterL, f32::NAN);
        bus.publish_peak(MeterTap::MasterL, f32::INFINITY);
        bus.publish_peak(MeterTap::MasterL, -9.0);
        assert_eq!(bus.take(MeterTap::MasterL), 0.4);
    }

    #[test]
    fn reduction_keeps_the_most_negative() {
        let bus = MeterBus::new();
        for db in [-3.0, -11.5, -1.0] {
            bus.publish_reduction(MeterTap::DynamicsGr, db);
        }
        assert_eq!(bus.take(MeterTap::DynamicsGr), -11.5);
        // Rest is 0 dB — "no reduction" — so a bypassed compressor that never
        // publishes reads truthfully rather than holding its last value.
        assert_eq!(bus.take(MeterTap::DynamicsGr), 0.0);
    }

    #[test]
    fn reduction_ignores_positive_and_non_finite() {
        let bus = MeterBus::new();
        bus.publish_reduction(MeterTap::DynamicsGr, -2.0);
        bus.publish_reduction(MeterTap::DynamicsGr, 6.0); // makeup, not reduction
        bus.publish_reduction(MeterTap::DynamicsGr, f32::NAN);
        assert_eq!(bus.take(MeterTap::DynamicsGr), -2.0);
    }

    #[test]
    fn block_peak_fills_a_stereo_pair() {
        let bus = MeterBus::new();
        // Negative samples count by magnitude; the pair addresses L then L+1.
        bus.publish_block_peak(MeterTap::Layer1L, &[0.1, -0.8, 0.3], &[0.2, 0.25]);
        assert_eq!(bus.peek(MeterTap::Layer1L), 0.8);
        assert_eq!(bus.peek(MeterTap::Layer1R), 0.25);
    }

    #[test]
    fn drain_reads_and_clears_every_slot_in_index_order() {
        let bus = MeterBus::new();
        bus.publish_peak(MeterTap::Layer1L, 0.5);
        bus.publish_peak(MeterTap::MasterR, 0.6);
        bus.publish_reduction(MeterTap::DynamicsGr, -4.0);

        let mut frame = [0.0f32; MeterTap::COUNT];
        bus.drain_into(&mut frame);
        assert_eq!(frame[MeterTap::Layer1L.idx()], 0.5);
        assert_eq!(frame[MeterTap::MasterR.idx()], 0.6);
        assert_eq!(frame[MeterTap::DynamicsGr.idx()], -4.0);

        let mut again = [1.0f32; MeterTap::COUNT];
        bus.drain_into(&mut again);
        assert_eq!(again, [0.0; MeterTap::COUNT]);
    }

    #[test]
    fn clear_resets_everything() {
        let bus = MeterBus::new();
        bus.publish_peak(MeterTap::Layer1L, 0.9);
        bus.publish_reduction(MeterTap::DynamicsGr, -7.0);
        bus.clear();
        assert_eq!(bus.peek(MeterTap::Layer1L), 0.0);
        assert_eq!(bus.peek(MeterTap::DynamicsGr), 0.0);
    }

    #[test]
    fn publish_is_wait_free_across_threads() {
        // Two publishers racing one slot must not lose the larger value — the
        // reason publish is a CAS loop rather than a compare-then-store.
        use std::sync::Arc;
        let bus = Arc::new(MeterBus::new());
        let handles: Vec<_> = (0..4)
            .map(|t| {
                let bus = bus.clone();
                std::thread::spawn(move || {
                    for i in 0..1000 {
                        bus.publish_peak(MeterTap::MasterL, (t * 1000 + i) as f32 / 8000.0);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        // The largest value any thread published is t=3, i=999 → 3999/8000.
        assert_eq!(bus.peek(MeterTap::MasterL), 3999.0 / 8000.0);
    }
}
