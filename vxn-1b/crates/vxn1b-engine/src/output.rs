//! Oversampled synthesis path — the decimators between the voice banks and the
//! FX bus (ticket 0251).
//!
//! Ported from VXN1's `OutputStage`
//! ([`vxn-1/crates/vxn-engine/src/lib.rs`]), adapted to VXN1b's two-synth
//! structure. The voices render into an oversampled stereo bus; this stage
//! brings it back to the base rate through a halfband FIR pair before the FX
//! chain and master volume run.
//!
//! Four behaviours ride along, each one a shipped VXN1 bug fix rather than a
//! nicety:
//!
//! * **`spread == 0` skip** (VXN1 0107). Spread distributes voice lanes across
//!   L/R; at 0 every lane sits centre, so L and R are bit-identical and the R
//!   decimator is redundant — skip it and copy. The mono→stereo transition seeds
//!   R from L's *converged* state, or R starts from an empty FIR and clicks.
//! * **Silent-drain skip** (VXN1 0106). Once the banks have been silent long
//!   enough for the FIR state to flush ([`DECIMATOR_DRAIN_BLOCKS`]), decimation
//!   is skipped outright and the output zero-filled — an idle instrument should
//!   not pay for a 33-tap filter per sample.
//! * **OS-change crossfade** (VXN1 0191). The FIR state is rate-specific, so a
//!   factor change must reset it; the rebuilt filter then emits near-zero for
//!   its first samples, which is a hard step down from the pre-switch level. A
//!   fade-in *from zero* cannot hide that — it is the step. Instead the
//!   pre-switch level is frozen and crossfaded into the rebuilt output, so the
//!   join is continuous and the FIR settle lands under low weight.
//! * **Phase alignment.** Both decimators see the same weights and the same
//!   ordering every block, so a spread-0 patch stays exactly mono.
//!
//! The whole stage is fixed-size and allocation-free: decimator state is owned,
//! and the OS buses are the caller's stack scratch.

use vxn_dsp::{Oversampler, ms_to_samples};

/// Consecutive silent blocks before the decimator skip kicks in. `HalfbandFir`
/// is 33 taps; at OS = 8 the worst-case cascaded drain is well under one block
/// of OS samples, so 4 base blocks is heavily conservative — the FIR state has
/// certainly flushed to zero before the output starts being zero-filled.
const DECIMATOR_DRAIN_BLOCKS: u32 = 4;

/// Crossfade window (ms) after an oversampling factor change. Long enough to
/// bury the rebuilt FIR's settle, short enough that the frozen level isn't
/// audible as a hold.
const OS_FADE_MS: f32 = 5.0;

/// Equal-gain raised-cosine rise `0.5 − 0.5·cos(π·t)` for `t ∈ [0, 1]`. Zero
/// slope at *both* endpoints, so neither the start nor the steady handoff leaves
/// a slope corner (a corner reads as a click). Same law as VXN1's fade and the
/// FX bypass crossfade.
#[inline]
fn raised_cosine_rise(t: f32) -> f32 {
    0.5 - 0.5 * (core::f32::consts::PI * t).cos()
}

/// The oversampled synthesis path's L/R anti-aliasing decimators plus the
/// bookkeeping their skip paths need. Owned as one unit so the duplicated
/// reset / decimate branches collapse into its API.
pub struct OutputStage {
    /// L/R decimators. Both stay phase-aligned: at spread = 0 the L and R input
    /// streams are identical, so the filter states evolve in lock-step and `r`
    /// outputs match `l` sample-for-sample.
    oversampler: Oversampler,
    oversampler_r: Oversampler,
    /// Whether the R decimator was skipped last block (drives the mono→stereo
    /// state seed).
    spread_zero_last_block: bool,
    /// Consecutive blocks every bank took the silent fast path.
    silent_blocks: u32,
    /// Oversampling factor in effect last block; a change resets the decimators.
    last_os: usize,
    /// Raised-cosine crossfade after a factor change. `0` ⇒ steady.
    os_fade_remaining: usize,
    /// Crossfade window in samples (`OS_FADE_MS` at the base rate).
    os_fade_len: usize,
    /// Last decimated L/R sample of the previous block. Snapshotted into
    /// `os_hold_*` when a factor change arms the crossfade.
    prev_last_l: f32,
    prev_last_r: f32,
    /// Frozen pre-switch level, held constant across the crossfade window.
    os_hold_l: f32,
    os_hold_r: f32,
    /// Whether the first factor has been adopted. Until then a factor "change"
    /// is just the initial seed (decimators already empty), so it neither resets
    /// nor arms a fade — only a *later* genuine change does.
    os_primed: bool,
}

impl OutputStage {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            oversampler: Oversampler::new(),
            oversampler_r: Oversampler::new(),
            spread_zero_last_block: true,
            silent_blocks: 0,
            last_os: 1,
            os_fade_remaining: 0,
            os_fade_len: ms_to_samples(OS_FADE_MS, sample_rate),
            prev_last_l: 0.0,
            prev_last_r: 0.0,
            os_hold_l: 0.0,
            os_hold_r: 0.0,
            os_primed: false,
        }
    }

    /// Recompute the fade window for a new base rate. The decimator reset itself
    /// is [`Self::reset`], which the engine calls alongside it.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.os_fade_len = ms_to_samples(OS_FADE_MS, sample_rate);
    }

    /// Clear both decimators and the phase-alignment bookkeeping. Leaves
    /// `last_os` alone — a transport reset doesn't change the factor, so no fade
    /// is armed; a genuine change is [`Self::on_os_change`]'s job.
    pub fn reset(&mut self) {
        self.oversampler.reset();
        self.oversampler_r.reset();
        self.spread_zero_last_block = true;
        self.silent_blocks = 0;
        self.os_fade_remaining = 0;
        self.prev_last_l = 0.0;
        self.prev_last_r = 0.0;
        self.os_primed = false;
    }

    /// Reset both decimators when the factor changes between process calls (the
    /// FIR state is rate-specific) and arm the crossfade that buries the
    /// resulting settle.
    pub fn on_os_change(&mut self, os: usize) {
        if !self.os_primed {
            // First factor after construction / reset: adopt silently. The
            // decimators are empty, so there is nothing to settle and no click.
            self.last_os = os;
            self.os_primed = true;
        } else if os != self.last_os {
            self.oversampler.reset();
            self.oversampler_r.reset();
            self.last_os = os;
            self.os_hold_l = self.prev_last_l;
            self.os_hold_r = self.prev_last_r;
            self.os_fade_remaining = self.os_fade_len;
        }
    }

    /// Decimate the oversampled L/R buses into `dst_l`/`dst_r` at the base rate.
    ///
    /// `spread_zero` means every sounding layer is centred (L == R), so the R
    /// decimator is skipped and `dst_r` is copied from `dst_l`. `both_silent`
    /// drives the drain-skip. The mono→stereo transition seeds R from L *before*
    /// L decimates this block, so R starts from L's converged state rather than
    /// one block ahead.
    #[allow(clippy::too_many_arguments)] // one paired decimate step, single caller
    pub fn decimate_block(
        &mut self,
        l_os: &[f32],
        r_os: &[f32],
        dst_l: &mut [f32],
        dst_r: &mut [f32],
        os: usize,
        spread_zero: bool,
        both_silent: bool,
    ) {
        if both_silent {
            self.silent_blocks = self.silent_blocks.saturating_add(1);
        } else {
            self.silent_blocks = 0;
        }
        let skip_decimator = self.silent_blocks > DECIMATOR_DRAIN_BLOCKS;

        if !spread_zero && self.spread_zero_last_block {
            self.oversampler_r.clone_state_from(&self.oversampler);
        }

        if skip_decimator {
            dst_l.fill(0.0);
        } else {
            self.oversampler.decimate(l_os, dst_l, os);
        }
        if spread_zero {
            dst_r.copy_from_slice(dst_l);
        } else if skip_decimator {
            dst_r.fill(0.0);
        } else {
            self.oversampler_r.decimate(r_os, dst_r, os);
        }
        self.spread_zero_last_block = spread_zero;

        // OS-change crossfade, applied *last* — after the mono→stereo R-seed and
        // both decimations — so it doesn't fight the seed and touches L/R with
        // identical weights (phase alignment intact). At the first sample the
        // rebuilt output is near-zero (empty FIR) and the hold weight is 1, so
        // the join is continuous; as the weight ramps, the by-then settled new
        // output takes over. Idle it costs nothing.
        let n = dst_l.len().min(dst_r.len());
        if self.os_fade_remaining > 0 && n > 0 {
            let span = (self.os_fade_len as f32 - 1.0).max(1.0);
            let start = (self.os_fade_len - self.os_fade_remaining) as f32;
            for i in 0..n {
                let t = ((start + i as f32) / span).min(1.0);
                let rise = raised_cosine_rise(t);
                dst_l[i] = (1.0 - rise) * self.os_hold_l + rise * dst_l[i];
                dst_r[i] = (1.0 - rise) * self.os_hold_r + rise * dst_r[i];
            }
            self.os_fade_remaining = self.os_fade_remaining.saturating_sub(n);
        }
        if n > 0 {
            self.prev_last_l = dst_l[n - 1];
            self.prev_last_r = dst_r[n - 1];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;
    const BLOCK: usize = 32;

    /// Fill an OS bus with a constant so the decimator's steady-state output is
    /// predictable (a halfband FIR has unity DC gain).
    fn constant_bus(os: usize, v: f32) -> Vec<f32> {
        vec![v; BLOCK * os]
    }

    fn decimate(stage: &mut OutputStage, bus: &[f32], os: usize, spread_zero: bool) -> Vec<f32> {
        let mut l = vec![0.0; BLOCK];
        let mut r = vec![0.0; BLOCK];
        stage.decimate_block(bus, bus, &mut l, &mut r, os, spread_zero, false);
        l
    }

    #[test]
    fn os_one_is_a_passthrough() {
        let mut s = OutputStage::new(SR);
        s.on_os_change(1);
        let bus: Vec<f32> = (0..BLOCK).map(|i| (i as f32) * 0.01 - 0.15).collect();
        let out = decimate(&mut s, &bus, 1, true);
        assert_eq!(out, bus, "OS = 1 must not touch the samples");
    }

    #[test]
    fn spread_zero_keeps_channels_identical() {
        for os in [1usize, 2, 4, 8] {
            let mut s = OutputStage::new(SR);
            s.on_os_change(os);
            let bus = constant_bus(os, 0.5);
            let mut l = vec![0.0; BLOCK];
            let mut r = vec![0.0; BLOCK];
            s.decimate_block(&bus, &bus, &mut l, &mut r, os, true, false);
            assert_eq!(l, r, "spread 0 must stay bit-mono at OS = {os}");
        }
    }

    #[test]
    fn mono_to_stereo_transition_is_seeded_not_stepped() {
        // Run mono (R skipped) until the L decimator has converged on DC, then
        // flip to stereo. Without the state seed R would restart from an empty
        // FIR and step from ~0 up to the DC level — a click.
        let os = 4;
        let mut s = OutputStage::new(SR);
        s.on_os_change(os);
        let bus = constant_bus(os, 0.5);
        for _ in 0..8 {
            decimate(&mut s, &bus, os, true);
        }
        let mut l = vec![0.0; BLOCK];
        let mut r = vec![0.0; BLOCK];
        s.decimate_block(&bus, &bus, &mut l, &mut r, os, false, false);
        assert!(
            (r[0] - l[0]).abs() < 1e-6,
            "R must resume from L's converged state, got {} vs {}",
            r[0],
            l[0]
        );
    }

    #[test]
    fn silent_blocks_eventually_skip_the_decimator() {
        let os = 2;
        let mut s = OutputStage::new(SR);
        s.on_os_change(os);
        let silent = vec![0.0; BLOCK * os];
        let mut l = vec![0.0; BLOCK];
        let mut r = vec![0.0; BLOCK];
        for _ in 0..=(DECIMATOR_DRAIN_BLOCKS + 1) {
            s.decimate_block(&silent, &silent, &mut l, &mut r, os, true, true);
        }
        assert!(s.silent_blocks > DECIMATOR_DRAIN_BLOCKS, "drain counter should have armed");
        assert!(l.iter().all(|v| *v == 0.0), "skipped output must be silent");
        // One non-silent block re-arms the decimator immediately.
        let loud = constant_bus(os, 0.5);
        s.decimate_block(&loud, &loud, &mut l, &mut r, os, true, false);
        assert_eq!(s.silent_blocks, 0, "a sounding block resets the drain counter");
        assert!(l.iter().any(|v| *v != 0.0), "decimation must resume at once");
    }

    #[test]
    fn factor_change_crossfades_from_the_previous_level() {
        // Settle at DC on 2x, then switch to 8x. The rebuilt FIR starts near
        // zero; the crossfade must keep the first output sample near the
        // pre-switch level instead of stepping down to it.
        let mut s = OutputStage::new(SR);
        s.on_os_change(2);
        let bus2 = constant_bus(2, 0.5);
        let mut settled = 0.0;
        for _ in 0..16 {
            settled = *decimate(&mut s, &bus2, 2, true).last().unwrap();
        }
        assert!((settled - 0.5).abs() < 1e-3, "should settle at DC, got {settled}");

        s.on_os_change(8);
        let bus8 = constant_bus(8, 0.5);
        let out = decimate(&mut s, &bus8, 8, true);
        assert!(
            (out[0] - settled).abs() < 0.05,
            "first post-switch sample {} should join near the held level {settled}",
            out[0]
        );
    }

    #[test]
    fn first_factor_is_adopted_without_arming_a_fade() {
        let mut s = OutputStage::new(SR);
        s.on_os_change(8);
        assert_eq!(s.os_fade_remaining, 0, "the initial seed must not fade");
        assert_eq!(s.last_os, 8);
    }
}
