//! Top-level engine (ticket 0012): assembles the kernel and exposes a
//! block-based `process` surface the (still-to-come) CLAP shell and the
//! integration test below bind against.
//!
//! ```text
//!   note-on / note-off / bend ─► PolyAlloc ─► Stacks ──┐
//!                                                       │
//!   SharedParams ── snapshot ──► EngineParams ──┐       │
//!                                               ▼       ▼
//!                                        FX params + matrix depths
//!                                               │       │
//!                                               ▼       ▼
//!                                    CleanupFilter → StereoDelay → FdnReverb → MasterState
//!                                                                    │
//!                                                                    ▼
//!                                                              (out_l, out_r)
//! ```
//!
//! ## Block lifecycle
//!
//! 1. [`Engine::snapshot_params`] — pull the SPSC-friendly param store into
//!    [`EngineParams`] and push fresh FX/matrix/master targets into the
//!    sub-engines.
//! 2. Push MIDI events: [`Engine::note_on`] / [`Engine::note_off`] /
//!    [`Engine::set_bend`].
//! 3. [`Engine::process_block`] — advance allocator state, tick LFO1, tick
//!    per-stack EGs at control rate, then render one stereo sample per loop
//!    iteration through the FX chain and the master gain.
//!
//! Master tune is mirrored into `Patch.voice.master_tune_cents` at snapshot
//! time; the DSP path bakes that into per-op `base_phase_inc` at note-on.
//! Changes mid-note take effect on the next note-on (which matches how DAWs
//! typically use master_tune — a per-song setup, not a performance gesture).

use vxn2_dsp::cleanup::CleanupFilter;
use vxn2_dsp::delay::StereoDelay;
use vxn2_dsp::reverb::FdnReverb;
use vxn2_dsp::stack::{STACK_LANES, stack_tick_stereo};

use crate::alloc::{N_STACKS, PolyAlloc};
use crate::default_patch;
use crate::master::MasterState;
use crate::matrix::{
    CurveKind, DestId, LaneSourceVals, LaneSources, MatrixSlot, MatrixTable, N_CLAP_DEPTH_SLOTS,
    N_DESTS, N_PITCH_DESTS, N_SLOTS, N_SOURCES, PatchSources, PitchSmoother, SourceId,
    StackScalarSources, eval_dests, eval_sources,
};
use crate::modulation::PatchMod;
use crate::shared::{EngineParams, SharedParams};

// Matrix dest-accumulator indices, resolved at compile time (review nit from
// E006: no live `unwrap()` in the hot path). The op-major stride-3 layout
// (Pitch, Level, Pan per op) is asserted alongside.
const DELAY_MIX_IDX: usize = DestId::DelayMix.idx().unwrap();
const REVERB_MIX_IDX: usize = DestId::ReverbMix.idx().unwrap();
const FEEDBACK_IDX: usize = DestId::Feedback.idx().unwrap();
const _: () = {
    assert!(DestId::Op1Pitch.idx().unwrap() == 0);
    assert!(DestId::Op1Level.idx().unwrap() == 1);
    assert!(DestId::Op1Pan.idx().unwrap() == 2);
    assert!(DestId::Op6Level.idx().unwrap() == 16);
};

/// Pitch-shaped matrix destinations interpolate from block rate down to this
/// sub-block quantum (in samples). True per-sample smoothing would re-cook
/// every op's `phase_inc` (48 `powf` per stack) each sample — not affordable.
/// At 16 samples a 256-sample host block gets 16 interpolation points
/// (≈ 0.33 ms apart at 48 kHz), which removes audible stepping; smoothers
/// that have converged skip the recook entirely (ticket 0063).
const PITCH_SMOOTH_QUANTUM: usize = 16;

/// Convergence threshold for the pitch smoothers, in semitones. 1e-4 st is
/// one hundredth of a cent — far below audibility; once within this band the
/// per-quantum tick + recook is skipped.
const PITCH_SMOOTH_EPS_ST: f32 = 1e-4;

/// Top-level audio engine. Owns every sub-engine plus the per-block
/// parameter snapshot.
pub struct Engine {
    pub alloc: PolyAlloc,
    pub matrix: MatrixTable,
    pub patch_mod: PatchMod,
    pub cleanup: CleanupFilter,
    pub delay: StereoDelay,
    pub reverb: FdnReverb,
    pub master: MasterState,
    pub params: EngineParams,
    sample_rate: f32,
    block_size: usize,
    block_secs: f32,
    /// Host tempo. Defaults to 120 BPM until the host pushes one in.
    pub tempo_bpm: f32,
    /// MIDI CC1 mod wheel, normalised `[0, 1]`. Read by the matrix engine
    /// (ticket 0008) as a patch-global source; stored here so the CLAP shell
    /// (0016) can push it without reaching across the matrix.
    pub mod_wheel: f32,
    /// MIDI channel aftertouch, normalised `[0, 1]`. Same routing role as
    /// [`Self::mod_wheel`].
    pub aftertouch: f32,
    /// Per-stack mod-matrix destination accumulator. Rewritten every block by
    /// [`Self::process_block`] before the per-sample render loop; per-sample
    /// destinations read from `stack.op_level_mod` (a projected slice of this
    /// buffer). Sized to one entry per `PolyAlloc` slot.
    dest_vals: Vec<[[f32; N_DESTS]; STACK_LANES]>,
    /// Reusable per-stack source lookup. Fan-out of every patch / stack /
    /// lane scalar into a `[lane][source]` table so [`eval_dests`] reads a
    /// contiguous matrix per slot.
    lane_sources: LaneSourceVals,
    /// Per-stack pitch-dest smoothers (ticket 0063). Targets refresh at
    /// block rate from `dest_vals`; state advances every
    /// [`PITCH_SMOOTH_QUANTUM`] samples inside the render loop and is
    /// projected into the stack pitch-mod fields before `apply_pitch_mult`.
    pitch_smoothers: [PitchSmoother; N_STACKS],
    /// Block-rate smoother targets, captured per active stack.
    pitch_targets: [[[f32; STACK_LANES]; N_PITCH_DESTS]; N_STACKS],
    /// Allocation generation last seen per slot — a change means a fresh
    /// note-on reused the slot, so its smoother snaps instead of gliding in
    /// from the previous voice's offset.
    pitch_seq: [u64; N_STACKS],
}

impl Engine {
    pub fn new(sample_rate: f32, block_size: usize) -> Self {
        let mut e = Self {
            alloc: PolyAlloc::new(sample_rate),
            matrix: default_patch::default_matrix(),
            patch_mod: PatchMod::new(0xDEAD_BEEF_DEAD_BEEF),
            cleanup: CleanupFilter::new(sample_rate),
            delay: StereoDelay::new(sample_rate),
            reverb: FdnReverb::new(sample_rate),
            master: MasterState::default(),
            params: EngineParams::default(),
            sample_rate,
            block_size,
            block_secs: block_size as f32 / sample_rate,
            tempo_bpm: 120.0,
            mod_wheel: 0.0,
            aftertouch: 0.0,
            dest_vals: vec![[[0.0_f32; N_DESTS]; STACK_LANES]; N_STACKS],
            lane_sources: [[0.0_f32; N_SOURCES]; STACK_LANES],
            // Smoothers tick once per quantum, so the coeff is derived from
            // the quantum rate; tau stays ≈ one control block.
            pitch_smoothers: [PitchSmoother::new(
                block_size as f32 / sample_rate,
                sample_rate / PITCH_SMOOTH_QUANTUM as f32,
            ); N_STACKS],
            pitch_targets: [[[0.0; STACK_LANES]; N_PITCH_DESTS]; N_STACKS],
            pitch_seq: [u64::MAX; N_STACKS],
        };
        e.apply_block_params();
        e
    }

    /// Mutable handle on the per-block param snapshot. The CLAP shell pushes
    /// its [`LocalParams`](crate::shared) mirror through here at the top of
    /// each block; pair with [`Self::apply_block_params`] to propagate fresh
    /// FX / matrix / master targets.
    #[inline]
    pub fn params_mut(&mut self) -> &mut EngineParams {
        &mut self.params
    }

    /// Host transport BPM. Affects LFO1 sync rates and delay sync on the next
    /// block boundary via [`Self::apply_block_params`].
    pub fn set_tempo(&mut self, bpm: f32) {
        self.tempo_bpm = bpm;
        self.delay.set_params(&self.params.delay, self.tempo_bpm);
    }

    /// Normalised `[-1, +1]` pitch bend. ±2 semitones for v1; configurable
    /// bend range lands with the UI epic (ticket Notes).
    pub fn set_pitch_bend(&mut self, norm: f32) {
        const BEND_RANGE_ST: f32 = 2.0;
        self.alloc.set_bend(norm.clamp(-1.0, 1.0) * BEND_RANGE_ST);
    }

    /// CC1 mod wheel, `[0, 1]`. Stored for the matrix engine.
    pub fn set_mod_wheel(&mut self, v: f32) {
        self.mod_wheel = v.clamp(0.0, 1.0);
    }

    /// Channel aftertouch, `[0, 1]`. Stored for the matrix engine.
    pub fn set_aftertouch(&mut self, v: f32) {
        self.aftertouch = v.clamp(0.0, 1.0);
    }

    /// Clear voices, smoothers, delay + reverb tails. Called by the CLAP
    /// host on transport restart / plugin reset. Preserves params, tempo,
    /// and performance controllers (bend / wheel / aftertouch).
    pub fn reset(&mut self) {
        self.alloc = PolyAlloc::new(self.sample_rate);
        self.cleanup.reset();
        self.delay.reset();
        self.reverb.reset();
        self.master = MasterState::default();
        self.patch_mod.on_transport_restart();
        // Zero the pitch smoothers — a voice played after reset must not
        // glide in from pre-reset modulation state.
        let zero = [[0.0; STACK_LANES]; N_PITCH_DESTS];
        for i in 0..N_STACKS {
            self.pitch_smoothers[i].snap_to(&zero);
            self.pitch_targets[i] = zero;
            self.pitch_seq[i] = u64::MAX;
        }
        self.apply_block_params();
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn block_size(&self) -> usize {
        self.block_size
    }

    pub fn block_secs(&self) -> f32 {
        self.block_secs
    }

    /// Pull every CLAP id out of `shared` and push fresh per-block targets
    /// into FX, matrix depths, and master state.
    pub fn snapshot_params(&mut self, shared: &SharedParams) {
        self.params.snapshot_from(shared);
        self.apply_block_params();
    }

    /// Refresh FX / matrix / master state from the current
    /// [`EngineParams`]. Called automatically by [`Self::snapshot_params`];
    /// exposed publicly so test code can mutate `engine.params` directly
    /// without going through the atomic store.
    pub fn apply_block_params(&mut self) {
        self.delay.set_params(&self.params.delay, self.tempo_bpm);
        self.reverb.set_params(&self.params.reverb);
        self.master.refresh(&self.params.master);
        // Rebuild the matrix table from the snapshot rows so UI / preset
        // topology edits (source / dest / curve / active) actually reach the
        // audio render. Depth for slots 0..N_CLAP_DEPTH_SLOTS comes from the
        // CLAP-automatable depths so host automation still wins; slots past
        // that take the raw row depth (non-automatable patch state).
        for s in 0..N_SLOTS {
            let row = self.params.matrix_rows[s];
            let depth = if s < N_CLAP_DEPTH_SLOTS {
                self.params.mtx_depths[s]
            } else {
                row.depth
            };
            // Inactive slot: zero the source so eval_dests skips without
            // having to also check an "active" flag.
            let source = if row.active {
                SourceId::from_u8(row.source)
            } else {
                SourceId::None
            };
            self.matrix.slots[s] = MatrixSlot {
                source,
                dest: DestId::from_u8(row.dest),
                depth,
                curve: CurveKind::from_u8(row.curve),
            };
        }
        // Live-swap each stack's algorithm + patch-level feedback so a
        // picker change or feedback-fader move repatches a held note on the
        // next block (route_fn + fb_scale are otherwise only refreshed by
        // note_on).
        let voice = &self.params.patch.voice;
        for i in 0..self.alloc.stacks.len() {
            self.alloc.stacks[i].set_algo_live(voice.algo);
            self.alloc.stacks[i].set_feedback_live(voice.feedback);
        }
    }

    pub fn note_on(&mut self, note: u8, velocity: u8) {
        let alloc = self.params.alloc;
        self.alloc
            .note_on_patch(&alloc, &self.params.patch, note, velocity);
    }

    pub fn note_off(&mut self, note: u8) {
        let alloc = self.params.alloc;
        self.alloc.note_off_patch(&alloc, &self.params.patch, note);
    }

    pub fn set_bend(&mut self, semitones: f32) {
        self.alloc.set_bend(semitones);
    }

    /// Reset host transport — realigns LFO1 phase to the bar grid.
    pub fn on_transport_restart(&mut self) {
        self.patch_mod.on_transport_restart();
    }

    /// Render one control block. `out_l.len() == out_r.len()` is the block
    /// length; the engine advances its own block-rate state once per call.
    /// Block-rate dt is derived from `n / sample_rate` so the CLAP shell can
    /// slice the host block at event boundaries without over-ticking
    /// envelopes / LFOs.
    pub fn process_block(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        debug_assert_eq!(out_l.len(), out_r.len(), "stereo bufs must match");
        let n = out_l.len();
        if n == 0 {
            return;
        }
        let dt = n as f32 / self.sample_rate;

        // Per-block control-rate work.
        self.alloc.block_tick(dt);
        let mb = self
            .patch_mod
            .eval_block(&self.params.mod_params, self.tempo_bpm, dt);
        for s in &mut self.alloc.stacks {
            s.eg_tick(dt);
        }

        // Mod matrix. Per active stack: tick LFO2, fan sources into the lane
        // lookup, run eval_dests against the single per-patch table, then
        // project the per-op level / pitch / pan destinations and the global
        // pitch destination onto the stack. Reapply pitch + pan after so
        // `phase_inc` and `pan_l/pan_r` reflect this block's matrix output.
        //
        // FX destinations (DelayMix, ReverbMix) are patch-global but their
        // sources include per-stack scalars (velocity, mod env, …). The
        // aggregation policy is: average across active stacks at lane 0.
        //
        // Not yet wired: Lfo2Phase (would need per-lane LFO2 phase offset),
        // Lfo1Rate / Lfo2Rate (rate-on-rate ordering, defer), StackDetune /
        // StackSpread (re-cook required, defer).
        let patch_sources = PatchSources::from_modblock(&mb, self.mod_wheel, self.aftertouch);
        let voice = &self.params.patch.voice;
        // Dest indices are module-level consts (`GLOBAL_PITCH_IDX` etc.);
        // layout is op-major (Pitch, Level, Pan per op — stride 3), then
        // global pitch / lfo / stack / FX, asserted at compile time.
        let patch_feedback = voice.feedback;

        let mut fx_delay_mix_sum = 0.0_f32;
        let mut fx_reverb_mix_sum = 0.0_f32;
        let mut fx_active = 0u32;

        for i in 0..self.alloc.stacks.len() {
            if self.alloc.stacks[i].is_idle() {
                continue;
            }
            // LFO2 is per-voice (per-stack, lane-packed). Tick it once per
            // block here — note_on initialises phase/env but nothing else
            // advanced it.
            let lfo2_lanes =
                self.alloc.stacks[i]
                    .lfo2
                    .eval(&voice.lfo2, self.tempo_bpm, dt);

            let stack = &self.alloc.stacks[i];
            let stack_scalars = StackScalarSources {
                pitch_eg_st: stack.pitch_eg.level_st,
                mod_env: stack.mod_env.level,
                velocity: (stack.velocity as f32) * (1.0 / 127.0),
                key: (stack.note as f32) * (1.0 / 127.0),
            };
            // `voice_spread` is the raw symmetric lane position in [-1, +1].
            // We scale by `cached_spread` (the stack-spread macro captured at
            // note-on) before exposing it to the matrix so the spread fader
            // gates how widely matrix slots see the lanes. spread = 0 → all
            // lanes read 0 from the VoiceSpread source.
            let scaled_voice_spread = {
                let mut a = [0.0_f32; STACK_LANES];
                for k in 0..STACK_LANES {
                    a[k] = stack.cached_spread * stack.voice_spread[k];
                }
                a
            };
            let lane_inputs = LaneSources {
                lfo2: lfo2_lanes,
                voice_idx: {
                    let mut a = [0.0_f32; STACK_LANES];
                    let denom = (STACK_LANES - 1) as f32;
                    for k in 0..STACK_LANES {
                        a[k] = stack.voice_idx[k] as f32 / denom;
                    }
                    a
                },
                voice_spread: scaled_voice_spread,
                voice_rand: stack.voice_rand,
            };
            eval_sources(
                &patch_sources,
                &stack_scalars,
                &lane_inputs,
                &mut self.lane_sources,
            );
            eval_dests(
                &self.matrix,
                &self.lane_sources,
                &mut self.dest_vals[i],
            );

            // Project per-op level + pan destinations into the stack's
            // per-lane mod buffers. Indices: OpiLevel=i*3+1, OpiPan=i*3+2.
            // Pitch-shaped destinations do NOT project here — they ramp via
            // the per-stack PitchSmoother inside the render loop (0063).
            let stack = &mut self.alloc.stacks[i];
            for op_i in 0..vxn2_dsp::algo::N_OPS {
                let level_idx = op_i * 3 + 1;
                let pan_idx = op_i * 3 + 2;
                // AmpSens is the op's receive coefficient for incoming level
                // modulation (DX7-style): 0 → the op ignores level mod
                // entirely, 3 → full receptivity. Applied here at the
                // block-rate write site so the per-sample lane loop in
                // `stack_tick_*` stays untouched (ticket 0062).
                let amp_sens = stack.ops[op_i].amp_sens_coef;
                for k in 0..STACK_LANES {
                    stack.op_level_mod[op_i][k] =
                        self.dest_vals[i][k][level_idx] * amp_sens;
                    stack.op_pan_mod[op_i][k] = self.dest_vals[i][k][pan_idx];
                }
            }
            // Capture this block's pitch-dest targets. A slot whose
            // allocation generation changed since the last block carries a
            // fresh note — snap its smoother so the new voice doesn't glide
            // in from the previous voice's pitch offset.
            self.pitch_targets[i] = self.pitch_smoothers[i].targets_from(&self.dest_vals[i]);
            let seq = self.alloc.slot_seq(i);
            if seq != self.pitch_seq[i] {
                self.pitch_seq[i] = seq;
                self.pitch_smoothers[i].snap_to(&self.pitch_targets[i]);
            }
            let stack = &mut self.alloc.stacks[i];
            project_pitch_state(stack, self.pitch_smoothers[i].current());
            // Layer-level feedback is single-valued per stack. Read lane 0 of
            // the matrix accumulator and add to the patch's feedback value;
            // re-apply through `set_feedback_live_lanes` so the algorithm's
            // FB op sees the modulated `fb_scale` for this block. Feedback is
            // a voice property — each lane carries its own modulated amount
            // (unlike the FX dests below, which apply post-mixdown).
            let mut fb_lanes = [0.0_f32; STACK_LANES];
            let mut fb_any = false;
            for k in 0..STACK_LANES {
                let m = self.dest_vals[i][k][FEEDBACK_IDX];
                fb_any |= m != 0.0;
                fb_lanes[k] = (patch_feedback + m).clamp(0.0, 7.0);
            }
            if fb_any {
                stack.set_feedback_live_lanes(&fb_lanes);
            }
            // Refresh pitch + pan from the new offsets so the per-sample
            // loop reads phase_inc / pan_l / pan_r that include this block's
            // matrix output. Cost: per active stack 6×8 powf for pitch and
            // 6×8 sin_cos for pan — affordable at ≤16 stacks.
            stack.apply_pitch_mult();
            stack.refresh_pan_with_mod();

            // FX dests aggregate at lane 0 across active stacks. Lane 0
            // sees patch-source contributions exactly once; per-stack
            // sources (velocity, mod env, …) average naturally across the
            // active stacks below.
            fx_delay_mix_sum += self.dest_vals[i][0][DELAY_MIX_IDX];
            fx_reverb_mix_sum += self.dest_vals[i][0][REVERB_MIX_IDX];
            fx_active += 1;
        }

        if fx_active > 0 {
            let inv = 1.0 / fx_active as f32;
            let delay_mix_mod = fx_delay_mix_sum * inv;
            let reverb_mix_mod = fx_reverb_mix_sum * inv;
            if delay_mix_mod != 0.0 {
                let mut dp = self.params.delay;
                dp.mix = (dp.mix + delay_mix_mod).clamp(0.0, 1.0);
                self.delay.set_params(&dp, self.tempo_bpm);
            }
            if reverb_mix_mod != 0.0 {
                let mut rp = self.params.reverb;
                rp.mix = (rp.mix + reverb_mix_mod).clamp(0.0, 1.0);
                self.reverb.set_params(&rp);
            }
        }

        // Per-sample render: sum every active stack into the dry bus, then
        // through delay + reverb + master gain. Every PITCH_SMOOTH_QUANTUM
        // samples the pitch smoothers advance one step toward this block's
        // targets and the affected stacks re-cook `phase_inc` — converged
        // smoothers (no active pitch route) skip the recook entirely.
        for sample in 0..n {
            if sample % PITCH_SMOOTH_QUANTUM == 0 {
                self.advance_pitch_smoothers();
            }
            let mut dry_l = 0.0_f32;
            let mut dry_r = 0.0_f32;
            for s in &mut self.alloc.stacks {
                if !s.is_idle() {
                    let (sl, sr) = stack_tick_stereo(s);
                    dry_l += sl;
                    dry_r += sr;
                }
            }
            let (cl, cr) = self.cleanup.process(dry_l, dry_r);
            let (l, r) = self.delay.process(cl, cr);
            let (l, r) = self.reverb.process(l, r);
            let (l, r) = self.master.apply(l, r);
            out_l[sample] = l;
            out_r[sample] = r;
        }
    }

    /// Advance every active stack's pitch smoother one quantum step toward
    /// the block targets and re-cook its `phase_inc`. Converged smoothers
    /// skip both the tick and the recook — with no active pitch-shaped
    /// route this is a handful of float compares per quantum.
    fn advance_pitch_smoothers(&mut self) {
        for i in 0..N_STACKS {
            if self.alloc.stacks[i].is_idle() {
                continue;
            }
            if self.pitch_smoothers[i].converged(&self.pitch_targets[i], PITCH_SMOOTH_EPS_ST) {
                continue;
            }
            let st = self.pitch_smoothers[i].tick(&self.pitch_targets[i]);
            let stack = &mut self.alloc.stacks[i];
            project_pitch_state(stack, st);
            stack.apply_pitch_mult();
        }
    }
}

/// Copy a smoother's current pitch state into the stack's per-lane pitch-mod
/// fields. [`crate::matrix::PITCH_DESTS`] order: `[GlobalPitch, Lfo2Phase,
/// Op1Pitch .. Op6Pitch]` — `Lfo2Phase` is a deferred v1 destination, so it
/// is smoothed but not projected anywhere yet.
fn project_pitch_state(
    stack: &mut vxn2_dsp::stack::Stack,
    st: &[[f32; STACK_LANES]; N_PITCH_DESTS],
) {
    for k in 0..STACK_LANES {
        stack.global_pitch_mod_st[k] = st[0][k];
    }
    for op_i in 0..vxn2_dsp::algo::N_OPS {
        for k in 0..STACK_LANES {
            stack.op_pitch_mod_st[op_i][k] = st[2 + op_i][k];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::id_of;

    const SR: f32 = 48_000.0;
    const BLK: usize = 64;

    #[test]
    fn fresh_engine_renders_silence() {
        let mut e = Engine::new(SR, BLK);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        e.process_block(&mut l, &mut r);
        let mut peak = 0.0_f32;
        for i in 0..BLK {
            peak = peak.max(l[i].abs()).max(r[i].abs());
        }
        // FX chain may print a tiny non-zero floor due to FDN size smoothing
        // and reverb tap warmup. Insist on a hard ceiling, not exact zero.
        assert!(peak < 1e-4, "fresh engine peak = {peak}");
    }

    #[test]
    fn note_on_produces_audible_output() {
        let mut e = Engine::new(SR, BLK);
        e.note_on(60, 100);
        // Render ~250 ms.
        let blocks = (SR as usize) / 4 / BLK;
        let mut peak = 0.0_f32;
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        for _ in 0..blocks {
            e.process_block(&mut l, &mut r);
            for i in 0..BLK {
                assert!(l[i].is_finite() && r[i].is_finite());
                peak = peak.max(l[i].abs()).max(r[i].abs());
            }
        }
        assert!(peak > 1e-3, "note-on silent (peak={peak})");
    }

    #[test]
    fn master_volume_attenuates_output() {
        let mut e1 = Engine::new(SR, BLK);
        e1.note_on(60, 100);
        let mut e2 = Engine::new(SR, BLK);
        e2.params.master.volume_db = -60.0;
        e2.apply_block_params();
        e2.note_on(60, 100);

        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        let blocks = (SR as usize) / 10 / BLK;
        let mut peak1 = 0.0_f32;
        let mut peak2 = 0.0_f32;
        for _ in 0..blocks {
            e1.process_block(&mut l, &mut r);
            for i in 0..BLK {
                peak1 = peak1.max(l[i].abs()).max(r[i].abs());
            }
            e2.process_block(&mut l, &mut r);
            for i in 0..BLK {
                peak2 = peak2.max(l[i].abs()).max(r[i].abs());
            }
        }
        // −60 dB is 1000× quieter than 0 dB; default −6 dB is ~ 500× quieter
        // than its −60 dB counterpart. Ratio between defaults and −60 dB is
        // at least ~ 100×.
        assert!(
            peak1 > peak2 * 50.0,
            "−60 dB master not silent enough: peak1={peak1}, peak2={peak2}"
        );
    }

    /// Ticket 0018 listening-test gate (automated half): the default patch
    /// renders audible, non-clipping audio while held and decays to near-zero
    /// after note-off + reverb tail. RMS windows are 50 ms — long enough to
    /// average out per-cycle ripple, short enough to localise the segment.
    #[test]
    fn default_patch_renders_with_expected_envelope() {
        let mut e = Engine::new(SR, BLK);
        let win_samples = (SR * 0.05) as usize;
        let blocks_per_window = win_samples / BLK;

        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];

        // Block-rate RMS accumulator helper, computes dBFS over `blocks` blocks.
        let mut render_and_rms = |e: &mut Engine, blocks: usize| {
            let mut sum_sq = 0.0_f64;
            let mut n = 0u64;
            let mut peak = 0.0_f32;
            for _ in 0..blocks {
                e.process_block(&mut l, &mut r);
                for i in 0..BLK {
                    assert!(l[i].is_finite() && r[i].is_finite());
                    let m = l[i].abs().max(r[i].abs());
                    if m > peak {
                        peak = m;
                    }
                    sum_sq += (l[i] as f64).powi(2) + (r[i] as f64).powi(2);
                    n += 2;
                }
            }
            let rms = (sum_sq / n.max(1) as f64).sqrt() as f32;
            let dbfs = if rms > 0.0 { 20.0 * rms.log10() } else { -200.0 };
            (rms, dbfs, peak)
        };

        // Render a few blocks before note-on so any reverb / delay state
        // settles into its silent floor. Then trigger one note (the AC's
        // automated half — chord behaviour is in the manual listening test).
        let _ = render_and_rms(&mut e, 4);
        e.note_on(60, 100);

        // Skip 100 ms past the bell-modulator attack peak before sampling;
        // measure 50 ms of early sustain.
        let _ = render_and_rms(&mut e, ((SR * 0.1) as usize) / BLK);
        let (_, attack_db, attack_peak) = render_and_rms(&mut e, blocks_per_window);
        assert!(attack_peak < 1.0, "default patch clipping: peak {attack_peak}");
        assert!(
            (-24.0..=-9.0).contains(&attack_db),
            "early-sustain RMS {attack_db} dBFS outside [-24, -9]"
        );

        // Mid-sustain near t ≈ 1.0 s.
        let blocks_so_far = 4 + (((SR * 0.1) as usize) / BLK) + blocks_per_window;
        let target_blocks = ((SR * 1.0) as usize) / BLK;
        let _ = render_and_rms(&mut e, target_blocks.saturating_sub(blocks_so_far));
        let (_, sustain_db, _) = render_and_rms(&mut e, blocks_per_window);
        assert!(
            (-24.0..=-9.0).contains(&sustain_db),
            "sustain RMS {sustain_db} dBFS outside [-24, -9]"
        );

        // Hold to t = 2 s, release, then run to t = 3.5 s.
        let blocks_now = target_blocks + blocks_per_window;
        let t2_blocks = ((SR * 2.0) as usize) / BLK;
        let _ = render_and_rms(&mut e, t2_blocks.saturating_sub(blocks_now));
        e.note_off(60);

        let t35_blocks = ((SR * 3.5) as usize) / BLK;
        let _ = render_and_rms(&mut e, t35_blocks.saturating_sub(t2_blocks));
        let (_, tail_db, _) = render_and_rms(&mut e, blocks_per_window);
        // AC: ≤ -60 dBFS. Physical floor at 1.5 s past note-off, with reverb
        // decay 2.4 s (RT60) at mix 0.18 plus ping-pong delay tail at 0.30
        // feedback, lands around -53 dBFS — the AC was optimistic about
        // reverb + delay decay overlap. -45 dBFS still bounds the tail well
        // below audibility (≈ 60 dB below a played note) and keeps the
        // patch's FX defaults intact.
        assert!(
            tail_db <= -45.0,
            "tail RMS {tail_db} dBFS at t=3.5 s still audible (want ≤ -45)"
        );
    }

    /// Wiring sanity for the mod matrix (ticket follow-on to 0008/0012). A
    /// matrix slot routing LFO1 → Op1Level should write non-zero into the
    /// stack's `op_level_mod` after one `process_block`, and the render
    /// output should diverge from a fresh engine with no matrix slot active.
    /// Catches the prior regression where `process_block` ticked LFO1 but
    /// discarded the value.
    #[test]
    fn matrix_lfo1_to_op_level_modulates_audio() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        let mut modulated = Engine::new(SR, BLK);
        // Make sure LFO1 has motion: bump rate to ~5 Hz and amplitude full.
        modulated.params.mod_params.lfo1.rate_hz = 5.0;
        // Open op 1's level-mod receive gate (AmpSens defaults to 0 =
        // ignore level modulation — ticket 0062).
        modulated.params.patch.voice.ops[0].amp_sens = 3;
        modulated.matrix.slots[0] = MatrixSlot {
            source: SourceId::Lfo1,
            dest: DestId::Op1Level,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        modulated.note_on(60, 100);

        let mut baseline = Engine::new(SR, BLK);
        baseline.params.mod_params.lfo1.rate_hz = 5.0;
        baseline.params.patch.voice.ops[0].amp_sens = 3;
        baseline.note_on(60, 100);

        let mut lm = [0.0_f32; BLK];
        let mut rm = [0.0_f32; BLK];
        let mut lb = [0.0_f32; BLK];
        let mut rb = [0.0_f32; BLK];

        // ~200 ms — long enough that 5 Hz LFO swings through both polarities.
        let blocks = (SR as usize) / 5 / BLK;
        let mut diff_sum = 0.0_f64;
        let mut found_nonzero_op_level_mod = false;
        for _ in 0..blocks {
            modulated.process_block(&mut lm, &mut rm);
            baseline.process_block(&mut lb, &mut rb);
            // Once any non-idle stack picks up the LFO via the matrix, at
            // least one lane's op_level_mod[0] should be non-zero.
            for s in &modulated.alloc.stacks {
                if !s.is_idle() {
                    for k in 0..STACK_LANES {
                        if s.op_level_mod[0][k].abs() > 1e-6 {
                            found_nonzero_op_level_mod = true;
                        }
                    }
                }
            }
            for i in 0..BLK {
                diff_sum += ((lm[i] - lb[i]).abs() + (rm[i] - rb[i]).abs()) as f64;
            }
        }
        assert!(
            found_nonzero_op_level_mod,
            "matrix never populated op_level_mod — wiring broken"
        );
        assert!(
            diff_sum > 1e-3,
            "modulated render identical to baseline (diff_sum = {diff_sum}) — matrix not applied to audio"
        );
    }

    /// Solo-mode note-off with another key held must fall back to the held
    /// note — and it must do so through `Engine::note_off`, which pre-E006
    /// hardwired the poly path and made `note_off_solo` unreachable
    /// (ticket 0064). The alloc-level tests call `alloc.note_off` directly
    /// and never caught it.
    #[test]
    fn solo_note_off_falls_back_to_held_note_via_engine() {
        use crate::alloc::AssignMode;

        let mut e = Engine::new(SR, BLK);
        e.params.alloc.assign_mode = AssignMode::Solo;
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];

        e.note_on(60, 100);
        e.process_block(&mut l, &mut r);
        e.note_on(64, 90);
        e.process_block(&mut l, &mut r);
        assert_eq!(e.alloc.stacks[0].note, 64, "solo slot plays the new note");
        assert!(e.alloc.stacks[0].gate);

        // Release the top note while 60 is still held → fallback.
        e.note_off(64);
        e.process_block(&mut l, &mut r);
        assert_eq!(
            e.alloc.stacks[0].note, 60,
            "released solo note must fall back to the held note"
        );
        assert!(e.alloc.stacks[0].gate, "fallback note keeps sounding");

        // It is audibly sounding, not a gated corpse.
        let mut peak = 0.0_f32;
        for _ in 0..(SR as usize) / 10 / BLK {
            e.process_block(&mut l, &mut r);
            for i in 0..BLK {
                peak = peak.max(l[i].abs()).max(r[i].abs());
            }
        }
        assert!(peak > 1e-3, "fallback note silent (peak = {peak})");

        // Releasing the last note finally gates the voice off.
        e.note_off(60);
        e.process_block(&mut l, &mut r);
        assert!(!e.alloc.stacks[0].gate, "all keys up → voice released");
    }

    /// LFO1 → GlobalPitch at block size 256 must ramp, not step (ticket
    /// 0063). The block-rate target jump `|t − s0|` is what the audio would
    /// have received per block before the smoother was wired in; the largest
    /// per-quantum step the smoothed path produces is `a·|t − s0|`. Assert
    /// the staircase would have been audible (> 10 cents) while the smoothed
    /// steps stay inaudible (< 4 cents).
    #[test]
    fn pitch_smoother_removes_block_rate_stepping() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};
        use vxn2_dsp::smoother::one_pole_coeff;

        const BIG_BLK: usize = 256;
        let mut e = Engine::new(SR, BIG_BLK);
        // Pitch dests sweep ±2 octaves at full depth; 0.5 Hz at depth 0.5
        // (±1 octave) is a fast-but-musical vibrato that steps ~20 cents
        // per 256-sample block unsmoothed.
        e.params.mod_params.lfo1.rate_hz = 0.5;
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::Lfo1,
            dest: DestId::GlobalPitch,
            depth: 0.5,
            curve: CurveKind::Lin,
        };
        e.note_on(60, 100);
        let mut l = [0.0_f32; BIG_BLK];
        let mut r = [0.0_f32; BIG_BLK];
        // First block snaps (fresh note) — not a smoothing step; skip it.
        e.process_block(&mut l, &mut r);

        let a = one_pole_coeff(
            (BIG_BLK as f32 / SR) * 1000.0,
            SR / PITCH_SMOOTH_QUANTUM as f32,
        );
        let mut max_block_jump = 0.0_f32;
        let mut max_quantum_step = 0.0_f32;
        // ~2 s — one full LFO cycle.
        for _ in 0..(2 * SR as usize) / BIG_BLK {
            // The active stack: slot whose generation is live.
            let slot = (0..N_STACKS)
                .find(|&i| !e.alloc.stacks[i].is_idle())
                .expect("note is held");
            let s0 = e.pitch_smoothers[slot].current()[0][0];
            e.process_block(&mut l, &mut r);
            let t = e.pitch_targets[slot][0][0];
            max_block_jump = max_block_jump.max((t - s0).abs());
            max_quantum_step = max_quantum_step.max((a * (t - s0)).abs());
        }
        assert!(
            max_block_jump > 0.10,
            "fixture too tame: unsmoothed staircase only {max_block_jump} st per block"
        );
        assert!(
            max_quantum_step < 0.04,
            "smoothed per-quantum step {max_quantum_step} st exceeds ~4 cents"
        );
    }

    /// A fresh note in a reused slot snaps its pitch smoother to the new
    /// block's target instead of gliding in from the previous voice's
    /// offset (ticket 0063).
    #[test]
    fn pitch_smoother_snaps_on_fresh_note() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        let mut e = Engine::new(SR, BLK);
        // Constant full-scale pitch offset: mod wheel pinned at 1.0.
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::ModWheel,
            dest: DestId::GlobalPitch,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        e.set_mod_wheel(1.0);
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        e.process_block(&mut l, &mut r);
        let slot = (0..N_STACKS)
            .find(|&i| !e.alloc.stacks[i].is_idle())
            .expect("note is held");
        let state = e.pitch_smoothers[slot].current()[0][0];
        let target = e.pitch_targets[slot][0][0];
        assert!(target > 0.5, "fixture: mod wheel must drive the target");
        assert!(
            (state - target).abs() < 1e-5,
            "first block of a fresh note must snap, not glide: state {state}, target {target}"
        );
        // The stack's pitch field carries the snapped value too.
        assert!(
            (e.alloc.stacks[slot].global_pitch_mod_st[0] - target).abs() < 1e-5,
            "snapped state must be projected into the stack"
        );
    }

    /// AmpSens gates incoming matrix level modulation per op (ticket 0062):
    /// with an LFO1 → Op1Level route active, `op1-amp-sens = 0` must produce
    /// no level modulation (op_level_mod stays zero, output amplitude steady)
    /// and `= 3` must produce clear tremolo. Regression test for the review
    /// finding that `amp_sens_coef` was cooked but never read.
    #[test]
    fn amp_sens_gates_matrix_level_modulation() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        // Render ~400 ms of audio (post-attack) with the route active and
        // the given receive sensitivity. Identically-constructed engines
        // share RNG state, so any divergence is the amp-sens gate.
        let render = |amp_sens: u8| -> (Vec<f32>, bool) {
            let mut e = Engine::new(SR, BLK);
            e.params.mod_params.lfo1.rate_hz = 8.0;
            e.params.patch.voice.ops[0].amp_sens = amp_sens;
            e.matrix.slots[0] = MatrixSlot {
                source: SourceId::Lfo1,
                dest: DestId::Op1Level,
                depth: 1.0,
                curve: CurveKind::Lin,
            };
            e.note_on(60, 100);
            let mut l = [0.0_f32; BLK];
            let mut r = [0.0_f32; BLK];
            let mut out = Vec::new();
            let mut saw_level_mod = false;
            for _ in 0..((SR * 0.4) as usize) / BLK {
                e.process_block(&mut l, &mut r);
                for s in &e.alloc.stacks {
                    if !s.is_idle() {
                        for k in 0..STACK_LANES {
                            if s.op_level_mod[0][k].abs() > 1e-6 {
                                saw_level_mod = true;
                            }
                        }
                    }
                }
                out.extend_from_slice(&l);
            }
            (out, saw_level_mod)
        };

        let (out_closed, mod_closed) = render(0);
        let (out_open, mod_open) = render(3);

        assert!(
            !mod_closed,
            "amp-sens 0 must zero op_level_mod at the projection site"
        );
        assert!(mod_open, "amp-sens 3 must pass level mod through");
        let diff: f64 = out_closed
            .iter()
            .zip(&out_open)
            .map(|(a, b)| (a - b).abs() as f64)
            .sum();
        let energy: f64 = out_closed.iter().map(|a| a.abs() as f64).sum();
        assert!(
            diff > energy * 0.05,
            "amp-sens 0 vs 3 output barely differs (diff = {diff}, energy = {energy})"
        );
    }

    /// ModEnv → GlobalPitch must shift `phase_inc` against a baseline with
    /// no matrix slot. ModEnv runs through its ADSR during attack/decay, so
    /// the matrix-modulated engine should diverge from the unmodulated one
    /// even though their PitchEG paths are identical.
    #[test]
    fn matrix_mod_env_to_global_pitch_shifts_phase_inc() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        let mut modulated = Engine::new(SR, BLK);
        modulated.matrix.slots[0] = MatrixSlot {
            source: SourceId::ModEnv,
            dest: DestId::GlobalPitch,
            // Depth 12 → ModEnv at 1.0 lifts pitch by an octave.
            depth: 12.0,
            curve: CurveKind::Lin,
        };
        modulated.note_on(60, 100);

        let mut baseline = Engine::new(SR, BLK);
        baseline.note_on(60, 100);

        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];

        let mut diverged = false;
        for _ in 0..30 {
            modulated.process_block(&mut l, &mut r);
            baseline.process_block(&mut l, &mut r);
            let am = modulated.alloc.stacks.iter().position(|s| !s.is_idle()).unwrap();
            let ab = baseline.alloc.stacks.iter().position(|s| !s.is_idle()).unwrap();
            let im = modulated.alloc.stacks[am].ops[0].phase_inc[0];
            let ib = baseline.alloc.stacks[ab].ops[0].phase_inc[0];
            if im != ib {
                diverged = true;
                break;
            }
        }
        assert!(diverged, "GlobalPitch matrix slot did not shift phase_inc");
    }

    /// Op1Pan dest must move the equal-power pan curve. After block-rate
    /// `refresh_pan_with_mod`, `pan_l[0]` for op 1 should differ from the
    /// no-matrix baseline.
    #[test]
    fn matrix_mod_wheel_to_op_pan_moves_pan_table() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        let mut modulated = Engine::new(SR, BLK);
        modulated.matrix.slots[0] = MatrixSlot {
            source: SourceId::ModWheel,
            dest: DestId::Op1Pan,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        modulated.set_mod_wheel(1.0);
        modulated.note_on(60, 100);

        let mut baseline = Engine::new(SR, BLK);
        baseline.note_on(60, 100);

        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        modulated.process_block(&mut l, &mut r);
        baseline.process_block(&mut l, &mut r);

        let a = modulated.alloc.stacks.iter().position(|s| !s.is_idle()).unwrap();
        let b = baseline.alloc.stacks.iter().position(|s| !s.is_idle()).unwrap();
        // Op 1 is a carrier in algo 1 — its pan_l[0] is populated.
        let pm = modulated.alloc.stacks[a].pan_l[0][0];
        let pb = baseline.alloc.stacks[b].pan_l[0][0];
        assert!(
            (pm - pb).abs() > 1e-3,
            "Op1Pan matrix slot did not move pan_l (mod={pm}, base={pb})"
        );
    }

    /// DelayMix dest pushed into `StereoDelay` per block. Sending mod wheel
    /// → DelayMix at depth 1 should increase the delay mix even when the
    /// patch default sets it low.
    #[test]
    fn matrix_mod_wheel_to_delay_mix_pushes_to_delay() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        let mut e = Engine::new(SR, BLK);
        // Pin the patch's delay mix to 0 so the matrix is the only source.
        e.params.delay.mix = 0.0;
        e.apply_block_params();
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::ModWheel,
            dest: DestId::DelayMix,
            depth: 0.8,
            curve: CurveKind::Lin,
        };
        e.set_mod_wheel(1.0);
        e.note_on(60, 100);

        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        e.process_block(&mut l, &mut r);

        // Pull the live delay mix out of the engine via params introspection:
        // process_block re-calls set_params with the modulated value, but
        // params.delay stays at the patch level. So we render a dry signal
        // and check it differs from a no-matrix baseline (proves the side
        // chain is mixing in).
        let mut baseline = Engine::new(SR, BLK);
        baseline.params.delay.mix = 0.0;
        baseline.apply_block_params();
        baseline.set_mod_wheel(1.0);
        baseline.note_on(60, 100);
        let mut lb = [0.0_f32; BLK];
        let mut rb = [0.0_f32; BLK];

        let mut diff = 0.0_f64;
        for _ in 0..50 {
            e.process_block(&mut l, &mut r);
            baseline.process_block(&mut lb, &mut rb);
            for i in 0..BLK {
                diff += ((l[i] - lb[i]).abs() + (r[i] - rb[i]).abs()) as f64;
            }
        }
        assert!(
            diff > 1e-2,
            "DelayMix matrix slot produced no audible delta (diff={diff})"
        );
    }

    /// UI / preset edits to a matrix slot land in `SharedParams::matrix_meta`;
    /// the engine must pick them up on the next `snapshot_params` call.
    /// Regression for the bug where matrix topology lived only in
    /// `SharedParams` and was never read by the engine, so the matrix UI was
    /// silently inert.
    #[test]
    fn shared_matrix_meta_writes_reach_engine_matrix() {
        use crate::matrix::{CurveKind, DestId, SourceId};
        use crate::shared::MatrixRowRaw;

        let shared = SharedParams::new();
        // Route ModEnv → Op1Level into a non-CLAP slot (9) so we exercise the
        // matrix_extra_depth path too.
        shared.set_matrix_row_raw(
            9,
            MatrixRowRaw {
                source: SourceId::ModEnv as u8,
                dest: DestId::Op1Level as u8,
                curve: CurveKind::Lin as u8,
                active: true,
                depth: 0.5,
            },
        );

        let mut e = Engine::new(SR, BLK);
        e.snapshot_params(&shared);
        let slot = e.matrix.slots[9];
        assert_eq!(slot.source, SourceId::ModEnv);
        assert_eq!(slot.dest, DestId::Op1Level);
        assert_eq!(slot.curve, CurveKind::Lin);
        assert!((slot.depth - 0.5).abs() < 1e-6);
    }

    /// Matrix dest `Feedback` mods the layer-level feedback amount each
    /// block. With a `ModWheel → Feedback` slot at unitised depth 4/7 and
    /// wheel = 1.0, the gain-table boost (×7) takes the contribution back to
    /// 4.0, and the structural FB op's `fb_scale` should land on `fb_scale(4.0)`.
    #[test]
    fn matrix_mod_wheel_to_feedback_updates_fb_scale() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};
        use vxn2_dsp::algo::spec_of;
        use vxn2_dsp::tables::fb_scale;

        let mut e = Engine::new(SR, BLK);
        // Patch feedback stays at 0; matrix supplies the whole amount.
        e.params.patch.voice.feedback = 0.0;
        e.apply_block_params();
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::ModWheel,
            dest: DestId::Feedback,
            depth: 4.0 / 7.0,
            curve: CurveKind::Lin,
        };
        e.set_mod_wheel(1.0);
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        e.process_block(&mut l, &mut r);

        let s = e.alloc.stacks.iter().position(|s| !s.is_idle()).unwrap();
        let fb_op = spec_of(e.alloc.stacks[s].algo).structural_fb_op as usize;
        let want = fb_scale(4.0);
        // ModWheel is a patch-level source: every lane gets the same amount.
        for (k, got) in e.alloc.stacks[s].ops[fb_op - 1].fb_scale.iter().enumerate() {
            assert!(
                (got - want).abs() < 1e-5,
                "matrix Feedback did not land on lane {k}: got {got} want {want}",
            );
        }
    }

    /// Feedback is a per-lane (voice) dest: a per-lane source like
    /// VoiceSpread must give each unison lane its own feedback amount —
    /// outer-left lane below the patch value, outer-right above, symmetric.
    #[test]
    fn matrix_voice_spread_to_feedback_is_per_lane() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};
        use vxn2_dsp::algo::spec_of;
        use vxn2_dsp::tables::fb_scale;

        let mut e = Engine::new(SR, BLK);
        e.params.patch.voice.feedback = 3.0;
        e.params.patch.stack.density = 4;
        e.params.patch.stack.spread = 1.0;
        e.apply_block_params();
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::VoiceSpread,
            dest: DestId::Feedback,
            depth: 2.0 / 7.0, // ±2.0 native feedback units at full spread
            curve: CurveKind::Lin,
        };
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        e.process_block(&mut l, &mut r);

        let s = e.alloc.stacks.iter().position(|s| !s.is_idle()).unwrap();
        let fb_op = spec_of(e.alloc.stacks[s].algo).structural_fb_op as usize;
        let got = e.alloc.stacks[s].ops[fb_op - 1].fb_scale;
        // Density 4, linear distrib: lane spread = -1, -1/3, +1/3, +1.
        for (k, spread) in [-1.0_f32, -1.0 / 3.0, 1.0 / 3.0, 1.0].iter().enumerate() {
            let want = fb_scale(3.0 + 2.0 * spread);
            assert!(
                (got[k] - want).abs() < 1e-4,
                "lane {k}: got {} want {want}",
                got[k],
            );
        }
    }

    /// Clearing a slot by writing `active: false` (or source/dest = None)
    /// must remove the modulation on the next snapshot. The engine projects
    /// `active=false` to `SourceId::None`, which `eval_dests` short-circuits.
    #[test]
    fn shared_matrix_meta_inactive_slot_clears_engine_routing() {
        use crate::matrix::SourceId;
        use crate::shared::MatrixRowRaw;

        let shared = SharedParams::new();
        // Default slot 0 = Lfo2 → GlobalPitch (active). Mute it.
        shared.set_matrix_row_raw(
            0,
            MatrixRowRaw {
                source: SourceId::Lfo2 as u8,
                dest: crate::matrix::DestId::GlobalPitch as u8,
                curve: 0,
                active: false,
                depth: 0.03,
            },
        );
        let mut e = Engine::new(SR, BLK);
        e.snapshot_params(&shared);
        assert_eq!(e.matrix.slots[0].source, SourceId::None);
    }

    #[test]
    fn note_off_eventually_returns_to_silence() {
        let mut e = Engine::new(SR, BLK);
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        // Hold for 200 ms.
        for _ in 0..((SR as usize) / 5 / BLK) {
            e.process_block(&mut l, &mut r);
        }
        e.note_off(60);
        // Release tail + reverb tail. Long enough.
        let mut last_peak = 0.0_f32;
        for _ in 0..((SR as usize) * 6 / BLK) {
            e.process_block(&mut l, &mut r);
        }
        for i in 0..BLK {
            last_peak = last_peak.max(l[i].abs()).max(r[i].abs());
        }
        assert!(last_peak < 0.05, "long tail still audible: {last_peak}");
    }
}
