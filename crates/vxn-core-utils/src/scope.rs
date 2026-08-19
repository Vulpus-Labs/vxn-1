//! Lock-free waveform-capture ring for an oscilloscope display.
//!
//! The audio→UI path for a scope trace, and the sibling of [`crate::meter`]: a
//! [`ScopeBus`] is a power-of-two ring of `f32` slots held as `AtomicU32` bit
//! patterns. The audio thread appends one mono sample per base-rate frame; the
//! UI thread reads the most recent window back at display cadence.
//!
//! ## Why a ring rather than a queue
//!
//! A scope wants "the last N milliseconds *as of now*", not "every sample since
//! you last looked". A queue would either back up (the UI reads ~30×/s, the
//! audio thread writes tens of thousands) or need a drop policy that decides
//! which samples to lose. A ring answers the display's actual question
//! directly, needs no capacity negotiation, and a reader that falls behind
//! simply sees newer data — which is what it wanted anyway.
//!
//! ## Reads are deliberately unsynchronised
//!
//! The reader takes a snapshot of the write counter and walks backwards. A
//! block landing mid-read can overwrite the oldest slots of the window, so a
//! frame may splice two capture instants together. That is bounded by ring
//! length (with the defaults below, the writer needs ~5× the window's duration
//! to lap a reader), costs the display at worst one visibly-glitched frame at
//! 30 Hz, and buys a wait-free audio side: one relaxed store per sample and one
//! per block for the counter. Locking or double-buffering to remove it would
//! put the audio thread in the display's way for no audible or visible gain.
//!
//! ## Source selection
//!
//! One bus captures one signal at a time, chosen by the view
//! ([`ScopeBus::set_source`]). The alternative — a ring per tappable point —
//! costs the audio thread a write per tap for signals nobody is looking at,
//! and the wire a frame per tap. Source `0` is "off": nothing is captured and
//! nothing is published, which is the state whenever the editor is closed.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};

/// Ring capacity in samples. A power of two so the index wrap is a mask.
///
/// 8192 base-rate samples is ~170 ms at 48 kHz — a little over five times the
/// default read window, which is the headroom the unsynchronised read above
/// relies on.
pub const SCOPE_RING_SAMPLES: usize = 8192;

const RING_MASK: u64 = (SCOPE_RING_SAMPLES - 1) as u64;

/// "Capture nothing" — the resting source, and what the view selects when the
/// scope is not on screen.
pub const SCOPE_SOURCE_OFF: u8 = 0;

/// A ring of atomically-published mono samples, shared between the audio thread
/// (writer) and the UI thread (reader) behind an `Arc`.
#[derive(Debug)]
pub struct ScopeBus {
    ring: Box<[AtomicU32]>,
    /// Total samples ever written. Also the write cursor (`& RING_MASK`).
    /// Published once per block, not once per sample.
    write: AtomicU64,
    /// Which signal is being captured; `0` = off. Written by the UI thread,
    /// read by the audio thread on every publish.
    source: AtomicU8,
}

impl Default for ScopeBus {
    fn default() -> Self {
        Self::new()
    }
}

impl ScopeBus {
    /// An empty bus with no source selected.
    pub fn new() -> Self {
        Self {
            ring: (0..SCOPE_RING_SAMPLES).map(|_| AtomicU32::new(0)).collect(),
            write: AtomicU64::new(0),
            source: AtomicU8::new(SCOPE_SOURCE_OFF),
        }
    }

    /// Select the captured signal. Changing it clears the ring, so the trace
    /// can never splice the tail of one source onto the head of another.
    ///
    /// A block already in flight can restore the pre-clear sample count a
    /// moment later (it holds its own cursor), which shows the reader one
    /// window of the zeros written here — a flat frame, not another source's
    /// audio, so the invariant that matters holds.
    pub fn set_source(&self, source: u8) {
        if self.source.swap(source, Ordering::Relaxed) != source {
            self.clear();
        }
    }

    /// The currently selected source.
    #[inline]
    pub fn source(&self) -> u8 {
        self.source.load(Ordering::Relaxed)
    }

    /// Zero the ring and restart the sample count, so reads report
    /// "not enough data yet" until a fresh window has accumulated.
    pub fn clear(&self) {
        for slot in self.ring.iter() {
            slot.store(0, Ordering::Relaxed);
        }
        self.write.store(0, Ordering::Relaxed);
    }

    /// Append a stereo block as mono, taking every `stride`-th frame.
    ///
    /// No-op unless `source` is the selected one — which is what keeps an
    /// unwatched tap free, and the whole bus free while the editor is closed.
    ///
    /// `stride` exists for oversampled callers: a tap sitting inside an N×
    /// region hands the oversampled buffers with `stride = N` and the ring
    /// stays at the base rate, so the trace's time axis doesn't change when
    /// oversampling does. Point-sampling rather than filtering is deliberate —
    /// this is a display, and a decimation filter would cost the audio thread
    /// real work for pixels.
    #[inline]
    pub fn publish_stride(&self, source: u8, left: &[f32], right: &[f32], stride: usize) {
        if source == SCOPE_SOURCE_OFF || self.source.load(Ordering::Relaxed) != source {
            return;
        }
        let stride = stride.max(1);
        let n = left.len().min(right.len());
        let mut w = self.write.load(Ordering::Relaxed);
        let mut i = 0;
        while i < n {
            let s = (left[i] + right[i]) * 0.5;
            // A non-finite sample would draw as a gap or blow the trace off the
            // canvas; the engine's own guard catches these at the output, but
            // this tap sits upstream of it.
            let bits = if s.is_finite() { s.to_bits() } else { 0 };
            self.ring[(w & RING_MASK) as usize].store(bits, Ordering::Relaxed);
            w += 1;
            i += stride;
        }
        self.write.store(w, Ordering::Relaxed);
    }

    /// Copy the most recent `window` samples, taking every `decimation`-th, into
    /// `out` in oldest → newest order.
    ///
    /// Returns `false` — leaving `out` untouched — until the ring holds a full
    /// window, so a freshly-cleared bus reports "nothing yet" instead of
    /// drawing a half-filled trace.
    ///
    /// Read-time decimation (rather than storing pre-thinned samples) keeps the
    /// time base the display's choice: the same ring serves a slow trace and a
    /// zoomed-in one.
    pub fn read_window(&self, decimation: usize, window: usize, out: &mut Vec<f32>) -> bool {
        let dec = decimation.max(1);
        let win = window.min(SCOPE_RING_SAMPLES / dec);
        if win == 0 {
            return false;
        }
        let span = (dec * win) as u64;
        let w = self.write.load(Ordering::Relaxed);
        if w < span {
            return false;
        }
        out.clear();
        out.reserve(win);
        for k in 0..win {
            // `w` is one past the newest sample, so the newest is `w - 1`.
            let back = ((win - 1 - k) * dec + 1) as u64;
            let slot = ((w - back) & RING_MASK) as usize;
            out.push(f32::from_bits(self.ring[slot].load(Ordering::Relaxed)));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: u8 = 1;

    fn armed() -> ScopeBus {
        let bus = ScopeBus::new();
        bus.set_source(SRC);
        bus
    }

    #[test]
    fn nothing_is_captured_until_a_source_is_selected() {
        let bus = ScopeBus::new();
        bus.publish_stride(SRC, &[1.0; 4096], &[1.0; 4096], 1);
        let mut out = Vec::new();
        assert!(!bus.read_window(1, 64, &mut out), "off bus must capture nothing");
    }

    #[test]
    fn a_different_source_is_ignored() {
        let bus = armed();
        bus.publish_stride(SRC + 1, &[1.0; 4096], &[1.0; 4096], 1);
        let mut out = Vec::new();
        assert!(!bus.read_window(1, 64, &mut out));
    }

    #[test]
    fn read_reports_nothing_until_a_full_window_exists() {
        let bus = armed();
        let mut out = Vec::new();
        bus.publish_stride(SRC, &[0.5; 31], &[0.5; 31], 1);
        assert!(!bus.read_window(1, 32, &mut out), "31 of 32 samples is not a window");
        bus.publish_stride(SRC, &[0.5; 1], &[0.5; 1], 1);
        assert!(bus.read_window(1, 32, &mut out));
        assert_eq!(out.len(), 32);
    }

    #[test]
    fn the_window_is_the_latest_samples_oldest_first() {
        let bus = armed();
        let l: Vec<f32> = (0..100).map(|i| i as f32).collect();
        bus.publish_stride(SRC, &l, &l, 1);
        let mut out = Vec::new();
        assert!(bus.read_window(1, 4, &mut out));
        assert_eq!(out, vec![96.0, 97.0, 98.0, 99.0]);
    }

    #[test]
    fn stereo_folds_to_mono() {
        let bus = armed();
        bus.publish_stride(SRC, &[1.0; 64], &[0.0; 64], 1);
        let mut out = Vec::new();
        assert!(bus.read_window(1, 8, &mut out));
        assert!(out.iter().all(|&s| s == 0.5), "L=1, R=0 should read 0.5");
    }

    #[test]
    fn stride_thins_an_oversampled_block_to_the_base_rate() {
        let bus = armed();
        let l: Vec<f32> = (0..64).map(|i| i as f32).collect();
        // 4× oversampled: only frames 0, 4, 8 … are captured, so 64 samples in
        // yields 16 out and the values step by 4.
        bus.publish_stride(SRC, &l, &l, 4);
        let mut out = Vec::new();
        assert!(bus.read_window(1, 16, &mut out));
        assert_eq!(out.first(), Some(&0.0));
        assert_eq!(out.last(), Some(&60.0));
        assert!(!bus.read_window(1, 17, &mut out), "only 16 frames were captured");
    }

    #[test]
    fn decimation_strides_the_read_and_still_ends_on_the_newest_sample() {
        let bus = armed();
        let l: Vec<f32> = (0..100).map(|i| i as f32).collect();
        bus.publish_stride(SRC, &l, &l, 1);
        let mut out = Vec::new();
        assert!(bus.read_window(4, 4, &mut out));
        assert_eq!(out, vec![87.0, 91.0, 95.0, 99.0]);
    }

    #[test]
    fn the_ring_wraps_and_keeps_only_the_newest_samples() {
        let bus = armed();
        let block: Vec<f32> = (0..SCOPE_RING_SAMPLES + 16).map(|i| i as f32).collect();
        bus.publish_stride(SRC, &block, &block, 1);
        let mut out = Vec::new();
        assert!(bus.read_window(1, 4, &mut out));
        let last = (SCOPE_RING_SAMPLES + 15) as f32;
        assert_eq!(out, vec![last - 3.0, last - 2.0, last - 1.0, last]);
    }

    #[test]
    fn a_read_wider_than_the_ring_is_clamped_not_panicked() {
        let bus = armed();
        let block = vec![0.25f32; SCOPE_RING_SAMPLES];
        bus.publish_stride(SRC, &block, &block, 1);
        let mut out = Vec::new();
        assert!(bus.read_window(4, SCOPE_RING_SAMPLES, &mut out));
        assert_eq!(out.len(), SCOPE_RING_SAMPLES / 4);
    }

    #[test]
    fn changing_source_clears_the_trace() {
        let bus = armed();
        let block = vec![1.0f32; 256];
        bus.publish_stride(SRC, &block, &block, 1);
        let mut out = Vec::new();
        assert!(bus.read_window(1, 64, &mut out));
        // Switching taps must not leave the previous signal on screen.
        bus.set_source(SRC + 1);
        assert!(!bus.read_window(1, 64, &mut out));
    }

    #[test]
    fn reselecting_the_same_source_is_not_a_clear() {
        let bus = armed();
        let block = vec![1.0f32; 256];
        bus.publish_stride(SRC, &block, &block, 1);
        bus.set_source(SRC);
        let mut out = Vec::new();
        assert!(bus.read_window(1, 64, &mut out), "an idempotent set must not blank the trace");
    }

    #[test]
    fn non_finite_samples_are_written_as_silence() {
        let bus = armed();
        bus.publish_stride(SRC, &[f32::NAN; 64], &[f32::INFINITY; 64], 1);
        let mut out = Vec::new();
        assert!(bus.read_window(1, 8, &mut out));
        assert!(out.iter().all(|s| *s == 0.0), "NaN/inf must not reach the display");
    }
}
