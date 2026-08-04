//! Meter frames — the audio→view half of the metering spine (ticket 0240).
//!
//! The lock-free bus itself is shared infrastructure
//! ([`vxn_core_utils::MeterBus`]); what lives here is VXN1b's **frame**: one
//! drained snapshot of every tap, and its JSON shape on the wire to the
//! faceplate.
//!
//! ## Path
//!
//! ```text
//! audio thread            main thread (~60 Hz on_timer)        page
//! ────────────            ────────────────────────────        ────
//! publish_block_peak  →   MeterFrame::drain(&bus)         →   ev.kind === 'meters'
//!   (atomic max)            (read-and-clear, one pass)          panels/meter.js
//! ```
//!
//! The frame rides the existing per-tick `ViewEvent` batch as a
//! `ViewEvent::Custom` payload, serialised by VXN1b's `serialise_custom_view`
//! hook — so it costs no extra `evaluate_script` and needs no new bridge
//! channel or `vxn-core-app` enum variant.
//!
//! Values are **linear peak magnitudes** (and dB for the reduction slot), not
//! normalised bar heights: the dB mapping and the ballistics (decay, peak-hold)
//! belong to the view, where they can be tuned without touching DSP.

use vxn_core_utils::{MeterBus, MeterTap};

/// One drained snapshot of every meter tap.
///
/// Built by [`Self::drain`], which *clears* the bus — so each frame reports
/// "the extreme since the previous frame". Producing the whole frame in one
/// pass keeps a tick's values mutually consistent (they all cover the same
/// interval) without a lock.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeterFrame {
    /// Layer 1 post-fader peak, `(left, right)`, linear magnitude.
    pub layer1: (f32, f32),
    /// Layer 2 post-fader peak, `(left, right)`, linear magnitude. Rests at
    /// `(0, 0)` in single mode — synth 2 is never ticked, so nothing publishes.
    pub layer2: (f32, f32),
    /// Dynamics slot input peak, `(left, right)`, linear magnitude.
    pub dynamics_in: (f32, f32),
    /// Dynamics slot output peak, `(left, right)`, linear magnitude — post
    /// comp/sat and post the bypass crossfade. Read against `dynamics_in` it
    /// shows what the block is doing to the level, makeup included.
    pub dynamics_out: (f32, f32),
    /// Dynamics gain reduction in dB — `0.0` means none, negative is reduction.
    /// One value, not a pair: the kernel's detector is stereo-linked (0241).
    pub dynamics_gr: f32,
    /// Master output peak, `(left, right)`, linear magnitude. Post master
    /// volume and post the engine's finite guard.
    pub master: (f32, f32),
}

impl MeterFrame {
    /// Read every tap and reset the bus. The UI thread's once-per-tick drain.
    pub fn drain(bus: &MeterBus) -> Self {
        let mut v = [0.0f32; MeterTap::COUNT];
        bus.drain_into(&mut v);
        Self {
            layer1: (v[MeterTap::Layer1L.idx()], v[MeterTap::Layer1R.idx()]),
            layer2: (v[MeterTap::Layer2L.idx()], v[MeterTap::Layer2R.idx()]),
            dynamics_in: (v[MeterTap::DynamicsInL.idx()], v[MeterTap::DynamicsInR.idx()]),
            dynamics_out: (v[MeterTap::DynamicsOutL.idx()], v[MeterTap::DynamicsOutR.idx()]),
            dynamics_gr: v[MeterTap::DynamicsGr.idx()],
            master: (v[MeterTap::MasterL.idx()], v[MeterTap::MasterR.idx()]),
        }
    }

    /// True when every tap is at rest — the whole plugin is silent and the
    /// compressor is not reducing.
    ///
    /// The tick drain skips pushing a frame in this case *only* after it has
    /// pushed at least one silent frame, so the view's decay always gets the
    /// zero that starts it falling but an idle plugin doesn't stream 60
    /// identical frames a second across the bridge for nothing.
    pub fn is_silent(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_maps_every_tap_to_its_field() {
        let bus = MeterBus::new();
        bus.publish_peak(MeterTap::Layer1L, 0.1);
        bus.publish_peak(MeterTap::Layer1R, 0.2);
        bus.publish_peak(MeterTap::Layer2L, 0.3);
        bus.publish_peak(MeterTap::Layer2R, 0.4);
        bus.publish_peak(MeterTap::DynamicsInL, 0.5);
        bus.publish_peak(MeterTap::DynamicsInR, 0.6);
        bus.publish_peak(MeterTap::DynamicsOutL, 0.55);
        bus.publish_peak(MeterTap::DynamicsOutR, 0.65);
        bus.publish_reduction(MeterTap::DynamicsGr, -7.0);
        bus.publish_peak(MeterTap::MasterL, 0.8);
        bus.publish_peak(MeterTap::MasterR, 0.9);

        let f = MeterFrame::drain(&bus);
        assert_eq!(f.layer1, (0.1, 0.2));
        assert_eq!(f.layer2, (0.3, 0.4));
        assert_eq!(f.dynamics_in, (0.5, 0.6));
        assert_eq!(f.dynamics_out, (0.55, 0.65));
        assert_eq!(f.dynamics_gr, -7.0);
        assert_eq!(f.master, (0.8, 0.9));
    }

    #[test]
    fn drain_clears_so_the_next_frame_is_silent() {
        let bus = MeterBus::new();
        bus.publish_peak(MeterTap::MasterL, 0.5);
        assert!(!MeterFrame::drain(&bus).is_silent());
        assert!(MeterFrame::drain(&bus).is_silent());
    }

    #[test]
    fn default_frame_is_silent() {
        assert!(MeterFrame::default().is_silent());
    }
}
