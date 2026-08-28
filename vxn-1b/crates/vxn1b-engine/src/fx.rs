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
//! Each of the five slots carries a short bypass **fade** (a `Smoothed` 0..1) and
//! is gated on it:
//!
//! - **Steady off** (`on == false` and the fade has reached exactly 0) is a
//!   *true skip* — the kernel's `process` is never called, so five idle effects
//!   cost five gate checks, not five wet=0 multiplies through the DSP (E037 CPU
//!   risk).
//! - **Toggling** ramps the fade 0↔1 over [`FX_FADE_MS`], crossfading dry input
//!   against the kernel's wet output so a switch doesn't click. `Smoothed` snaps
//!   to its target within `SNAP_EPS`, so the fade genuinely reaches 0 and the
//!   true-skip gate re-arms.
//!
//! The kernels are held **internally on** and their own `mix` argument carries
//! the musical wet amount; this outer fade owns bypass. That's the same split
//! VXN1's `MasterFx` uses for its reverb (bypass owned by the outer fade, kernel
//! `on` held true). On an off→on edge the slot's kernel state is cleared so a
//! re-enabled delay / reverb doesn't dump a stale tail.

use std::sync::Arc;

use vxn_core_utils::{MeterBus, MeterTap};
use vxn_dsp::{
    DynamicsBlock, DynamicsParams, FdnReverb, FdnReverbParams, StereoChorus, StereoDelay,
    StereoPhaser,
};

use crate::params::{ParamId, Params};

/// Bypass fade length. Long enough to mask an on/off click, short enough to feel
/// instant — matches VXN1's 10 ms FX toggle fade.
const FX_FADE_MS: f32 = 10.0;

/// Delay high-frequency damping in the feedback path. Fixed internal default
/// (not a param) — mirrors VXN1's hardcoded `0.3`.
const DELAY_DAMPING: f32 = 0.3;

/// Longest delay time the ring buffer must hold. Allocated once at
/// construction. Deliberately **twice** the `delay_time` param ceiling (2 s):
/// free-run mode can only ask for 2 s, but tempo sync resolves a subdivision
/// *period*, and the slow end of the table runs well past the knob's range —
/// `1/1` is 4 s at 60 BPM. Sizing the line to the knob would have silently
/// clamped those, so the label would read `1/1` while the ear heard 2 s (0267).
/// Costs ~768 KB more than a 2 s line at 48 kHz, per instance.
pub(crate) const DELAY_MAX_SECONDS: f32 = 4.0;

// Slot indices into the fade / on-state arrays, in chain order. Dynamics runs
// FIRST (input compression / drive ahead of the modulation + time effects),
// matching the faceplate order (Dynamics left of FX) and VXN2's FX bus.
const DYNAMICS: usize = 0;
const CHORUS: usize = 1;
const PHASER: usize = 2;
const DELAY: usize = 3;
const REVERB: usize = 4;
const N_SLOTS: usize = 5;

/// Block-rate snapshot of the FX params, fanned into the chain each control
/// block. Character values map straight to each kernel's setter; the `*_on`
/// bools drive the bypass fades.
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
    /// Bypass fade per slot (0 = fully bypassed / skipped, 1 = fully wet).
    fades: [vxn_dsp::smoothing::Smoothed; N_SLOTS],
    /// Meter publish target (0240/0241). `None` until the engine attaches one,
    /// so a bare `FxChain` (unit tests) runs with no metering and no branch cost
    /// beyond this check once per block.
    meters: Option<Arc<MeterBus>>,
    /// Latched on-state per slot — drives the fade target and the rising-edge
    /// state-clear.
    on: [bool; N_SLOTS],
}

/// One serial FX slot's per-sample runner: skip entirely while bypassed and
/// settled, otherwise run the kernel and crossfade it against the dry input.
///
/// Five byte-identical copies of this before 0319, differing only in the slot
/// constant and the kernel field. A `dyn`-dispatched slot list would read
/// better and be a deoptimisation — this is per-sample hot — so the repetition
/// moves into a macro that expands to exactly the code that was there.
macro_rules! fx_slot {
    ($run:ident, $slot:ident, $kernel:ident) => {
        #[inline]
        fn $run(&mut self, xl: f32, xr: f32) -> (f32, f32) {
            if !self.on[$slot] && self.fades[$slot].current() == 0.0 {
                return (xl, xr);
            }
            let (wl, wr) = self.$kernel.process(xl, xr);
            blend(xl, xr, wl, wr, self.fades[$slot].tick())
        }
    };
}

impl FxChain {
    pub fn new(sample_rate: f32) -> Self {
        let fade = vxn_dsp::smoothing::Smoothed::new(0.0, FX_FADE_MS, sample_rate);
        Self {
            chorus: StereoChorus::new(sample_rate),
            phaser: StereoPhaser::new(sample_rate),
            delay: StereoDelay::new(sample_rate, DELAY_MAX_SECONDS),
            reverb: FdnReverb::new(sample_rate),
            dynamics: DynamicsBlock::new(sample_rate),
            fades: [fade; N_SLOTS],
            meters: None,
            on: [false; N_SLOTS],
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
        self.chorus.clear();
        self.phaser.clear();
        self.delay.clear();
        self.reverb.reset();
        self.dynamics.clear();
        for f in &mut self.fades {
            f.snap(0.0);
        }
        self.on = [false; N_SLOTS];
    }

    /// Fan a block-rate param snapshot into the kernels and retarget the bypass
    /// fades. Character params always update (cheap; ready for the moment a slot
    /// re-activates); the kernels are held internally on, so their own `mix`
    /// carries the wet amount and this chain's fade owns bypass.
    pub fn set_params(&mut self, p: &FxParams) {
        self.retarget(CHORUS, p.chorus_on);
        self.retarget(PHASER, p.phaser_on);
        self.retarget(DELAY, p.delay_on);
        self.retarget(REVERB, p.reverb_on);
        self.retarget(DYNAMICS, p.dynamics_on);

        self.chorus.set_params(p.chorus_rate, p.chorus_depth, p.chorus_mix);
        self.phaser.set_params(
            p.phaser_rate,
            p.phaser_depth,
            p.phaser_feedback,
            p.phaser_mix,
            p.phaser_stereo,
        );
        self.delay.set_params(
            p.delay_time,
            p.delay_time,
            p.delay_feedback,
            DELAY_DAMPING,
            p.delay_mix,
            p.delay_pingpong,
        );
        self.reverb.set_params(&FdnReverbParams {
            on: true,
            size: p.reverb_size,
            decay_secs: p.reverb_decay,
            damp: p.reverb_damp,
            mix: p.reverb_mix,
        });
        self.dynamics.set_from(&DynamicsParams {
            on: true,
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

    /// Latch a slot's on-state, retarget its fade, and clear the kernel on the
    /// off→on edge so a re-enabled tail-carrying effect starts clean.
    fn retarget(&mut self, slot: usize, on: bool) {
        if on && !self.on[slot] {
            self.clear_slot(slot);
        }
        self.on[slot] = on;
        self.fades[slot].set_target(if on { 1.0 } else { 0.0 });
    }

    /// Clear one slot's kernel state. `DYNAMICS` is spelled out rather than
    /// left as the catch-all: as `_ =>` it meant any bogus index silently reset
    /// the compressor, which is a real state change attributed to the wrong
    /// slot. An unknown index is now a no-op.
    fn clear_slot(&mut self, slot: usize) {
        match slot {
            CHORUS => self.chorus.clear(),
            PHASER => self.phaser.clear(),
            DELAY => self.delay.clear(),
            REVERB => self.reverb.reset(),
            DYNAMICS => self.dynamics.clear(),
            _ => {}
        }
    }

    fx_slot!(run_dynamics, DYNAMICS, dynamics);
    fx_slot!(run_chorus, CHORUS, chorus);
    fx_slot!(run_phaser, PHASER, phaser);
    fx_slot!(run_delay, DELAY, delay);
    fx_slot!(run_reverb, REVERB, reverb);
}

/// Linear bypass crossfade: `g = 0` → dry input, `g = 1` → kernel wet output.
#[inline]
fn blend(dry_l: f32, dry_r: f32, wet_l: f32, wet_r: f32, g: f32) -> (f32, f32) {
    (dry_l + g * (wet_l - dry_l), dry_r + g * (wet_r - dry_r))
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
        // passthrough from the first sample (fades start snapped to 0), so
        // render-parity vs a no-FX render holds.
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

    #[test]
    fn toggling_off_settles_back_to_bit_exact_skip() {
        // Turn an effect on, run it, then turn it off: after the fade settles the
        // slot must return to a bit-exact passthrough (the true-skip path).
        let mut fx = FxChain::new(SR);
        let mut p = all_off();
        p.delay_on = true;
        p.delay_mix = 0.5;
        fx.set_params(&p);
        for i in 0..4_000 {
            let (x, y) = sig(i);
            let mut l = [x];
            let mut r = [y];
            fx.process_block(&mut l, &mut r);
        }
        // Switch off and let the fade reach exactly 0.
        p.delay_on = false;
        fx.set_params(&p);
        for i in 0..(SR * 0.2) as usize {
            let (x, y) = sig(i);
            let mut l = [x];
            let mut r = [y];
            fx.process_block(&mut l, &mut r);
        }
        // Now settled off — assert bit-exact passthrough.
        for i in 0..1_000 {
            let (x, y) = sig(i);
            let mut l = [x];
            let mut r = [y];
            fx.process_block(&mut l, &mut r);
            assert_eq!(l[0].to_bits(), x.to_bits(), "L not skipped after settle i={i}");
            assert_eq!(r[0].to_bits(), y.to_bits(), "R not skipped after settle i={i}");
        }
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
