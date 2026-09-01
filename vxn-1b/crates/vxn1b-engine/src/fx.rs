//! Serial post-voice FX chain (ticket 0207, epic E037).
//!
//! Order (ADR 0001 §8): **dynamics → chorus → phaser → delay → reverb** —
//! dynamics first (input compression / drive ahead of the modulation + time
//! effects), matching the faceplate (Dynamics left of FX) and VXN2's FX bus.
//! Runs
//! between the summed bank output and master volume, at the engine's global
//! oversample rate (today 1×; the chain keys off the rate it's constructed at,
//! so it follows the OS factor once that lands).
//!
//! ## On/off discipline
//!
//! Every slot is a **true skip** when it is bypassed and its bypass has fully
//! settled: the kernel's `process` is never called, so five idle effects cost
//! five gate checks rather than five wet=0 multiplies through the DSP (the E037
//! CPU risk).
//!
//! Since ticket 0232 closed E041 there is **one** answer to where the bypass
//! lives: inside the kernel, as a `WetFade`. Every slot is the same three lines
//! — gate on `is_active()`, otherwise call `process` — and the dry/wet crossfade
//! happens once, in the kernel, against the mix the patch asked for. The
//! stale-state clear on the off→on edge is the kernel's too, driven by
//! `EdgeAction::RisingClear`.
//!
//! What this chain used to hold, and no longer does: a `Smoothed` fade and a
//! latched on-state per slot, an outer crossfade against the dry input, and a
//! `clear_slot` dispatch. Nothing here is allowed to wrap a kernel that already
//! fades internally (E041's double-fade ban), and now nothing can — there is no
//! outer fade left to wrap it with.

use std::sync::Arc;

use vxn_core_utils::{MeterBus, MeterTap};
use vxn_dsp::phaser::FxKernel as _;
use vxn_dsp::{
    ChorusParams, DynamicsBlock, DynamicsParams, FdnReverb, FdnReverbParams, PhaserParams,
    StereoChorus, StereoDelay, StereoDelayParams, StereoPhaser,
};

use crate::params::{ParamId, Params};

/// Delay high-frequency damping in the feedback path. Fixed internal default
/// (not a param) — mirrors VXN1's hardcoded `0.3`.
const DELAY_DAMPING: f32 = 0.3;

/// Longest delay time the ring buffer must hold. Deliberately **twice** the
/// `delay_time` param ceiling (2 s): free-run mode can only ask for 2 s, but
/// tempo sync resolves a subdivision *period*, and the slow end of the table
/// runs well past the knob's range — `1/1` is 4 s at 60 BPM. Sizing the line to
/// the knob would have silently clamped those, so the label would read `1/1`
/// while the ear heard 2 s (0267).
///
/// Since 0231 this is the shared kernel's own capacity, re-exported rather than
/// passed: `vxn-core-dsp` allocates for `MAX_DELAY_S` at construction and both
/// synths get the same 4 s line. `crate::sync` clamps synced times against it.
pub(crate) const DELAY_MAX_SECONDS: f32 = vxn_dsp::delay::MAX_DELAY_S;

/// Block-rate snapshot of the FX params, fanned into the chain each control
/// block. Character values map straight to each kernel's setter; the `*_on`
/// bools travel with their slot's mix into the kernel that owns the fade.
#[derive(Clone, Copy, Debug)]
pub struct FxParams {
    pub chorus_on: bool,
    pub chorus_rate: f32,
    pub chorus_depth: f32,
    pub chorus_mix: f32,

    pub phaser_on: bool,
    pub phaser_rate: f32,
    pub phaser_depth: f32,
    pub phaser_feedback: f32,
    pub phaser_mix: f32,
    /// L/R LFO sweep offset, already normalised to the kernel's `spread`
    /// (`[0, 1]`) — the param itself is in degrees.
    pub phaser_stereo: f32,

    pub delay_on: bool,
    pub delay_time: f32,
    pub delay_feedback: f32,
    pub delay_mix: f32,
    /// Feedback crossfeed (ping-pong) on/off.
    pub delay_pingpong: bool,

    pub reverb_on: bool,
    pub reverb_size: f32,
    pub reverb_decay: f32,
    pub reverb_damp: f32,
    pub reverb_mix: f32,

    pub dynamics_on: bool,
    pub dynamics_threshold: f32,
    pub dynamics_ratio: f32,
    pub dynamics_attack: f32,
    pub dynamics_release: f32,
    pub dynamics_makeup: f32,
    pub dynamics_drive: f32,
    pub dynamics_mix: f32,
}

impl FxParams {
    /// Read the FX params out of the flat param table. `tempo_bpm` resolves the
    /// delay's time when Delay Sync is on (0267); it is ignored otherwise.
    pub fn from_params(p: &Params, tempo_bpm: f32) -> Self {
        Self {
            chorus_on: p.bool(ParamId::ChorusOn),
            chorus_rate: p.get(ParamId::ChorusRate),
            chorus_depth: p.get(ParamId::ChorusDepth),
            chorus_mix: p.get(ParamId::ChorusMix),

            phaser_on: p.bool(ParamId::PhaserOn),
            phaser_rate: p.get(ParamId::PhaserRate),
            phaser_depth: p.get(ParamId::PhaserDepth),
            phaser_feedback: p.get(ParamId::PhaserFeedback),
            phaser_mix: p.get(ParamId::PhaserMix),
            phaser_stereo: p.get(ParamId::PhaserStereo) / 180.0,

            delay_on: p.bool(ParamId::DelayOn),
            delay_time: crate::sync::delay_time_seconds(p, tempo_bpm),
            delay_feedback: p.get(ParamId::DelayFeedback),
            delay_mix: p.get(ParamId::DelayMix),
            delay_pingpong: p.bool(ParamId::DelayPingPong),

            reverb_on: p.bool(ParamId::ReverbOn),
            reverb_size: p.get(ParamId::ReverbSize),
            reverb_decay: p.get(ParamId::ReverbDecay),
            reverb_damp: p.get(ParamId::ReverbDamp),
            reverb_mix: p.get(ParamId::ReverbMix),

            dynamics_on: p.bool(ParamId::DynamicsOn),
            dynamics_threshold: p.get(ParamId::DynamicsThreshold),
            dynamics_ratio: p.get(ParamId::DynamicsRatio),
            dynamics_attack: p.get(ParamId::DynamicsAttack),
            dynamics_release: p.get(ParamId::DynamicsRelease),
            dynamics_makeup: p.get(ParamId::DynamicsMakeup),
            dynamics_drive: p.get(ParamId::DynamicsDrive),
            dynamics_mix: p.get(ParamId::DynamicsMix),
        }
    }
}

/// The five-slot serial FX chain. Constructed once (all buffers allocated at
/// `new`); `process_block` is allocation-free.
pub struct FxChain {
    chorus: StereoChorus,
    phaser: StereoPhaser,
    delay: StereoDelay,
    reverb: FdnReverb,
    dynamics: DynamicsBlock,
    /// Meter publish target (0240/0241). `None` until the engine attaches one,
    /// so a bare `FxChain` (unit tests) runs with no metering and no branch cost
    /// beyond this check once per block.
    meters: Option<Arc<MeterBus>>,
}

impl FxChain {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            chorus: StereoChorus::new(sample_rate),
            phaser: StereoPhaser::new(sample_rate),
            delay: StereoDelay::new(sample_rate),
            reverb: FdnReverb::new(sample_rate),
            dynamics: DynamicsBlock::new(sample_rate),
            meters: None,
        }
    }

    /// Attach the meter bus the dynamics taps publish into (0240/0241). The
    /// engine calls this when it adopts a bus, so the chain and the engine
    /// always publish into the same slots.
    pub fn set_meters(&mut self, meters: Arc<MeterBus>) {
        self.meters = Some(meters);
    }

    /// Silence all tails and snap every slot to fully bypassed. Called on engine
    /// reset alongside the voice/bank reset.
    pub fn reset(&mut self) {
        // `reset`, not `clear`, for the kernels whose bypass lives inside them:
        // re-idling has to settle that fade too, not just empty the state.
        self.chorus.reset();
        self.phaser.reset();
        self.reverb.reset();
        self.delay.reset();
        self.dynamics.reset();
    }

    /// Fan a block-rate param snapshot into the kernels. Every enable travels
    /// with its own mix into the kernel that owns it; character params always
    /// update, which is cheap and leaves each slot ready for the moment it
    /// re-activates.
    pub fn set_params(&mut self, p: &FxParams) {
        self.chorus.set_params(&ChorusParams {
            on: p.chorus_on,
            rate_hz: p.chorus_rate,
            depth: p.chorus_depth,
            mix: p.chorus_mix,
            ..ChorusParams::default()
        });
        self.phaser.set_params(&PhaserParams {
            on: p.phaser_on,
            rate_hz: p.phaser_rate,
            depth: p.phaser_depth,
            feedback: p.phaser_feedback,
            mix: p.phaser_mix,
            spread: p.phaser_stereo,
        });
        // Sync is resolved upstream in `FxParams::from_params` (0267), so the
        // kernel is handed a free time in ms and its own sync stays off; the
        // 100 ms glide to that target still happens inside the kernel.
        self.delay.set_params(&StereoDelayParams {
            on: p.delay_on,
            time_ms: p.delay_time * 1_000.0,
            sync: false,
            sync_index: 0,
            feedback: p.delay_feedback,
            damping: DELAY_DAMPING,
            mix: p.delay_mix,
            pingpong: p.delay_pingpong,
        });
        self.reverb.set_params(&FdnReverbParams {
            on: p.reverb_on,
            size: p.reverb_size,
            decay_secs: p.reverb_decay,
            damp: p.reverb_damp,
            mix: p.reverb_mix,
        });
        self.dynamics.set_from(&DynamicsParams {
            on: p.dynamics_on,
            threshold_db: p.dynamics_threshold,
            ratio: p.dynamics_ratio,
            attack_ms: p.dynamics_attack,
            release_ms: p.dynamics_release,
            makeup_db: p.dynamics_makeup,
            drive_db: p.dynamics_drive,
            mix: p.dynamics_mix,
        });
    }

    /// Run the serial chain over a control block, in place. Each stage is a true
    /// skip while it's bypassed and settled.
    pub fn process_block(&mut self, l: &mut [f32], r: &mut [f32]) {
        // Dynamics in/out peaks for the metering spine (0240/0241). Accumulated
        // in locals across the block and published once at the end, so the tap
        // costs two compares per sample rather than an atomic per sample.
        let (mut in_pk_l, mut in_pk_r) = (0.0f32, 0.0f32);
        let (mut out_pk_l, mut out_pk_r) = (0.0f32, 0.0f32);
        for (ls, rs) in l.iter_mut().zip(r.iter_mut()) {
            let (mut xl, mut xr) = (*ls, *rs);
            // Input to the dynamics slot = the summed layers, since dynamics
            // runs first in the chain (ADR 0001 §8).
            in_pk_l = in_pk_l.max(xl.abs());
            in_pk_r = in_pk_r.max(xr.abs());
            (xl, xr) = self.run_dynamics(xl, xr);
            // Output is post comp/sat AND post the bypass crossfade, so it
            // reads what the slot actually hands to the chorus.
            out_pk_l = out_pk_l.max(xl.abs());
            out_pk_r = out_pk_r.max(xr.abs());
            (xl, xr) = self.run_chorus(xl, xr);
            (xl, xr) = self.run_phaser(xl, xr);
            (xl, xr) = self.run_delay(xl, xr);
            (xl, xr) = self.run_reverb(xl, xr);
            *ls = xl;
            *rs = xr;
        }
        if let Some(meters) = &self.meters {
            meters.publish_peak(MeterTap::DynamicsInL, in_pk_l);
            meters.publish_peak(MeterTap::DynamicsInR, in_pk_r);
            meters.publish_peak(MeterTap::DynamicsOutL, out_pk_l);
            meters.publish_peak(MeterTap::DynamicsOutR, out_pk_r);
            // Read-and-clear from the kernel, then fold into the bus's own
            // read-and-clear slot — the deepest reduction survives both hops
            // however the block and frame rates line up.
            meters.publish_reduction(MeterTap::DynamicsGr, self.dynamics.take_gain_reduction_db());
        }
    }

    /// The chorus slot. Not an [`fx_slot!`]: since 0229 the kernel carries its
    /// own `WetFade`, so bypass is `is_active()` and the equal-power dry/wet
    /// blend is the kernel's own — an outer crossfade would have re-scaled it.
    #[inline]
    fn run_chorus(&mut self, xl: f32, xr: f32) -> (f32, f32) {
        if !self.chorus.is_active() {
            return (xl, xr);
        }
        self.chorus.process(xl, xr)
    }

    /// The phaser slot. Same shape as [`Self::run_chorus`], since 0228 — the
    /// kernel owns its fade, including the wet-makeup curve.
    #[inline]
    fn run_phaser(&mut self, xl: f32, xr: f32) -> (f32, f32) {
        if !self.phaser.is_active() {
            return (xl, xr);
        }
        self.phaser.process(xl, xr)
    }

    /// The reverb slot, internal fade since 0230. `is_active()` stays true
    /// through the whole switch-off glide, so the tail rings out through the
    /// fade rather than being cut at the slot boundary.
    #[inline]
    fn run_reverb(&mut self, xl: f32, xr: f32) -> (f32, f32) {
        if !self.reverb.is_active() {
            return (xl, xr);
        }
        self.reverb.process(xl, xr)
    }

    /// The delay slot, internal fade since 0231. `is_active()` covers the whole
    /// switch-off glide, so the repeats ring out through the fade instead of
    /// being cut at the slot boundary, and the kernel clears its own lines on
    /// the off→on edge — what `clear_slot(DELAY)` used to do from here.
    #[inline]
    fn run_delay(&mut self, xl: f32, xr: f32) -> (f32, f32) {
        if !self.delay.is_active() {
            return (xl, xr);
        }
        self.delay.process(xl, xr)
    }

    /// The dynamics slot, internal fade since 0232 — the last slot to give up
    /// its outer one. `DynamicsBlock` has carried a `WetFade` since it moved to
    /// `vxn-core-dsp` in 0227; this chain was holding it permanently on and
    /// fading it from outside, which is the double fade E041 bans.
    #[inline]
    fn run_dynamics(&mut self, xl: f32, xr: f32) -> (f32, f32) {
        if !self.dynamics.is_active() {
            return (xl, xr);
        }
        self.dynamics.process(xl, xr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const SR: f32 = 48_000.0;

    fn all_off() -> FxParams {
        FxParams {
            chorus_on: false,
            chorus_rate: 0.6,
            chorus_depth: 0.5,
            chorus_mix: 0.4,
            phaser_on: false,
            phaser_rate: 0.5,
            phaser_depth: 0.7,
            phaser_feedback: 0.0,
            phaser_mix: 0.5,
            phaser_stereo: 1.0,
            delay_on: false,
            delay_time: 0.35,
            delay_feedback: 0.4,
            delay_mix: 0.25,
            delay_pingpong: true,
            reverb_on: false,
            reverb_size: 0.5,
            reverb_decay: 2.5,
            reverb_damp: 0.4,
            reverb_mix: 0.3,
            dynamics_on: false,
            dynamics_threshold: -12.0,
            dynamics_ratio: 4.0,
            dynamics_attack: 10.0,
            dynamics_release: 100.0,
            dynamics_makeup: 0.0,
            dynamics_drive: 0.0,
            dynamics_mix: 1.0,
        }
    }

    fn sig(i: usize) -> (f32, f32) {
        let x = 0.4 * (TAU * 220.0 * i as f32 / SR).sin();
        let y = -0.3 * (TAU * 330.0 * i as f32 / SR).cos();
        (x, y)
    }

    #[test]
    fn all_off_is_bit_exact_passthrough() {
        // The default patch has every effect off: the chain must be a bit-exact
        // passthrough from the first sample (each kernel's `WetFade` snaps to
        // bypassed on its first `set`), so render-parity vs a no-FX render
        // holds.
        let mut fx = FxChain::new(SR);
        fx.set_params(&all_off());
        for i in 0..2_000 {
            let (x, y) = sig(i);
            let mut l = [x];
            let mut r = [y];
            fx.process_block(&mut l, &mut r);
            assert_eq!(l[0].to_bits(), x.to_bits(), "L not bit-exact at i={i}");
            assert_eq!(r[0].to_bits(), y.to_bits(), "R not bit-exact at i={i}");
        }
    }

    #[test]
    fn enabling_an_effect_changes_the_output() {
        // Dynamics on with heavy drive must audibly diverge from the dry input
        // once the bypass fade has ridden up.
        let mut fx = FxChain::new(SR);
        let mut p = all_off();
        p.dynamics_on = true;
        p.dynamics_drive = 30.0;
        p.dynamics_mix = 1.0;
        fx.set_params(&p);
        let mut diverged = false;
        for i in 0..4_000 {
            let (x, y) = sig(i);
            let mut l = [x];
            let mut r = [y];
            fx.process_block(&mut l, &mut r);
            if (l[0] - x).abs() > 1.0e-3 {
                diverged = true;
            }
        }
        assert!(diverged, "dynamics on did not change the signal");
    }

    /// Drive a slot on, then off, and assert the two properties every
    /// internal-`WetFade` slot owes: it settles back to a bit-exact skip, and
    /// its switch-off glides instead of stepping.
    ///
    /// Shared because 0228-0232 each need it — no slot has an outer fade left,
    /// so this is the only cover any of their bypasses has. It replaced a
    /// bespoke `toggling_off_settles_back_to_bit_exact_skip`, which asserted a
    /// subset of the same thing against whichever slot still had a 10 ms outer
    /// fade to be the last of.
    fn assert_internal_fade_slot(name: &str, on: fn(&mut FxParams), off: fn(&mut FxParams)) {
        let mut fx = FxChain::new(SR);
        let mut p = all_off();
        on(&mut p);
        fx.set_params(&p);

        let mut diverged = false;
        let mut before = 0.0;
        for i in 0..4_000 {
            let (x, y) = sig(i);
            let (mut l, mut r) = ([x], [y]);
            fx.process_block(&mut l, &mut r);
            if (l[0] - x).abs() > 1.0e-3 {
                diverged = true;
            }
            before = l[0];
        }
        assert!(diverged, "{name} on did not change the signal");

        off(&mut p);
        fx.set_params(&p);
        let (x, y) = sig(4_000);
        let (mut l, mut r) = ([x], [y]);
        fx.process_block(&mut l, &mut r);
        assert!(
            (l[0] - before).abs() < 0.05,
            "{name} switch-off stepped the output: {before} -> {}",
            l[0]
        );
        assert!(
            (l[0] - x).abs() > 1.0e-5,
            "{name} switch-off was instant (already dry at {})",
            l[0]
        );

        // The kernels' fades are 30 ms, longer than the chain's 10 ms outer
        // one; 0.6 s clears a 30 ms one-pole's snap floor with margin.
        for i in 0..(SR * 0.6) as usize {
            let (x, y) = sig(i);
            let (mut l, mut r) = ([x], [y]);
            fx.process_block(&mut l, &mut r);
        }
        for i in 0..1_000 {
            let (x, y) = sig(i);
            let (mut l, mut r) = ([x], [y]);
            fx.process_block(&mut l, &mut r);
            assert_eq!(l[0].to_bits(), x.to_bits(), "{name} L not skipped after settle i={i}");
            assert_eq!(r[0].to_bits(), y.to_bits(), "{name} R not skipped after settle i={i}");
        }
    }

    #[test]
    fn phaser_slot_bypass_fades_and_settles() {
        assert_internal_fade_slot(
            "phaser",
            |p| {
                p.phaser_on = true;
                p.phaser_mix = 1.0;
                p.phaser_feedback = 0.7;
            },
            |p| p.phaser_on = false,
        );
    }

    #[test]
    fn chorus_slot_bypass_fades_and_settles() {
        assert_internal_fade_slot(
            "chorus",
            |p| {
                p.chorus_on = true;
                p.chorus_mix = 1.0;
                p.chorus_depth = 0.8;
            },
            |p| p.chorus_on = false,
        );
    }

    #[test]
    fn reverb_slot_bypass_fades_and_settles() {
        assert_internal_fade_slot(
            "reverb",
            |p| {
                p.reverb_on = true;
                p.reverb_mix = 1.0;
                p.reverb_decay = 2.0;
            },
            |p| p.reverb_on = false,
        );
    }

    #[test]
    fn delay_slot_bypass_fades_and_settles() {
        assert_internal_fade_slot(
            "delay",
            |p| {
                p.delay_on = true;
                p.delay_mix = 1.0;
                p.delay_time = 0.05;
                p.delay_feedback = 0.5;
            },
            |p| p.delay_on = false,
        );
    }

    #[test]
    fn dynamics_slot_bypass_fades_and_settles() {
        assert_internal_fade_slot(
            "dynamics",
            |p| {
                p.dynamics_on = true;
                p.dynamics_mix = 1.0;
                p.dynamics_drive = 24.0;
                p.dynamics_threshold = -30.0;
            },
            |p| p.dynamics_on = false,
        );
    }

    #[test]
    fn reset_snaps_to_bypass() {
        let mut fx = FxChain::new(SR);
        let mut p = all_off();
        p.reverb_on = true;
        fx.set_params(&p);
        for i in 0..1_000 {
            let (x, y) = sig(i);
            let mut l = [x];
            let mut r = [y];
            fx.process_block(&mut l, &mut r);
        }
        fx.reset();
        // After reset every slot is off and snapped to 0 — even with reverb_on
        // still latched false, the chain is an immediate passthrough.
        fx.set_params(&all_off());
        let (x, y) = sig(0);
        let mut l = [x];
        let mut r = [y];
        fx.process_block(&mut l, &mut r);
        assert_eq!(l[0].to_bits(), x.to_bits(), "reset did not restore skip");
        assert_eq!(r[0].to_bits(), y.to_bits(), "reset did not restore skip");
    }
}
