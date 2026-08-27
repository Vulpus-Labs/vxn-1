//! `WetFade` — the enable/disable declick idiom, extracted from vxn-2.
//!
//! [ADR 0002](../../../adrs/0002-vxn-core-dsp.md) §5 makes this the canonical
//! per-FX enable mechanism. It is vxn-2's, generalised: `enabled` + a smoothed
//! `mix` + a first-set snap + an inactive→active edge flag, lifted out of
//! `DynamicsBlock` where every vxn-2 effect had already reimplemented it.
//!
//! vxn-1 solved the same problem differently — an outer `BypassXfade` that
//! crossfaded the whole effect from outside. That approach retired with vxn-1
//! (2026-08-27): it could not express the thing that makes this idiom work, a
//! switch-off that keeps processing until the wet has actually reached zero and
//! only then hands back a *bit-exact* passthrough. Whole-span switches —
//! oversample-rate changes, bracketed spans — build their own weighting on
//! `raised_cosine_rise` directly, which is what vxn-1b and vxn-2 already did.
//!
//! # The three properties that matter
//!
//! 1. **Switch-off fades, it does not cut.** `set_enabled(false)` retargets the
//!    mix to 0; the effect keeps running until the fade lands.
//! 2. **Steady-off is bit-exact, not merely quiet.** Once
//!    [`settled_off`](WetFade::settled_off) is true the caller must return its
//!    input unchanged — not `input * 0.0 + wet * 0.0`, which is a different
//!    float. `assert_bit_exact_after_settle` in `crate::test_util` is the check.
//! 3. **Re-engaging clears stale state.** An effect that has been faded out for
//!    a while holds a stale envelope / delay tail. Dumping that on re-engage is
//!    audible, so the inactive→active edge reports
//!    [`EdgeAction::RisingClear`] and the caller clears.
//!
//! The edge is reported from [`tick`](WetFade::tick) — the audio path — not from
//! `set_enabled`. That is deliberate and matches vxn-2: after `set_enabled(true)`
//! the effect may already be active (a switch-off fade that never completed), in
//! which case there is no edge and clearing would be wrong.

use vxn_core_utils::smoothing::Smoothed;

/// What the caller must do about the edge this tick.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EdgeAction {
    /// Nothing. Steady state, either active or settled off.
    None,
    /// The effect just went inactive → active. Clear the audio state that would
    /// otherwise be stale: envelope followers, delay lines, filter memory.
    RisingClear,
}

/// Enable/disable with a smoothed wet mix, a first-set snap, and edge reporting.
///
/// Hold one per bypassable effect and drive it from `set_params`:
///
/// ```
/// use vxn_core_dsp::declick::{EdgeAction, WetFade};
/// let mut fade = WetFade::new(20.0, 48_000.0);
/// fade.set_enabled(true);
/// fade.set_mix(1.0);
/// // First set snaps, so a patch loaded with the effect on does not ride in.
/// assert_eq!(fade.tick(), (1.0, EdgeAction::RisingClear));
/// assert!(!fade.settled_off());
///
/// fade.set_enabled(false);        // retargets to 0, keeps processing
/// assert!(!fade.settled_off());   // still fading — do NOT passthrough yet
/// ```
#[derive(Clone, Debug)]
pub struct WetFade {
    mix: Smoothed,
    /// Wet level the caller asked for while enabled. Held separately so
    /// re-enabling returns to it rather than to 1.0.
    target: f32,
    enabled: bool,
    /// First `set_*` snaps instead of gliding.
    primed: bool,
    /// Was the effect contributing on the previous tick? Drives the edge.
    was_active: bool,
}

impl WetFade {
    /// `fade_ms` is the switch-on/off glide; 10–20 ms is the usual range.
    pub fn new(fade_ms: f32, sample_rate: f32) -> Self {
        Self {
            mix: Smoothed::new(0.0, fade_ms, sample_rate),
            target: 1.0,
            enabled: false,
            primed: false,
            was_active: false,
        }
    }

    /// Enable or bypass. Disabling pulls the wet mix to 0 so it fades rather
    /// than cuts; the caller keeps processing until [`settled_off`](Self::settled_off).
    #[inline]
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        self.retarget();
    }

    /// Set the wet level applied while enabled (`0.0..=1.0`). Ignored while
    /// bypassed, but remembered, so re-enabling returns to it.
    #[inline]
    pub fn set_mix(&mut self, mix: f32) {
        self.target = mix.clamp(0.0, 1.0);
        self.retarget();
    }

    #[inline]
    fn retarget(&mut self) {
        let t = if self.enabled { self.target } else { 0.0 };
        if self.primed {
            self.mix.set_target(t);
        } else {
            self.mix.snap(t);
            self.primed = true;
        }
    }

    /// Advance one sample. Returns the wet weight to apply and whether the
    /// caller must clear stale audio state first.
    ///
    /// Call this **once per sample**, before using the weight — the edge is only
    /// reported on the tick it happens.
    #[inline]
    pub fn tick(&mut self) -> (f32, EdgeAction) {
        let active = self.is_active();
        let action = if active && !self.was_active {
            EdgeAction::RisingClear
        } else {
            EdgeAction::None
        };
        self.was_active = active;
        if !active {
            // Settled off: do not tick the smoother, so `current()` stays
            // exactly 0.0 and the caller's passthrough stays bit-exact.
            return (0.0, action);
        }
        (self.mix.tick(), action)
    }

    /// Is the effect contributing anything — enabled, or mid fade-out?
    ///
    /// Gate the expensive path on this, **not** on `enabled`: a switch-off fade
    /// must keep running after `enabled` goes false.
    #[inline]
    pub fn is_active(&self) -> bool {
        self.enabled || self.mix.current() > 0.0
    }

    /// Bypassed *and* the fade has fully reached zero. Only now may the caller
    /// return its input bit-exactly.
    #[inline]
    pub fn settled_off(&self) -> bool {
        !self.is_active()
    }

    /// Current wet weight without advancing.
    #[inline]
    pub fn current(&self) -> f32 {
        self.mix.current()
    }

    /// Jump to the current target with no glide — a reset, or a state load that
    /// should not be audible as a sweep.
    #[inline]
    pub fn snap(&mut self) {
        let t = if self.enabled { self.target } else { 0.0 };
        self.mix.snap(t);
        self.primed = true;
        self.was_active = self.is_active();
    }

    /// Re-idle for a transport reset or sample-rate change: drop to the settled
    /// state for the current flag and forget the edge, so the next active tick
    /// reports `RisingClear` and the caller clears.
    #[inline]
    pub fn reset(&mut self) {
        self.snap();
        self.was_active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn on_fade() -> WetFade {
        let mut f = WetFade::new(10.0, SR);
        f.set_enabled(true);
        f.set_mix(1.0);
        f
    }

    /// Property 1 of the module doc: the first set snaps, so a patch loaded
    /// with the effect already engaged does not ride in on a fade.
    #[test]
    fn first_set_snaps_rather_than_gliding() {
        let mut f = on_fade();
        let (w, edge) = f.tick();
        assert_eq!(w, 1.0, "first set should have snapped to full wet");
        assert_eq!(edge, EdgeAction::RisingClear, "inactive -> active must clear");
    }

    /// Property 2: switch-off fades, and is NOT bit-exact until it lands.
    #[test]
    fn switch_off_fades_and_only_then_settles() {
        let mut f = on_fade();
        f.tick();
        f.set_enabled(false);

        assert!(!f.settled_off(), "settled the instant it was disabled — that is a cut, not a fade");
        let mut ticks = 0;
        while !f.settled_off() {
            let (w, _) = f.tick();
            assert!((0.0..=1.0).contains(&w));
            ticks += 1;
            assert!(ticks < 100_000, "fade never settled");
        }
        // A 10 ms fade at 48 kHz is ~480 samples; the one-pole settles exactly
        // in bounded time (vxn-core-utils guarantees it). Just assert it took
        // long enough to be a fade rather than a jump.
        assert!(ticks > 100, "settled in {ticks} ticks — too fast to be a fade");
    }

    /// Property 2, the part that matters: once settled, the weight is exactly
    /// 0.0 — not 1e-9 — so the caller's passthrough can be bit-exact.
    #[test]
    fn settled_off_yields_exactly_zero_forever() {
        let mut f = on_fade();
        f.tick();
        f.set_enabled(false);
        while !f.settled_off() {
            f.tick();
        }
        for _ in 0..1_000 {
            let (w, edge) = f.tick();
            assert_eq!(w.to_bits(), 0.0f32.to_bits(), "settled weight must be exactly +0.0");
            assert_eq!(edge, EdgeAction::None);
        }
    }

    /// Property 3: re-engaging after a completed fade-out reports the edge.
    #[test]
    fn re_engaging_after_settling_reports_rising_clear() {
        let mut f = on_fade();
        f.tick();
        f.set_enabled(false);
        while !f.settled_off() {
            f.tick();
        }
        f.tick(); // steady off
        f.set_enabled(true);
        let (_, edge) = f.tick();
        assert_eq!(edge, EdgeAction::RisingClear);
    }

    /// The reason the edge is reported from `tick` and not `set_enabled`:
    /// toggling off and straight back on mid-fade never goes inactive, so there
    /// is no stale state and clearing would wrongly cut the tail.
    #[test]
    fn a_toggle_that_never_completes_its_fade_reports_no_edge() {
        let mut f = on_fade();
        f.tick();
        f.set_enabled(false);
        for _ in 0..5 {
            let (_, edge) = f.tick();
            assert_eq!(edge, EdgeAction::None);
        }
        assert!(!f.settled_off(), "test is meaningless if the fade already landed");
        f.set_enabled(true);
        for _ in 0..10 {
            let (_, edge) = f.tick();
            assert_eq!(edge, EdgeAction::None, "mid-fade re-enable must not clear state");
        }
    }

    /// `set_mix` while bypassed is remembered, not lost.
    #[test]
    fn mix_set_while_bypassed_applies_on_re_enable() {
        let mut f = WetFade::new(10.0, SR);
        f.set_enabled(false);
        f.set_mix(0.25);
        assert!(f.settled_off());
        f.set_enabled(true);
        f.snap();
        assert_eq!(f.current(), 0.25);
    }

    #[test]
    fn mix_is_clamped() {
        let mut f = WetFade::new(10.0, SR);
        f.set_enabled(true);
        f.set_mix(5.0);
        f.snap();
        assert_eq!(f.current(), 1.0);
        f.set_mix(-1.0);
        f.snap();
        assert_eq!(f.current(), 0.0);
    }

    /// `is_active` must gate on the fade, not the flag — the distinction the
    /// whole idiom exists for.
    #[test]
    fn is_active_follows_the_fade_not_the_flag() {
        let mut f = on_fade();
        f.tick();
        f.set_enabled(false);
        assert!(!f.settled_off());
        assert!(f.is_active(), "still fading, so still active despite enabled == false");
    }

    #[test]
    fn reset_idles_and_rearms_the_edge() {
        let mut f = on_fade();
        f.tick();
        f.reset();
        let (_, edge) = f.tick();
        assert_eq!(edge, EdgeAction::RisingClear, "reset must re-arm the clear");
    }
}
