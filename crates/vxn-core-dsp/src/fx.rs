//! `FxKernel` — the shared contract for a bypassable stereo effect.
//!
//! **Monomorphic use only.** The trait exists so the effects agree on a shape,
//! and so one test harness can be pointed at all of them; it is never a
//! `Box<dyn FxKernel>` in an audio path. [ADR 0002](../../../adrs/0002-vxn-core-dsp.md)
//! §4 forbids dyn dispatch in a sample loop, and E041's chains are built from
//! concrete types.
//!
//! The trait is what lets `assert_bit_exact_after_settle` and the
//! `process_block` equivalence harness in [`crate::test_util`] be written once
//! instead of per effect — which is the practical payoff, since those are
//! exactly the checks each of 0228–0232 has to repeat.

use crate::declick::WetFade;

/// A bypassable stereo effect.
///
/// # Contract
///
/// - [`process`](FxKernel::process) is a **bit-exact passthrough** once
///   [`is_active`](FxKernel::is_active) is false. Not "approximately silent" —
///   the same float bits in and out. Callers rely on this to skip whole spans.
/// - [`process_block`](FxKernel::process_block) must be **sample-identical** to
///   looping `process`. The default implementation does exactly that; an
///   override exists only to vectorise, and
///   [`crate::test_util::assert_block_matches_sample`] is the check that it
///   still agrees.
/// - [`is_active`](FxKernel::is_active) gates on the *fade*, not the enable
///   flag — see [`WetFade`]. An effect mid switch-off is still active.
pub trait FxKernel {
    /// This effect's parameter snapshot.
    type Params;

    fn new(sample_rate: f32) -> Self;

    /// Fan a parameter snapshot in. Called at control rate, never per sample.
    fn set_params(&mut self, params: &Self::Params);

    /// One stereo sample in, one out.
    fn process(&mut self, l: f32, r: f32) -> (f32, f32);

    /// Process `l`/`r` in place. **Must** be sample-identical to looping
    /// [`process`](FxKernel::process); the default is that loop.
    #[inline]
    fn process_block(&mut self, l: &mut [f32], r: &mut [f32]) {
        debug_assert_eq!(l.len(), r.len(), "stereo block halves must match");
        for (ls, rs) in l.iter_mut().zip(r.iter_mut()) {
            let (a, b) = self.process(*ls, *rs);
            *ls = a;
            *rs = b;
        }
    }

    /// Re-idle for a transport reset or sample-rate change. Parameter targets
    /// are preserved; only the running audio state is dropped.
    fn reset(&mut self);

    /// Clear the audio state that would be stale after an inactive interval —
    /// envelope followers, delay lines, filter memory. Called by the owner on
    /// [`EdgeAction::RisingClear`](crate::declick::EdgeAction::RisingClear).
    fn clear(&mut self);

    /// Is the effect contributing anything? False only when bypassed **and**
    /// the switch-off fade has fully landed.
    fn is_active(&self) -> bool;

    /// Largest absolute value anywhere in the internal state, for a quiescence
    /// gate: a caller deciding whether an oversampled span can be skipped needs
    /// to know a reverb tail has actually decayed, not merely that its input
    /// went quiet.
    ///
    /// The default is the conservative answer — `0.0` when inactive, otherwise
    /// "assume it is still ringing". Tailed kernels (reverbs, resonant filters,
    /// delays) should override with the real figure; vxn-2's
    /// `OtaLadderKernel::state_abs_max` is the model.
    #[inline]
    fn state_abs_max(&self) -> f32 {
        if self.is_active() { f32::INFINITY } else { 0.0 }
    }
}

/// Wraps a kernel that has no enable of its own, adding [`WetFade`] bypass with
/// the edge-clear glue every consumer would otherwise hand-roll.
///
/// This is the "off→on edge-reset" pattern vxn-1b spelled as
/// `limiter_fade` + `limiter_on` + `limiter_primed` in its engine, and vxn-2 as
/// a bare `limiter_was_on` with no fade at all. Ticket 0232 put `StereoLimiter`
/// behind it, so both synths get the same declick and the same true skip.
///
/// # Three weights, three paths
///
/// - **Settled off** — return the input, untouched. Bit-exact, per the
///   [`FxKernel`] contract.
/// - **Settled full** — return the kernel's own output, with no blend at all.
///   `dry + 1.0 * (wet - dry)` is not bitwise `wet`, and a master-bus effect
///   that is simply *on* should not be paying a ULP for the fade it is not
///   using. [`process_block`](Self::process_block) hands the whole block
///   straight to the kernel in this state.
/// - **Mid-fade** — the linear crossfade, per sample.
pub struct Bypassable<K> {
    inner: K,
    fade: WetFade,
}

impl<K> Bypassable<K> {
    pub fn new(inner: K, fade_ms: f32, sample_rate: f32) -> Self {
        Self { inner, fade: WetFade::new(fade_ms, sample_rate) }
    }

    /// Enable or bypass. Bypassing fades the wet out rather than cutting it.
    #[inline]
    pub fn set_enabled(&mut self, on: bool) {
        self.fade.set_enabled(on);
    }

    /// Wet level applied while enabled.
    #[inline]
    pub fn set_mix(&mut self, mix: f32) {
        self.fade.set_mix(mix);
    }

    #[inline]
    pub fn inner(&self) -> &K {
        &self.inner
    }

    #[inline]
    pub fn inner_mut(&mut self) -> &mut K {
        &mut self.inner
    }

    #[inline]
    pub fn is_active(&self) -> bool {
        self.fade.is_active()
    }

    /// The fade's current wet weight, without advancing it.
    #[inline]
    pub fn wet(&self) -> f32 {
        self.fade.current()
    }
}

impl<K: FxKernel> Bypassable<K> {
    /// Fan a parameter snapshot into the wrapped kernel. The enable and the
    /// fade are this wrapper's, not the kernel's.
    #[inline]
    pub fn set_params(&mut self, params: &K::Params) {
        self.inner.set_params(params);
    }

    /// Re-idle: drop the kernel's audio state and settle the fade to silence,
    /// un-primed, so the next `set_enabled` snaps the way a patch load does.
    pub fn reset(&mut self) {
        self.inner.reset();
        self.fade.reset();
    }

    /// Equal-gain dry/wet by the fade weight, with the stale-state clear applied
    /// on the rising edge. Bit-exact passthrough once the fade has settled, and
    /// the kernel's own output once it is fully wet.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let (w, edge) = self.fade.tick();
        if edge == crate::declick::EdgeAction::RisingClear {
            self.inner.clear();
        }
        if w == 0.0 {
            // Settled off — return the input untouched. Deliberately NOT
            // `l * 1.0 + wet * 0.0`, which is a different float for some inputs.
            return (l, r);
        }
        let (wl, wr) = self.inner.process(l, r);
        if w == 1.0 {
            // Fully wet — the kernel's output *is* the answer. Same reasoning
            // as the line above, at the other end of the fade, and it is what
            // keeps `process_block`'s whole-block shortcut sample-identical to
            // this path.
            return (wl, wr);
        }
        (l + w * (wl - l), r + w * (wr - r))
    }

    /// Process a block in place. Sample-identical to looping
    /// [`process`](Self::process); the two steady states short-circuit.
    ///
    /// The settled-full case is the one worth having: a master-bus kernel with a
    /// block entry point (a limiter's serial gain recurrence, say) keeps its
    /// state in registers across the block instead of being re-entered per
    /// sample through a crossfade that weights it 1.0.
    #[inline]
    pub fn process_block(&mut self, l: &mut [f32], r: &mut [f32]) {
        debug_assert_eq!(l.len(), r.len(), "stereo block halves must match");
        if self.fade.settled_off() {
            // True skip. No tick: the fade's latch already reads inactive, so
            // the next re-engage still reports its edge.
            return;
        }
        if self.fade.settled_full() {
            // One tick for the whole block, not none: the weight is 1.0 either
            // way (a settled smoother's tick is idempotent), but the fade's
            // active latch and its rising edge live in `tick`, and skipping it
            // leaves the latch reading "inactive" — so the first sample that
            // later falls to the per-sample path reports a `RisingClear` and
            // wipes a running kernel's state mid-block.
            let (_w, edge) = self.fade.tick();
            if edge == crate::declick::EdgeAction::RisingClear {
                self.inner.clear();
            }
            self.inner.process_block(l, r);
            return;
        }
        for (ls, rs) in l.iter_mut().zip(r.iter_mut()) {
            let (a, b) = self.process(*ls, *rs);
            *ls = a;
            *rs = b;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trivial kernel: gain, plus a counter so we can see `clear` land.
    struct Doubler {
        gain: f32,
        clears: usize,
        active: bool,
    }

    impl FxKernel for Doubler {
        type Params = f32;

        fn new(_sample_rate: f32) -> Self {
            Self { gain: 2.0, clears: 0, active: true }
        }
        fn set_params(&mut self, p: &f32) {
            self.gain = *p;
        }
        fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
            (l * self.gain, r * self.gain)
        }
        fn reset(&mut self) {
            self.active = true;
        }
        fn clear(&mut self) {
            self.clears += 1;
        }
        fn is_active(&self) -> bool {
            self.active
        }
    }

    /// Overrides `process_block` in a way that AGREES — the shape 0228-0232
    /// will use when they vectorise.
    struct BlockDoubler(Doubler);

    impl FxKernel for BlockDoubler {
        type Params = f32;
        fn new(sr: f32) -> Self {
            Self(Doubler::new(sr))
        }
        fn set_params(&mut self, p: &f32) {
            self.0.set_params(p)
        }
        fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
            self.0.process(l, r)
        }
        fn process_block(&mut self, l: &mut [f32], r: &mut [f32]) {
            let g = self.0.gain;
            for s in l.iter_mut().chain(r.iter_mut()) {
                *s *= g;
            }
        }
        fn reset(&mut self) {
            self.0.reset()
        }
        fn clear(&mut self) {
            self.0.clear()
        }
        fn is_active(&self) -> bool {
            self.0.is_active()
        }
    }

    #[test]
    fn default_process_block_matches_process() {
        let mut k = Doubler::new(48_000.0);
        let mut l = [0.1f32, 0.2, 0.3, -0.4];
        let mut r = [-0.1f32, 0.5, 0.25, 0.0];
        k.process_block(&mut l, &mut r);
        assert_eq!(l, [0.2, 0.4, 0.6, -0.8]);
        assert_eq!(r, [-0.2, 1.0, 0.5, 0.0]);
    }

    #[test]
    fn an_overriding_block_impl_agrees_with_the_sample_path() {
        crate::test_util::assert_block_matches_sample(
            || BlockDoubler::new(48_000.0),
            &2.0,
            64,
        );
    }

    #[test]
    fn state_abs_max_default_is_zero_only_when_inactive() {
        let mut k = Doubler::new(48_000.0);
        assert_eq!(k.state_abs_max(), f32::INFINITY);
        k.active = false;
        assert_eq!(k.state_abs_max(), 0.0);
    }

    #[test]
    fn bypassable_is_bit_exact_once_settled() {
        let mut b = Bypassable::new(Doubler::new(48_000.0), 5.0, 48_000.0);
        b.set_enabled(false);
        b.set_mix(1.0);
        // Settled off from construction: first tick is inactive.
        crate::test_util::assert_bit_exact_passthrough(|l, r| b.process(l, r), 512);
    }

    #[test]
    fn bypassable_clears_the_inner_on_the_rising_edge() {
        let mut b = Bypassable::new(Doubler::new(48_000.0), 5.0, 48_000.0);
        b.set_enabled(true);
        b.set_mix(1.0);
        b.process(0.1, 0.1);
        assert_eq!(b.inner().clears, 1, "rising edge must clear once");
        for _ in 0..100 {
            b.process(0.1, 0.1);
        }
        assert_eq!(b.inner().clears, 1, "steady state must not keep clearing");
    }

    #[test]
    fn bypassable_fades_rather_than_cutting() {
        let mut b = Bypassable::new(Doubler::new(48_000.0), 10.0, 48_000.0);
        b.set_enabled(true);
        b.set_mix(1.0);
        let (full, _) = b.process(0.5, 0.5);
        assert!((full - 1.0).abs() < 1e-6, "enabled doubler should give 1.0, got {full}");
        b.set_enabled(false);
        let (mid, _) = b.process(0.5, 0.5);
        assert!(mid > 0.5 && mid < 1.0, "mid-fade output {mid} should sit between dry and wet");
    }
}
