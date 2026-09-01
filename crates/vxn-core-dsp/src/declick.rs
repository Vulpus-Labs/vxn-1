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

    /// Set the enable flag and the wet level **together**, priming once.
    ///
    /// Prefer this to `set_enabled` + `set_mix` when a params snapshot carries
    /// both — which is the normal case, since an FX kernel's `set_params` fans
    /// an `on` flag and a `mix` in from the same struct.
    ///
    /// The difference is only visible on the very first call, and it matters:
    /// the separate setters prime on whichever lands first, so
    /// `set_enabled(true)` snaps to the *default* target and the following
    /// `set_mix(m)` then glides down to `m` — an audible ride-in on a patch
    /// that loads with the effect already engaged. Setting both before priming
    /// snaps straight to `m`, which is what vxn-2's hand-rolled
    /// `enabled` + `mix_primed` pair did and what the moved kernels must keep
    /// doing to stay bit-exact.
    #[inline]
    pub fn set(&mut self, on: bool, mix: f32) {
        self.enabled = on;
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
    ///
    /// The active/inactive latch behind the edge is updated from the state
    /// **after** the tick, not before it. That is what makes the edge survive
    /// the normal ownership pattern: on the sample a switch-off fade finally
    /// lands, the owner's `is_active` gate goes false and it stops calling
    /// `tick` at all. Latching the pre-tick state would leave "active" stuck on
    /// through the whole idle stretch and swallow the next
    /// [`EdgeAction::RisingClear`] — the re-enable the edge exists for.
    #[inline]
    pub fn tick(&mut self) -> (f32, EdgeAction) {
        let active = self.is_active();
        let action = if active && !self.was_active {
            EdgeAction::RisingClear
        } else {
            EdgeAction::None
        };
        if !active {
            // Settled off: do not tick the smoother, so `current()` stays
            // exactly 0.0 and the caller's passthrough stays bit-exact.
            self.was_active = false;
            return (0.0, action);
        }
        let w = self.mix.tick();
        // Latch the post-tick state — see the doc comment. A fade that reaches
        // zero on this very sample is already inactive, and this is the last
        // tick the owner will make before its gate closes. `w` *is* the
        // post-tick `mix.current()`, so this costs no extra load.
        self.was_active = self.enabled || w > 0.0;
        (w, action)
    }

    /// Is the effect contributing anything — enabled, or mid fade-out?
    ///
    /// Gate the expensive path on this, **not** on `enabled`: a switch-off fade
    /// must keep running after `enabled` goes false.
    #[inline]
    pub fn is_active(&self) -> bool {
        self.enabled || self.mix.current() > 0.0
    }

    /// Enabled, at full wet, and not mid-glide — so every sample of the coming
    /// block would be weighted exactly 1.0.
    ///
    /// **Reached by a snap, not by a glide.** A fade that rides *up* to 1.0
    /// stalls a ULP-scale distance short and stays there: `Smoothed`'s snap
    /// threshold is an absolute 1e-6, and the one-pole's increment falls below
    /// half a ULP near 1.0 while the remaining distance is still ~1.4e-5. So
    /// this reads true for an effect that loaded already engaged (the first
    /// `set` snaps) and false for one toggled on mid-render, which then keeps
    /// blending at ~0.999986 — inaudible, and *not* something to "fix" here:
    /// making the smoother land changes every render in the repo. See
    /// `tests::the_bare_smoother_stalls_short_of_a_full_wet_target`, which
    /// pins the behaviour so the next person meets it as a documented fact.
    ///
    /// The licence a block-processing owner needs to hand the whole block to its
    /// kernel and skip the per-sample crossfade entirely: at weight 1.0 the
    /// blend is the kernel's own output, so running it is arithmetic that
    /// changes nothing (and, at `dry + 1.0 * (wet - dry)`, changes it by a ULP).
    #[inline]
    pub fn settled_full(&self) -> bool {
        self.enabled && self.target == 1.0 && self.mix.current() == 1.0
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

    /// Re-idle for a transport reset or sample-rate change: drop to silence,
    /// forget the edge so the next active tick reports `RisingClear`, and
    /// **un-prime**, so the next `set`/`set_mix` snaps to the patch value
    /// instead of gliding to it.
    ///
    /// Un-priming is the part that matters, and it is what a re-idle means:
    /// nothing is playing, so the following parameter fan-in is a fresh load
    /// and should land at its value the way a patch load does. Snapping to the
    /// *current* enable flag and staying primed — the obvious reading — leaves
    /// a chain whose `reset` promises "every slot fully bypassed" gliding down
    /// from the old mix instead, which is audible on the first block after a
    /// transport reset and is what
    /// `vxn1b-engine::fx::tests::reset_snaps_to_bypass` caught.
    #[inline]
    pub fn reset(&mut self) {
        self.mix.snap(0.0);
        self.enabled = false;
        self.primed = false;
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

    /// The ownership pattern the post-tick latch exists for: the owner gates on
    /// `is_active`, so the moment the fade lands it stops calling `tick`
    /// entirely. The next re-enable must still report `RisingClear` — with a
    /// pre-tick latch it would not, because the last tick taken would have
    /// recorded "active".
    #[test]
    fn an_owner_that_stops_ticking_when_the_fade_lands_still_gets_the_edge() {
        let mut f = on_fade();
        f.tick();
        f.set_enabled(false);
        // Exactly what a gated chain does: tick only while active.
        while f.is_active() {
            f.tick();
        }
        f.set_enabled(true);
        assert_eq!(f.tick().1, EdgeAction::RisingClear, "re-enable must re-arm the clear");
    }

    /// The same, for an owner that keeps ticking through the idle stretch.
    #[test]
    fn ticking_through_the_settled_off_stretch_keeps_the_edge_armed() {
        let mut f = on_fade();
        f.tick();
        f.set_enabled(false);
        while !f.settled_off() {
            f.tick();
        }
        for _ in 0..256 {
            assert_eq!(f.tick(), (0.0, EdgeAction::None));
        }
        f.set_enabled(true);
        assert_eq!(f.tick().1, EdgeAction::RisingClear, "re-enable must re-arm the clear");
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

    /// The stall this fade guards against, demonstrated on the bare smoother:
    /// gliding up to 1.0, the one-pole's increment drops below half a ULP while
    /// the remaining distance is still far above `Smoothed`'s absolute snap
    /// threshold, so it stops moving and never arrives.
    #[test]
    fn the_bare_smoother_stalls_short_of_a_full_wet_target() {
        let mut s = Smoothed::new(0.0, 10.0, SR);
        s.set_target(1.0);
        let mut last = -1.0;
        let mut ticks = 0;
        while s.current() != last {
            last = s.current();
            s.tick();
            ticks += 1;
            assert!(ticks < 1_000_000, "smoother is still moving — re-check this");
        }
        assert!(s.current() < 1.0, "smoother reached 1.0 — the stall is gone");
        assert!(
            1.0 - s.current() < 1.0e-4,
            "stall distance should be ULP-scale, got {}",
            1.0 - s.current()
        );
    }

    #[test]
    fn settled_full_is_only_true_at_a_steady_full_wet() {
        let mut f = WetFade::new(10.0, SR);
        assert!(!f.settled_full(), "fresh fade is bypassed");
        f.set(true, 1.0);
        assert!(f.settled_full(), "first set snaps — already fully wet");
        f.set_mix(0.5);
        assert!(!f.settled_full(), "a partial mix is not full wet");
        for _ in 0..64 {
            f.tick();
        }
        f.set_mix(1.0);
        assert!(!f.settled_full(), "gliding back up is not settled yet");
        // A glide up to 1.0 never lands (see the doc comment); a snap does.
        f.snap();
        assert!(f.settled_full());
        f.set_enabled(false);
        assert!(!f.settled_full(), "retargeted to 0 — the block is not steady");
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
    fn reset_unprimes_so_the_next_load_snaps() {
        // A reset mid-effect must not leave the fade gliding down from the old
        // mix when the patch comes back on: the next `set` is a fresh load.
        let mut f = on_fade();
        f.tick();
        f.reset();
        assert!(f.settled_off(), "reset should drop to silence");
        f.set(true, 0.4);
        let (w, edge) = f.tick();
        assert_eq!(w, 0.4, "post-reset load should snap, not glide");
        assert_eq!(edge, EdgeAction::RisingClear, "and re-arm the clear");
    }

    #[test]
    fn reset_then_bypass_is_immediately_settled() {
        // What `FxChain::reset` relies on: reset, then params that say "off",
        // and the very next tick is already a bit-exact passthrough.
        let mut f = on_fade();
        f.tick();
        f.reset();
        f.set(false, 0.4);
        assert!(f.settled_off(), "off-after-reset must not ride down");
        assert_eq!(f.tick().0, 0.0);
    }

    /// The complement of `reset_unprimes_so_the_next_load_snaps`: a reset with
    /// no params behind it must sit silent rather than report an edge. The
    /// clear re-arms for whenever the effect next becomes active, which is not
    /// necessarily the next tick.
    #[test]
    fn reset_idles_until_params_arrive() {
        let mut f = on_fade();
        f.tick();
        f.reset();
        for _ in 0..64 {
            assert_eq!(f.tick(), (0.0, EdgeAction::None), "idle after reset");
        }
        assert!(f.settled_off());
    }
}
