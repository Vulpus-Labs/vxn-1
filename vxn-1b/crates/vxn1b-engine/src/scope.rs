//! Scope frames — the audio→view half of the oscilloscope spine.
//!
//! The capture ring itself is shared infrastructure
//! ([`vxn_core_utils::ScopeBus`]); what lives here is VXN1b's **tap vocabulary**
//! (which signal the ring is pointed at), the **frame** the UI thread reads out
//! of it, and the op the faceplate posts to re-point it.
//!
//! ## Path
//!
//! ```text
//! audio thread                main thread (~30 Hz)              page
//! ────────────                ────────────────────              ────
//! publish_stride(tap, …)  →   ScopeFrame::read(&bus)        →   ev.kind === 'scope'
//!   (one relaxed store            (latest window, decimated)       panels/scope.js
//!    per base-rate frame)
//! ```
//!
//! Like the meter frame it rides the existing per-tick `ViewEvent` batch as a
//! `ViewEvent::Custom` payload, so it costs no extra `evaluate_script` and needs
//! no new bridge channel.
//!
//! ## Why one tap at a time
//!
//! The scope shows the **edit layer's** output, and the faceplate only ever
//! displays one. Pointing the single ring at the layer the player is looking at
//! (via [`ScopeOp`]) keeps the audio thread's cost to the watched signal alone
//! and the wire to one frame, instead of capturing and shipping both layers so
//! the page can throw one away.

use vxn_core_utils::{SCOPE_SOURCE_OFF, ScopeBus};

/// What the capture ring is pointed at.
///
/// Wire-encoded as the discriminant, so the faceplate's `set_scope_source`
/// opcode and the audio thread's publish guard share one vocabulary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum ScopeTap {
    /// Capture nothing. The resting state: no scope on screen (the FX/Global
    /// tab), or no editor open at all.
    #[default]
    Off = SCOPE_SOURCE_OFF,
    /// Layer 1's post-fader, post-pan output — the same point the L1 meter
    /// reads, so the trace and the bar agree about what the layer contributes.
    Layer1 = 1,
    /// Layer 2's post-fader, post-pan output. Rests silent in single mode:
    /// synth 2 is never ticked, so nothing publishes.
    Layer2 = 2,
}

impl ScopeTap {
    /// Wire byte for this tap.
    #[inline]
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// Decode a wire byte. Unknown values decode to [`ScopeTap::Off`] rather
    /// than erroring — a page from a newer build asking for a tap this engine
    /// doesn't have should go quiet, not break the editor.
    pub fn from_code(code: u8) -> Self {
        match code {
            1 => ScopeTap::Layer1,
            2 => ScopeTap::Layer2,
            _ => ScopeTap::Off,
        }
    }

}

/// UI→engine op: re-point the capture ring. Not a param and not `KeyState` —
/// it is pure view state (which panel is on screen), so it never touches the
/// patch, the state blob or the host's undo stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeOp {
    SetTap(ScopeTap),
}

/// Display-window decimation. The ring stores every base-rate frame; the frame
/// takes every 4th, so a window of [`SCOPE_WINDOW`] samples covers 4× that many
/// frames — ~32 ms at 48 kHz, a handful of cycles of anything in the bass
/// register and plenty of them higher up.
pub const SCOPE_DECIMATION: usize = 4;

/// Samples per frame. Wide enough for the page's trigger search (it spends the
/// first quarter finding a rising zero-crossing and draws the rest), narrow
/// enough that 30 frames a second of JSON stays a rounding error on the bridge.
pub const SCOPE_WINDOW: usize = 384;

/// One window of captured audio, oldest → newest.
///
/// Mono: a scope trace of a stereo pair is either two traces (twice the wire,
/// half the vertical resolution each) or one overlaid on the other, and neither
/// tells the player anything the sum doesn't about the shape of the waveform.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScopeFrame {
    pub samples: Vec<f32>,
}

impl ScopeFrame {
    /// Read the latest window out of `bus`, or `None` while the ring holds less
    /// than a full window (freshly cleared, or the tap is off).
    ///
    /// Unlike the meter drain this does **not** clear: the ring is a moving
    /// window, so two reads without an intervening block legitimately return
    /// overlapping data. Nothing downstream depends on frames being disjoint.
    pub fn read(bus: &ScopeBus) -> Option<Self> {
        let mut samples = Vec::new();
        bus.read_window(SCOPE_DECIMATION, SCOPE_WINDOW, &mut samples)
            .then_some(Self { samples })
    }

    /// True when the whole window is silence. The tick skips pushing a silent
    /// frame once one has already been sent — the page needs the first flat
    /// window to settle the trace on the centre line, and nothing after it.
    pub fn is_silent(&self) -> bool {
        self.samples.iter().all(|&s| s == 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_codes_round_trip() {
        for tap in [ScopeTap::Off, ScopeTap::Layer1, ScopeTap::Layer2] {
            assert_eq!(ScopeTap::from_code(tap.code()), tap);
        }
    }

    #[test]
    fn an_unknown_tap_code_reads_as_off() {
        assert_eq!(ScopeTap::from_code(9), ScopeTap::Off);
    }

    #[test]
    fn no_frame_until_the_ring_holds_a_window() {
        let bus = ScopeBus::new();
        bus.set_source(ScopeTap::Layer1.code());
        assert!(ScopeFrame::read(&bus).is_none());
        let block = vec![0.5f32; SCOPE_DECIMATION * SCOPE_WINDOW];
        bus.publish_stride(ScopeTap::Layer1.code(), &block, &block, 1);
        let frame = ScopeFrame::read(&bus).expect("a full window");
        assert_eq!(frame.samples.len(), SCOPE_WINDOW);
        assert!(!frame.is_silent());
    }

    #[test]
    fn a_silent_window_reports_silent() {
        let bus = ScopeBus::new();
        bus.set_source(ScopeTap::Layer1.code());
        let block = vec![0.0f32; SCOPE_DECIMATION * SCOPE_WINDOW];
        bus.publish_stride(ScopeTap::Layer1.code(), &block, &block, 1);
        assert!(ScopeFrame::read(&bus).expect("a full window").is_silent());
    }
}
