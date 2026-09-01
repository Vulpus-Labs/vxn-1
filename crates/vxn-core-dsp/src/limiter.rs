//! `StereoLimiter` as an [`FxKernel`], so it can go behind
//! [`Bypassable`](crate::fx::Bypassable).
//!
//! The limiter itself stays in `vxn-core-utils` — it is a plain kernel with no
//! enable, no params snapshot and no lifecycle beyond `reset`, which is the
//! leaf-utils shape (ADR 0002 §1). What lives here is only the trait impl that
//! lets the shared bypass wrapper hold one, and the reason it is *here* rather
//! than there is the dependency direction: `FxKernel` is this crate's.
//!
//! Both engines held the same hand-rolled glue around it before ticket 0232 —
//! vxn-1b a `Smoothed` fade plus `limiter_on` plus `limiter_primed`, vxn-2 a
//! bare `limiter_was_on` edge with no fade — and `Bypassable<StereoLimiter>` is
//! that glue, once.

use vxn_core_utils::limiter::StereoLimiter;

use crate::fx::FxKernel;

/// The limiter's only control: the linear threshold it starts pulling gain at.
///
/// A struct rather than a bare `f32` because `FxKernel::Params` is the shape a
/// block-rate fan-in takes, and a named field survives a second control being
/// added better than a positional float does. Neither synth sets it today —
/// both run the fixed master ceiling — so [`Default`] is the ceiling
/// `StereoLimiter::new` already installs, and `set_params` is a no-op in
/// practice rather than a thing call sites must remember.
#[derive(Clone, Copy, Debug)]
pub struct LimiterParams {
    /// Linear threshold, before the limiter's internal trim. `1.0` is the
    /// constructor default.
    pub threshold: f32,
}

impl Default for LimiterParams {
    fn default() -> Self {
        Self { threshold: 1.0 }
    }
}

impl FxKernel for StereoLimiter {
    type Params = LimiterParams;

    fn new(sample_rate: f32) -> Self {
        StereoLimiter::new(sample_rate)
    }

    fn set_params(&mut self, p: &LimiterParams) {
        self.set_threshold(p.threshold);
    }

    #[inline]
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        StereoLimiter::process(self, l, r)
    }

    #[inline]
    fn process_block(&mut self, l: &mut [f32], r: &mut [f32]) {
        StereoLimiter::process_block(self, l, r);
    }

    /// Both re-idle and stale-state drop are the same operation here: the
    /// limiter's whole state is its lookahead lines and gain envelope, and it
    /// holds no fade of its own to settle — that is the wrapper's.
    fn reset(&mut self) {
        StereoLimiter::reset(self);
    }

    fn clear(&mut self) {
        StereoLimiter::reset(self);
    }

    /// Always. The limiter has no enable to be off — bypassing it is the
    /// wrapper's job, and `Bypassable` gates on its own fade.
    #[inline]
    fn is_active(&self) -> bool {
        true
    }

    /// Conservative: the lookahead lines hold up to `MAX_ATTACK_MS` of audio,
    /// and reporting anything smaller would license a caller to skip a span
    /// while a delayed transient is still in flight.
    #[inline]
    fn state_abs_max(&self) -> f32 {
        f32::INFINITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::Bypassable;

    const SR: f32 = 48_000.0;

    fn hot(n: usize) -> (f32, f32) {
        let x = 1.8 * (std::f32::consts::TAU * 220.0 * n as f32 / SR).sin();
        (x, -x)
    }

    /// Bypassed and settled, the wrapper hands the input back untouched — the
    /// property vxn-2's `if limiter_on` gate had and must keep.
    #[test]
    fn bypassed_is_a_bit_exact_skip() {
        let mut b = Bypassable::new(StereoLimiter::new(SR), 10.0, SR);
        b.set_enabled(false);
        for n in 0..1_024 {
            let (x, y) = hot(n);
            let (mut l, mut r) = ([x], [y]);
            b.process_block(&mut l, &mut r);
            assert_eq!(l[0].to_bits(), x.to_bits(), "L touched while bypassed at n={n}");
            assert_eq!(r[0].to_bits(), y.to_bits(), "R touched while bypassed at n={n}");
        }
    }

    /// Fully engaged, the wrapper is the bare limiter — no fade arithmetic in
    /// the path. This is what keeps vxn-2's limiter-on render bit-identical
    /// across 0232.
    #[test]
    fn fully_engaged_is_the_bare_limiter() {
        let mut wrapped = Bypassable::new(StereoLimiter::new(SR), 10.0, SR);
        wrapped.set_enabled(true);
        let mut bare = StereoLimiter::new(SR);

        for n in 0..4_096 {
            let (x, y) = hot(n);
            let (mut wl, mut wr) = ([x], [y]);
            wrapped.process_block(&mut wl, &mut wr);
            let (mut bl, mut br) = ([x], [y]);
            FxKernel::process_block(&mut bare, &mut bl, &mut br);
            assert_eq!(wl[0].to_bits(), bl[0].to_bits(), "L diverged at n={n}");
            assert_eq!(wr[0].to_bits(), br[0].to_bits(), "R diverged at n={n}");
        }
    }

    /// The block path must agree with the sample path at every weight, fade
    /// included — the case the `w == 1.0` shortcut in `process` exists for.
    #[test]
    fn block_and_sample_paths_agree_across_a_fade() {
        let mut by_block = Bypassable::new(StereoLimiter::new(SR), 10.0, SR);
        let mut by_sample = Bypassable::new(StereoLimiter::new(SR), 10.0, SR);
        for b in [&mut by_block, &mut by_sample] {
            b.set_enabled(true);
        }
        for n in 0..8_192 {
            // Toggle mid-run so both paths spend time at 1.0, mid-fade and 0.0.
            if n == 1_024 {
                by_block.set_enabled(false);
                by_sample.set_enabled(false);
            }
            if n == 6_000 {
                by_block.set_enabled(true);
                by_sample.set_enabled(true);
            }
            let (x, y) = hot(n);
            let (mut l, mut r) = ([x], [y]);
            by_block.process_block(&mut l, &mut r);
            let (sl, sr_) = by_sample.process(x, y);
            assert_eq!(l[0].to_bits(), sl.to_bits(), "L diverged at n={n}");
            assert_eq!(r[0].to_bits(), sr_.to_bits(), "R diverged at n={n}");
        }
    }

    /// Re-engaging after a settled bypass must clear the lookahead, or the
    /// transient that was in flight when it switched off leaks out later. This
    /// is `limiter.reset()` on the off→on edge, now inside the wrapper.
    #[test]
    fn re_engaging_clears_the_lookahead() {
        let mut b = Bypassable::new(StereoLimiter::new(SR), 10.0, SR);
        b.set_enabled(true);
        // Load the lookahead with a loud transient, then bypass before it can
        // come out the other end.
        let (mut l, mut r) = ([2.0_f32; 4], [2.0_f32; 4]);
        b.process_block(&mut l, &mut r);
        b.set_enabled(false);
        while b.is_active() {
            let (mut l, mut r) = ([0.0_f32; 32], [0.0_f32; 32]);
            b.process_block(&mut l, &mut r);
        }
        // Back on, into silence: nothing may emerge.
        b.set_enabled(true);
        let mut peak = 0.0_f32;
        for _ in 0..64 {
            let (mut l, mut r) = ([0.0_f32; 32], [0.0_f32; 32]);
            b.process_block(&mut l, &mut r);
            for (a, c) in l.iter().zip(r.iter()) {
                peak = peak.max(a.abs()).max(c.abs());
            }
        }
        assert_eq!(peak, 0.0, "stale lookahead leaked on re-engage: {peak}");
    }

    /// And the switch-off is a glide, not a step — vxn-1b's `limiter_fade`,
    /// which vxn-2 did not have before 0232.
    #[test]
    fn switching_off_glides_rather_than_stepping() {
        let mut b = Bypassable::new(StereoLimiter::new(SR), 10.0, SR);
        b.set_enabled(true);
        let mut last = 0.0;
        for n in 0..2_048 {
            let (x, y) = hot(n);
            last = b.process(x, y).0;
        }
        b.set_enabled(false);
        let (x, y) = hot(2_048);
        let first_off = b.process(x, y).0;
        assert!(
            (first_off - last).abs() < 0.2,
            "switch-off stepped the output: {last} -> {first_off}"
        );
        assert!(
            (first_off - x).abs() > 1.0e-6,
            "switch-off was instant (already dry at {first_off})"
        );
    }
}
