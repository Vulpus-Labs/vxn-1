//! One voice = 6 operators + a DX7 algorithm graph + voice-level state
//! (played note, velocity, gate, pitch, stack metadata).
//!
//! Ticket 0003 deliverables. Stack metadata (`voice_idx`, `voice_spread`,
//! `voice_rand`) is stored here so the stacking pass (0005) populates it at
//! allocation time without refactoring the tick path. Per-block resolved
//! modulation arrives via [`VoiceMod`] — a stub until 0008 lands the matrix.
//!
//! Two tick paths exposed:
//!
//! - [`voice_tick`] — mono sum of carrier outputs. Cheapest; suitable for
//!   the polyphony allocator's idle-detection pass and for benches.
//! - [`voice_tick_stereo`] — applies the per-op `pan` parameter to each
//!   carrier and returns `(L, R)`. Becomes the default path once stacking
//!   (0005) lands and `voice_spread` needs to write into pan.
//!
//! ## Sample-delay convention
//!
//! Matches [`crate::algo`]: the algorithm router consumes the prev-sample op
//! outputs to produce this-sample modulation inputs and the carrier bus. The
//! voice stores its own `prev_outs` and updates them at the end of each tick.

use crate::algo::{N_OPS, RouteFn, resolve_route, spec_of};
use crate::eg::EgStage;
use crate::envelope::{ModEnvParams, ModEnvState, PitchEgParams, PitchEgState};
use crate::lfo::Lfo2Params;
use crate::op::{OpParams, OpState, op_eg_tick, op_tick};

/// Patch-level parameters for one voice: per-op state + algorithm + voice-
/// global tune offset + per-voice LFO (LFO2) + patch-wide modulation
/// envelopes (Pitch EG + Mod Env, ticket 0007).
#[derive(Clone, Copy, Debug)]
pub struct VoiceParams {
    pub ops: [OpParams; N_OPS],
    pub algo: u8,
    pub master_tune_cents: f32,
    pub lfo2: Lfo2Params,
    pub pitch_eg: PitchEgParams,
    /// Pitch EG global depth in semitones at full-scale (l = ±99). Default
    /// 1.0 → ±1 semitone. Matrix routing can amplify further by reading
    /// `pitch_eg.level_st` and applying its own depth.
    pub peg_depth: f32,
    pub mod_env: ModEnvParams,
}

impl Default for VoiceParams {
    fn default() -> Self {
        Self {
            ops: [OpParams::default(); N_OPS],
            algo: 1,
            master_tune_cents: 0.0,
            lfo2: Lfo2Params::default(),
            pitch_eg: PitchEgParams::default(),
            peg_depth: 1.0,
            mod_env: ModEnvParams::default(),
        }
    }
}

/// Per-block resolved modulation input. Stubbed for ticket 0003; the mod
/// matrix (0008) will fill it in. Fields are placeholders — present so the
/// `voice_tick` signature is stable and 0008 plugs in without API churn.
#[derive(Clone, Copy, Debug, Default)]
pub struct VoiceMod {
    pub pitch_offset_st: f32,
    pub global_level: f32,
}

/// One voice. Holds six operator states + voice-scope state + a cached
/// algorithm-router function pointer.
#[derive(Clone, Copy, Debug)]
pub struct Voice {
    pub ops: [OpState; N_OPS],
    pub note: u8,
    pub velocity: u8,
    pub gate: bool,
    /// Cooked phase increment per op, BEFORE bend/glide multiplier. Snapshot
    /// at note-on. The live `ops[i].phase_inc` = `base_phase_inc[i] *
    /// 2^((bend_st + glide_st) / 12)`. Master-tune is already baked into the
    /// snapshot (it's patch-constant).
    pub base_phase_inc: [u32; N_OPS],
    /// Current pitch bend in semitones. Driven by the allocator's `set_bend`.
    pub bend_st: f32,
    /// Current glide offset in semitones relative to the played note. Driven
    /// by the allocator each block while a glide is in progress.
    pub glide_st: f32,
    /// Stack-instance index in [0, density). Populated by 0005. Default 0.
    pub voice_idx: u8,
    /// Symmetric stack position in [-1, +1]. Populated by 0005. Default 0.
    pub voice_spread: f32,
    /// Per-instance random in [0, 1). Captured at note-on. Used for stack
    /// decorrelation (LFO2 phase scatter, etc.). Default 0.5 until 0005
    /// wires a proper RNG.
    pub voice_rand: f32,
    /// Active algorithm number (1..=32). Cached alongside `route_fn` so the
    /// stereo path can read [`spec_of`] without a re-resolve.
    pub algo: u8,
    pub route_fn: RouteFn,
    /// Prev-sample op outputs, fed to the router each tick.
    pub prev_outs: [f32; N_OPS],
    /// Patch-wide Pitch EG (ticket 0007). Output in semitones; default
    /// routing adds into the voice pitch sum.
    pub pitch_eg: PitchEgState,
    /// Patch-wide Mod Env (ticket 0007). Matrix-only source; no default
    /// routing — voice state simply ticks it so the matrix can read it.
    pub mod_env: ModEnvState,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            ops: [OpState::default(); N_OPS],
            note: 0,
            velocity: 0,
            gate: false,
            base_phase_inc: [0; N_OPS],
            bend_st: 0.0,
            glide_st: 0.0,
            voice_idx: 0,
            voice_spread: 0.0,
            voice_rand: 0.5,
            algo: 1,
            route_fn: resolve_route(1),
            prev_outs: [0.0; N_OPS],
            pitch_eg: PitchEgState::default(),
            mod_env: ModEnvState::default(),
        }
    }
}

impl Voice {
    /// Note-on: capture played note + velocity, set the gate, re-cook every
    /// op against the new key/velocity/sample_rate, and trigger EG attack.
    /// `voice_rand` is left at its prior value — 0005 populates it from a
    /// per-allocation RNG. `prev_outs` is cleared to avoid carrying tail
    /// energy from a previous voice life. `bend_st` is preserved (channel-
    /// wide state owned by the allocator); `glide_st` is reset to 0 and
    /// re-driven by the allocator if portamento is active.
    pub fn note_on(
        &mut self,
        params: &VoiceParams,
        note: u8,
        velocity: u8,
        sample_rate: f32,
    ) {
        self.note = note;
        self.velocity = velocity;
        self.gate = true;
        self.algo = params.algo;
        self.route_fn = resolve_route(params.algo);
        let master_mult = 2_f32.powf(params.master_tune_cents / 1200.0);
        for i in 0..N_OPS {
            self.ops[i].cook(&params.ops[i], note, velocity, sample_rate);
            let base = (self.ops[i].phase_inc as f64 * master_mult as f64) as u32;
            self.base_phase_inc[i] = base;
            self.ops[i].eg.note_on();
        }
        self.pitch_eg.cook(&params.pitch_eg, params.peg_depth, 1.0);
        self.pitch_eg.note_on();
        self.mod_env.cook(&params.mod_env);
        self.mod_env.note_on();
        self.glide_st = 0.0;
        self.apply_pitch_mult();
        self.prev_outs = [0.0; N_OPS];
    }

    /// Re-cook EG targets/rates without resetting phase or restarting the EG.
    /// Used by Solo legato note changes: pitch glides, EG continues.
    pub fn retarget_pitch(
        &mut self,
        params: &VoiceParams,
        note: u8,
        velocity: u8,
        sample_rate: f32,
    ) {
        self.note = note;
        self.velocity = velocity;
        let master_mult = 2_f32.powf(params.master_tune_cents / 1200.0);
        for i in 0..N_OPS {
            // Cook recomputes phase_inc + EG targets/rates from the new note,
            // but doesn't touch phase, eg.level, eg.stage, or fb memory —
            // exactly what legato wants.
            self.ops[i].cook(&params.ops[i], note, velocity, sample_rate);
            let base = (self.ops[i].phase_inc as f64 * master_mult as f64) as u32;
            self.base_phase_inc[i] = base;
        }
        self.apply_pitch_mult();
    }

    /// Note-off: drop gate, transition every op EG + patch envelopes to
    /// release.
    pub fn note_off(&mut self) {
        self.gate = false;
        for op in &mut self.ops {
            op.eg.note_off();
        }
        self.pitch_eg.note_off();
        self.mod_env.note_off();
    }

    /// Apply current `bend_st + glide_st + pitch_eg.level_st` to
    /// `ops[i].phase_inc`, rederiving from `base_phase_inc`. Call after
    /// mutating bend / glide, or once per control tick after [`eg_tick`] so
    /// the per-sample loop sees the freshly ticked PEG.
    #[inline]
    pub fn apply_pitch_mult(&mut self) {
        let total_st = self.bend_st + self.glide_st + self.pitch_eg.level_st;
        let mult = 2_f32.powf(total_st / 12.0) as f64;
        for i in 0..N_OPS {
            self.ops[i].phase_inc = (self.base_phase_inc[i] as f64 * mult) as u32;
        }
    }

    /// Set channel-wide pitch bend in semitones (e.g. ±2 for standard range).
    #[inline]
    pub fn set_bend(&mut self, semitones: f32) {
        self.bend_st = semitones;
        self.apply_pitch_mult();
    }

    /// Set current glide offset (semitones from played note). Allocator drives
    /// this each block while a glide is in progress.
    #[inline]
    pub fn set_glide(&mut self, semitones: f32) {
        self.glide_st = semitones;
        self.apply_pitch_mult();
    }

    /// True when every op EG has reached `Idle` (release tail decayed past
    /// L4). The polyphony allocator (0004) uses this to free voices.
    #[inline]
    pub fn is_idle(&self) -> bool {
        self.ops.iter().all(|o| o.eg.stage == EgStage::Idle)
    }

    /// Advance every op's EG + the patch envelopes (Pitch EG + Mod Env) one
    /// control tick (typically once per block). Re-applies the pitch mult so
    /// the per-sample loop picks up the freshly ticked Pitch EG offset.
    #[inline]
    pub fn eg_tick(&mut self, dt: f32) {
        for op in &mut self.ops {
            op_eg_tick(op, dt);
        }
        self.pitch_eg.tick(dt);
        self.mod_env.tick(dt);
        self.apply_pitch_mult();
    }
}

/// Mono per-sample tick: route modulation, tick all 6 ops, return the
/// carrier-bus sum (one sample behind the new op outputs — see module docs).
///
/// `_modulation` is the per-block resolved matrix output; unused in this
/// ticket beyond signature stability.
#[inline]
pub fn voice_tick(voice: &mut Voice, _modulation: &VoiceMod) -> f32 {
    let (mi, carrier_sum) = (voice.route_fn)(&voice.prev_outs);
    let mut new_outs = [0.0_f32; N_OPS];
    for i in 0..N_OPS {
        new_outs[i] = op_tick(&mut voice.ops[i], mi[i]);
    }
    voice.prev_outs = new_outs;
    carrier_sum
}

/// Stereo per-sample tick: same routing + ticking as [`voice_tick`], but
/// pans each carrier's prev-output to `(L, R)` via the per-op `pan` param.
/// Equal-power pan: `pan = -1` → fully left, `pan = 0` → centre (each
/// channel at `cos(π/4) ≈ 0.707`), `pan = +1` → fully right.
#[inline]
pub fn voice_tick_stereo(
    voice: &mut Voice,
    params: &VoiceParams,
    _modulation: &VoiceMod,
) -> (f32, f32) {
    let (mi, _carrier_sum_mono) = (voice.route_fn)(&voice.prev_outs);
    let mut new_outs = [0.0_f32; N_OPS];
    for i in 0..N_OPS {
        new_outs[i] = op_tick(&mut voice.ops[i], mi[i]);
    }
    let spec = spec_of(voice.algo);
    let mut l = 0.0_f32;
    let mut r = 0.0_f32;
    for i in 0..N_OPS {
        if (spec.carriers >> i) & 1 == 1 {
            let pan = params.ops[i].pan.clamp(-1.0, 1.0);
            // pan ∈ [-1,+1] → theta ∈ [0, π/2]
            let theta = (pan + 1.0) * (core::f32::consts::FRAC_PI_4);
            let (s, c) = theta.sin_cos();
            l += voice.prev_outs[i] * c;
            r += voice.prev_outs[i] * s;
        }
    }
    voice.prev_outs = new_outs;
    (l, r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eg::EgStage;
    fn carrier_friendly_patch() -> VoiceParams {
        // Algo 32: all 6 ops are carriers, no edges — exercises every op's
        // own path with no inter-op coupling. Set every op to a quick
        // release so is_idle() is reachable in tests.
        let mut ops = [OpParams::default(); N_OPS];
        for op in &mut ops {
            // Fast release (R4=99 → ~4ms sweep).
            op.eg.r[3] = 99;
        }
        VoiceParams {
            ops,
            algo: 32,
            ..VoiceParams::default()
        }
    }

    #[test]
    fn note_on_triggers_attack_and_captures_state() {
        let params = carrier_friendly_patch();
        let mut voice = Voice::default();
        voice.note_on(&params, 60, 100, 48_000.0);
        assert!(voice.gate);
        assert_eq!(voice.note, 60);
        assert_eq!(voice.velocity, 100);
        for op in &voice.ops {
            assert_eq!(op.eg.stage, EgStage::Attack);
        }
    }

    #[test]
    fn fresh_voice_is_idle() {
        let voice = Voice::default();
        assert!(voice.is_idle());
    }

    #[test]
    fn note_off_transitions_to_release_then_idle() {
        let params = carrier_friendly_patch();
        let mut voice = Voice::default();
        voice.note_on(&params, 60, 100, 48_000.0);
        // Let it settle a bit.
        let dt = 1.0 / 48_000.0;
        for _ in 0..(48_000 / 4) {
            voice.eg_tick(dt);
        }
        voice.note_off();
        assert!(!voice.gate);
        // L4=0 default + R4=99 → idle in ~ms.
        let mut steps = 0;
        while !voice.is_idle() && steps < 48_000 {
            voice.eg_tick(dt);
            steps += 1;
        }
        assert!(voice.is_idle(), "voice never went idle after note_off");
    }

    #[test]
    fn voice_tick_produces_finite_output() {
        let params = carrier_friendly_patch();
        let mut voice = Voice::default();
        voice.note_on(&params, 60, 100, 48_000.0);
        let modu = VoiceMod::default();
        voice.eg_tick(64.0 / 48_000.0);
        for _ in 0..64 {
            let s = voice_tick(&mut voice, &modu);
            assert!(s.is_finite());
        }
    }

    #[test]
    fn voice_tick_silent_when_idle() {
        // Idle voice (no note_on) → all op EGs at Idle stage, level 0.
        // The router pulls prev_outs which are zero; output is zero.
        let mut voice = Voice::default();
        let modu = VoiceMod::default();
        let mut peak = 0.0_f32;
        for _ in 0..256 {
            let s = voice_tick(&mut voice, &modu).abs();
            if s > peak {
                peak = s;
            }
        }
        assert_eq!(peak, 0.0);
    }

    #[test]
    fn algo_changes_route_fn() {
        let mut params = carrier_friendly_patch();
        params.algo = 1;
        let mut voice = Voice::default();
        voice.note_on(&params, 60, 100, 48_000.0);
        assert_eq!(voice.algo, 1);
    }

    #[test]
    fn stereo_centre_pan_equal_channels() {
        let params = carrier_friendly_patch(); // all pan = 0.0
        let mut voice = Voice::default();
        voice.note_on(&params, 60, 100, 48_000.0);
        // Force into sustain so output is non-trivial immediately.
        for op in &mut voice.ops {
            op.force_sustain(0.4);
        }
        let modu = VoiceMod::default();
        // Run a few samples to populate prev_outs.
        for _ in 0..8 {
            voice_tick_stereo(&mut voice, &params, &modu);
        }
        let (l, r) = voice_tick_stereo(&mut voice, &params, &modu);
        assert!(
            (l - r).abs() < 1e-5,
            "centre pan should split equally, got L={l} R={r}"
        );
    }

    #[test]
    fn stereo_hard_left_silences_right() {
        let mut params = carrier_friendly_patch();
        // Pan every op fully left.
        for op in &mut params.ops {
            op.pan = -1.0;
        }
        let mut voice = Voice::default();
        voice.note_on(&params, 60, 100, 48_000.0);
        for op in &mut voice.ops {
            op.force_sustain(0.4);
        }
        let modu = VoiceMod::default();
        let mut peak_r = 0.0_f32;
        for _ in 0..256 {
            let (_l, r) = voice_tick_stereo(&mut voice, &params, &modu);
            if r.abs() > peak_r {
                peak_r = r.abs();
            }
        }
        assert!(peak_r < 1e-4, "hard-left pan leaked into R: {peak_r}");
    }

    #[test]
    fn master_tune_shifts_phase_inc() {
        let mut params = carrier_friendly_patch();
        let mut a = Voice::default();
        a.note_on(&params, 60, 100, 48_000.0);
        let inc_a = a.ops[0].phase_inc;
        params.master_tune_cents = 1200.0; // one octave up
        let mut b = Voice::default();
        b.note_on(&params, 60, 100, 48_000.0);
        let inc_b = b.ops[0].phase_inc;
        // Allow 1 ULP of f64-rounding slack.
        let ratio = inc_b as f64 / inc_a as f64;
        assert!(
            (ratio - 2.0).abs() < 1e-6,
            "+1200 ct should double phase_inc, got ratio {ratio}"
        );
    }
}
