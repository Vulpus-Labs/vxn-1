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
use vxn2_dsp::filter::{OtaLadderCoeffs, OtaLadderKernel};
use vxn2_dsp::halfband::{Interpolator, Oversampler};
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
const CUTOFF_IDX: usize = DestId::Cutoff.idx().unwrap();
const RESONANCE_IDX: usize = DestId::Resonance.idx().unwrap();
/// Lowest cutoff the ladder is driven to — C0 (MIDI 12), ≈16.35 Hz (VXN-1
/// parity). Lets a fully key-tracked, C0-based cutoff reach bass pitches.
const CUTOFF_MIN_HZ: f32 = 16.3516;
/// Highest cutoff the ladder is driven to (the `filter-cutoff` param ceiling).
const CUTOFF_MAX_HZ: f32 = 20_000.0;
/// Key-tracking centre note — C0 (MIDI 12). At full key-track the cutoff shifts
/// `(note − 12)/12` octaves, so a C0-floored cutoff tracks the played pitch.
const KEYTRACK_CENTRE_NOTE: f32 = 12.0;

/// Filter saturator headroom / gain-staging. VXN-2 filters the post-stack-sum,
/// which runs far hotter (rms ≈ 1.6, peaks ≈ 4–5) than VXN-1's per-voice input,
/// so the ladder's per-stage `tanh` compresses deep into its knee — ≈ −7 dB at
/// default drive even at density 1. We trim the signal into the saturator's
/// near-linear region and make it up after, so `drive = 1` is ≈ transparent at
/// the passband (cutoff open) and `drive` stays the knob that pushes into
/// saturation. Equivalent to a `tanh` headroom of `1/TRIM`; self-oscillation
/// stays bounded (the kernel limit cycle is ±~1, scaled by the make-up). Lives
/// engine-side so the ported kernel and its unit tests are untouched.
const FILTER_IN_TRIM: f32 = 0.2;
const FILTER_OUT_MAKEUP: f32 = 1.0 / FILTER_IN_TRIM;

/// Key-tracking cutoff offset in octaves: `(note − 12)/12 × amount`, centred on
/// C0 (`amount` ∈ [0,1]). With the base cutoff at the [`CUTOFF_MIN_HZ`] C0
/// floor, `amount = 1` makes the resulting `CUTOFF_MIN_HZ · 2^offset` equal the
/// played note's pitch (`midi_to_hz`), i.e. the cutoff tracks the keyboard 1:1.
#[inline]
fn keytrack_octaves(note: u8, amount: f32) -> f32 {
    (note as f32 - KEYTRACK_CENTRE_NOTE) / 12.0 * amount
}
/// Largest oversample factor the filter path supports (`filter-oversample`
/// tops out at 8×). Sizes the per-voice OS scratch and OS bus buffers.
const MAX_OVERSAMPLE: usize = 8;

/// Quiescence floor for the per-stack filter skip (ticket 0085), in ladder
/// state magnitude. An idle stack feeds the filter exact zero, so once *every*
/// ladder state (both L/R kernels) is below this, the filter's remaining output
/// is bounded by ≈ this value — about −100 dBFS, inaudible — and the
/// upsample + ladder + accumulate for that stack can be skipped exactly.
/// Deliberately *not* a denormal floor: it must sit below audibility yet above
/// the level a resonant tail rings through, so a ringing release is preserved
/// (it keeps re-filtering until truly settled) while a self-oscillating filter
/// — whose state never decays — is never wrongly skipped.
const FILTER_QUIESCENT_EPS: f32 = 1.0e-5;
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

/// Per-sample ramp snap band (per-block increment, level/gain units). A
/// converged ramp sits within f32 rounding of its target, never exactly on
/// it — increments below this band snap the state onto the target and count
/// as inactive, so a settled sound releases the per-sample advance.
const RAMP_SNAP_EPS: f32 = 1e-9;

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
    /// note-on reused the slot, so every smoothing/ramp state (pitch
    /// smoother, level + pan ramps) snaps instead of gliding in from the
    /// previous voice's modulation.
    mod_seq: [u64; N_STACKS],
    /// Per-stack per-sample increments ramping `stack.op_level_mod` and the
    /// folded pan gains `stack.pan_l` / `pan_r` linearly to each block's
    /// matrix targets (ticket 0074 — kills block-edge zipper on level/pan
    /// routes). Engine-owned so the `stack_tick_*` hot path stays untouched;
    /// the render loop advances them once per sample while live.
    level_mod_inc: Vec<[[f32; STACK_LANES]; vxn2_dsp::algo::N_OPS]>,
    pan_l_inc: Vec<[[f32; STACK_LANES]; vxn2_dsp::algo::N_OPS]>,
    pan_r_inc: Vec<[[f32; STACK_LANES]; vxn2_dsp::algo::N_OPS]>,
    /// Each op's EG level as seen by the previous block's ramp targets
    /// (ticket 0077). The ramp interpolates the *combined* effective level
    /// `eg + mod`; when the EG marches at the block edge, `op_level_mod` is
    /// rebased by `prev_eg - eg` so the sum the tick reads stays continuous,
    /// and the EG's block delta rides the same per-lane ramp as the matrix
    /// mod — one ramp, no separate EG staircase.
    prev_eg_level: Vec<[f32; vxn2_dsp::algo::N_OPS]>,
    /// Which slots carry a live ramp this block; `any_ramp_live` is the
    /// whole-engine OR so a patch with static effective levels pays one
    /// branch per sample.
    ramp_live: [bool; N_STACKS],
    any_ramp_live: bool,

    // ── Optional per-voice filter (E007 / ADR 0004) ──────────────────────
    // Two scalar OTA-C ladder kernels per stack (L/R) — the filter runs on a
    // stack's summed stereo pair. Plus one interpolating resampler per stack
    // (per-voice upsample, stateful) and a single shared decimator per channel
    // (deferred decimation past the voice-sum). All allocated once; untouched
    // while `filter-enable` is off.
    filter_l: Vec<OtaLadderKernel>,
    filter_r: Vec<OtaLadderKernel>,
    interp_l: Vec<Interpolator>,
    interp_r: Vec<Interpolator>,
    decim_l: Oversampler,
    decim_r: Oversampler,
    /// Base-rate per-voice render scratch (`block_size`).
    base_l: Vec<f32>,
    base_r: Vec<f32>,
    /// Oversampled per-voice scratch (`block_size * MAX_OVERSAMPLE`).
    os_l: Vec<f32>,
    os_r: Vec<f32>,
    /// Oversampled voice-sum bus (`block_size * MAX_OVERSAMPLE`).
    bus_l: Vec<f32>,
    bus_r: Vec<f32>,
    /// Decimated dry result fed to the FX chain (`block_size`).
    dry_l: Vec<f32>,
    dry_r: Vec<f32>,
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
            mod_seq: [u64::MAX; N_STACKS],
            level_mod_inc: vec![[[0.0; STACK_LANES]; vxn2_dsp::algo::N_OPS]; N_STACKS],
            pan_l_inc: vec![[[0.0; STACK_LANES]; vxn2_dsp::algo::N_OPS]; N_STACKS],
            pan_r_inc: vec![[[0.0; STACK_LANES]; vxn2_dsp::algo::N_OPS]; N_STACKS],
            prev_eg_level: vec![[0.0; vxn2_dsp::algo::N_OPS]; N_STACKS],
            ramp_live: [false; N_STACKS],
            any_ramp_live: false,
            filter_l: vec![OtaLadderKernel::new(); N_STACKS],
            filter_r: vec![OtaLadderKernel::new(); N_STACKS],
            interp_l: vec![Interpolator::new(); N_STACKS],
            interp_r: vec![Interpolator::new(); N_STACKS],
            decim_l: Oversampler::new(),
            decim_r: Oversampler::new(),
            base_l: vec![0.0; block_size],
            base_r: vec![0.0; block_size],
            os_l: vec![0.0; block_size * MAX_OVERSAMPLE],
            os_r: vec![0.0; block_size * MAX_OVERSAMPLE],
            bus_l: vec![0.0; block_size * MAX_OVERSAMPLE],
            bus_r: vec![0.0; block_size * MAX_OVERSAMPLE],
            dry_l: vec![0.0; block_size],
            dry_r: vec![0.0; block_size],
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
            self.mod_seq[i] = u64::MAX;
            self.ramp_live[i] = false;
            self.prev_eg_level[i] = [0.0; vxn2_dsp::algo::N_OPS];
            self.filter_l[i].reset();
            self.filter_r[i].reset();
            self.interp_l[i].reset();
            self.interp_r[i].reset();
        }
        self.decim_l.reset();
        self.decim_r.reset();
        self.any_ramp_live = false;
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
            let dest = DestId::from_u8(row.dest);
            self.matrix.slots[s] = MatrixSlot {
                source,
                dest,
                // Stored / CLAP depth stays linear; semitone dests get the
                // cubic taper here so host automation and the UI widget see
                // the same response.
                depth: dest.cook_depth(depth),
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

    /// Sustain pedal (CC64). Poly-only: while held, a poly note-off is
    /// deferred until release. Solo mode keeps last-note-priority unchanged.
    pub fn set_sustain(&mut self, on: bool) {
        self.alloc.set_sustain(on);
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
        // Block-rate dispatch: the filter-enable flag selects one of two render
        // bodies (ADR 0004 §5) — no per-sample branch. Read once here so the
        // matrix loop can reset filter state on fresh notes.
        let filter_enabled = self.params.filter.enable;

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
                self.ramp_live[i] = false;
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

            // Project per-op level + pan destinations into the stack.
            // Indices: OpiLevel=i*3+1, OpiPan=i*3+2. Neither applies as a
            // block constant: level ramps linearly to this block's target
            // via per-sample increments, and pan ramps the folded equal-
            // power gains the same way (ticket 0074). Pitch-shaped
            // destinations ride the per-stack PitchSmoother instead (0063).
            let mut level_targets = [[0.0_f32; STACK_LANES]; vxn2_dsp::algo::N_OPS];
            let stack = &mut self.alloc.stacks[i];
            for op_i in 0..vxn2_dsp::algo::N_OPS {
                let level_idx = op_i * 3 + 1;
                let pan_idx = op_i * 3 + 2;
                for k in 0..STACK_LANES {
                    level_targets[op_i][k] = self.dest_vals[i][k][level_idx];
                    stack.op_pan_mod[op_i][k] = self.dest_vals[i][k][pan_idx];
                }
            }
            // Capture this block's pitch-dest targets. A slot whose
            // allocation generation changed since the last block carries a
            // fresh note — snap every smoothing/ramp state (pitch smoother,
            // level + pan ramps) so the new voice doesn't glide in from the
            // previous voice's modulation.
            self.pitch_targets[i] = self.pitch_smoothers[i].targets_from(&self.dest_vals[i]);
            let seq = self.alloc.slot_seq(i);
            let fresh = seq != self.mod_seq[i];
            if fresh {
                self.mod_seq[i] = seq;
                self.pitch_smoothers[i].snap_to(&self.pitch_targets[i]);
                // A re-used slot carries a fresh note — clear its filter state
                // (kernels + interpolators) so the new voice starts clean
                // (ADR 0004: `reset()` on note-on). Only when the filter is on;
                // off, the state is inert anyway.
                if filter_enabled {
                    self.filter_l[i].reset();
                    self.filter_r[i].reset();
                    self.interp_l[i].reset();
                    self.interp_r[i].reset();
                }
            }
            let stack = &mut self.alloc.stacks[i];
            project_pitch_state(stack, self.pitch_smoothers[i].current());
            // Level modulation is MULTIPLICATIVE on the EG (ticket 0078):
            // effective level = `clamp(eg · (1 + m), 0, 1)`, with `m` the
            // matrix accumulator. The tick reads `eg + op_level_mod`, so the
            // engine projects the multiplicative target into that additive
            // offset: `op_level_mod_target = clamp(eg·(1+m), 0, 1) − eg`.
            //
            // Why multiplicative (vs the additive `eg + m` it replaced):
            // `eg = 0` forces eff = 0, so a RELEASED op always closes —
            // additive mod could refill what release drains and leave a voice
            // droning until the allocator cut it at full amplitude (a click).
            // A full-depth sine gates through silence at its trough, where the
            // LFO's own slope is zero, so tremolo gating is C¹-smooth — no
            // bottom corner to round, which is why the 0076 target one-pole
            // measured zero effect and was removed.
            //
            // The block-rate `clamp(…, 0, 1)` is the ONE bound for the whole
            // path: it absorbs boost overflow (`eg·(1+m) > 1` when eg > 0.5)
            // and multi-route `m` overflow (several slots summing into one
            // level dest) alike. Because both the ramp's start point (the
            // previous block's in-range effective level, carried by the EG
            // rebase below) and its end point (this clamped target) give
            // `eff ∈ [0, 1]`, the per-sample linear ramp stays in range too —
            // so `stack_tick_*` needs no per-sample clamp.
            //
            // The EG marches once per block (0077); `op_level_mod` is rebased
            // by the block delta so the sum the tick reads stays continuous
            // across the edge, then ramps to the new target — the EG's motion
            // rides the same per-lane ramp as the matrix mod (no block-rate EG
            // staircase). Static patches with settled EGs pass through
            // bit-exact: m = 0 → target offset 0, rebase +0.
            let prev_eg = &mut self.prev_eg_level[i];
            for op_i in 0..vxn2_dsp::algo::N_OPS {
                let eg = stack.ops[op_i].eg.level;
                for k in 0..STACK_LANES {
                    let eff = (eg * (1.0 + level_targets[op_i][k])).clamp(0.0, 1.0);
                    level_targets[op_i][k] = eff - eg;
                    if !fresh {
                        // Keep `eg + op_level_mod` continuous across the EG's
                        // block-edge march; the delta is folded into this
                        // block's ramp instead.
                        stack.op_level_mod[op_i][k] += prev_eg[op_i] - eg;
                    }
                }
                prev_eg[op_i] = eg;
            }
            if fresh {
                stack.op_level_mod = level_targets;
                stack.refresh_pan_with_mod();
                self.ramp_live[i] = false;
            } else {
                let inv = 1.0 / n as f32;
                let (pan_l_t, pan_r_t) = stack.pan_targets();
                let mut any = false;
                let lvl_inc = &mut self.level_mod_inc[i];
                let pl_inc = &mut self.pan_l_inc[i];
                let pr_inc = &mut self.pan_r_inc[i];
                for op_i in 0..vxn2_dsp::algo::N_OPS {
                    for k in 0..STACK_LANES {
                        // A ramp lands within f32 rounding of its target, so
                        // a settled value never compares exactly equal — snap
                        // inside RAMP_SNAP_EPS (≈ −120 dB) so a static sound
                        // releases the per-sample advance.
                        let mut dl =
                            (level_targets[op_i][k] - stack.op_level_mod[op_i][k]) * inv;
                        if dl.abs() < RAMP_SNAP_EPS {
                            stack.op_level_mod[op_i][k] = level_targets[op_i][k];
                            dl = 0.0;
                        }
                        let mut pl = (pan_l_t[op_i][k] - stack.pan_l[op_i][k]) * inv;
                        if pl.abs() < RAMP_SNAP_EPS {
                            stack.pan_l[op_i][k] = pan_l_t[op_i][k];
                            pl = 0.0;
                        }
                        let mut pr = (pan_r_t[op_i][k] - stack.pan_r[op_i][k]) * inv;
                        if pr.abs() < RAMP_SNAP_EPS {
                            stack.pan_r[op_i][k] = pan_r_t[op_i][k];
                            pr = 0.0;
                        }
                        lvl_inc[op_i][k] = dl;
                        pl_inc[op_i][k] = pl;
                        pr_inc[op_i][k] = pr;
                        any |= dl != 0.0 || pl != 0.0 || pr != 0.0;
                    }
                }
                self.ramp_live[i] = any;
            }
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
            // Refresh pitch from the new offsets so the per-sample loop
            // reads phase_inc that includes this block's matrix output.
            // Cost: per active stack 6×8 powf — affordable at ≤16 stacks.
            // (Pan gains are handled by the ramp above — ticket 0074.)
            stack.apply_pitch_mult();

            // FX dests aggregate at lane 0 across active stacks. Lane 0
            // sees patch-source contributions exactly once; per-stack
            // sources (velocity, mod env, …) average naturally across the
            // active stacks below.
            fx_delay_mix_sum += self.dest_vals[i][0][DELAY_MIX_IDX];
            fx_reverb_mix_sum += self.dest_vals[i][0][REVERB_MIX_IDX];
            fx_active += 1;
        }

        self.any_ramp_live = self.ramp_live.iter().any(|&b| b);

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

        if filter_enabled {
            // ON path — stack-major oversampled filter (ADR 0004 §3–§5).
            self.render_block_filtered(out_l, out_r, n);
        } else {
            // OFF path — the tuned sample-major loop, byte-for-byte unchanged.
            // Per-sample: sum every active stack into the dry bus, then through
            // delay + reverb + master gain. Every PITCH_SMOOTH_QUANTUM samples
            // the pitch smoothers advance one step toward this block's targets
            // and the affected stacks re-cook `phase_inc` — converged smoothers
            // (no active pitch route) skip the recook entirely.
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
                if self.any_ramp_live {
                    self.advance_mod_ramps();
                }
                let (cl, cr) = self.cleanup.process(dry_l, dry_r);
                let (l, r) = self.delay.process(cl, cr);
                let (l, r) = self.reverb.process(l, r);
                let (l, r) = self.master.apply(l, r);
                out_l[sample] = l;
                out_r[sample] = r;
            }
        }
    }

    /// ON-path render (ADR 0004 §3–§5): stack-major, oversampled per-voice
    /// filter with a single shared decimation deferred past the voice-sum.
    ///
    /// For each active stack we render its whole base-rate block (advancing
    /// *that stack's* pitch smoother every quantum and its mod-ramp every
    /// sample — licensed because every per-sample control field is already
    /// per-stack, so the stack-major reorder reproduces the OFF path's
    /// per-voice output exactly), upsample it, run its L/R ladder at the
    /// oversampled rate, and accumulate into the oversampled bus. After all
    /// stacks, one shared decimator brings the bus back to base rate; the FX
    /// chain then runs per sample exactly as in the OFF path.
    ///
    /// Factored so ticket 0085 can drop a per-stack quiescence-skip in front
    /// of the inner render and 0086 can read the resampler group delay.
    fn render_block_filtered(&mut self, out_l: &mut [f32], out_r: &mut [f32], n: usize) {
        let fp = self.params.filter;
        let f = fp.oversample.clamp(1, MAX_OVERSAMPLE);

        if f == 1 {
            // Fused unity-rate path: ladder runs directly on each stack's
            // stereo pair and accumulates straight into the dry bus — no
            // resamplers, no OS scratch/bus, no extra buffer passes. This is
            // the "sum + ladder → FX" shape the 1× setting should cost. (No
            // oversampling ⇒ no anti-alias filtering and no added latency; the
            // ladder's `tanh` aliases — the documented 1× tradeoff.)
            self.dry_l[..n].fill(0.0);
            self.dry_r[..n].fill(0.0);
            let os_rate = self.sample_rate;
            for i in 0..N_STACKS {
                let idle = self.alloc.stacks[i].is_idle();
                // Quiescence-skip (0085): an idle stack feeds the filter exact
                // zero; once its ladder has rung out, skipping the tick is
                // exact (zero contribution). State + frozen coeffs are left
                // untouched so re-entry on the next note is glitch-free.
                if idle && self.stack_filter_quiescent(i) {
                    continue;
                }
                self.set_stack_filter_coeffs(i, os_rate, fp);
                if idle {
                    // Ring-out: voice is silent but the resonant tail is still
                    // ringing. Pump zeros through the ladder so the tail decays
                    // naturally instead of being clipped. No stack tick / pitch
                    // / mod advance — all frozen until the next note-on.
                    for sample in 0..n {
                        self.dry_l[sample] += self.filter_l[i].tick(0.0) * FILTER_OUT_MAKEUP;
                        self.dry_r[sample] += self.filter_r[i].tick(0.0) * FILTER_OUT_MAKEUP;
                    }
                    continue;
                }
                for sample in 0..n {
                    if sample % PITCH_SMOOTH_QUANTUM == 0 {
                        self.advance_pitch_smoother_one(i);
                    }
                    let (sl, sr) = stack_tick_stereo(&mut self.alloc.stacks[i]);
                    self.dry_l[sample] +=
                        self.filter_l[i].tick(sl * FILTER_IN_TRIM) * FILTER_OUT_MAKEUP;
                    self.dry_r[sample] +=
                        self.filter_r[i].tick(sr * FILTER_IN_TRIM) * FILTER_OUT_MAKEUP;
                    if self.ramp_live[i] {
                        self.advance_mod_ramp_one(i);
                    }
                }
            }
        } else {
            // Oversampled path: per-voice upsample → ladder@F → accumulate into
            // the OS bus, then ONE shared decimation past the voice-sum.
            let osn = n * f;
            debug_assert!(osn <= self.bus_l.len(), "OS bus overflow: {osn}");
            let os_rate = self.sample_rate * f as f32;
            self.bus_l[..osn].fill(0.0);
            self.bus_r[..osn].fill(0.0);

            for i in 0..N_STACKS {
                let idle = self.alloc.stacks[i].is_idle();
                // Quiescence-skip (0085): skip the upsample + ladder for an idle
                // stack only once its filter has settled. The interp FIR history
                // self-flushes within tap-length zero-input samples, so by the
                // time the ladder reads quiescent the resampler tail is gone too
                // — skipping is exact (zero contribution to the OS bus).
                if idle && self.stack_filter_quiescent(i) {
                    continue;
                }
                self.set_stack_filter_coeffs(i, os_rate, fp);

                // 1. Render this stack's whole block to base-rate scratch.
                //    Idle-but-ringing stacks feed zeros — the resonant tail
                //    still rings out through the interpolator + ladder rather
                //    than being clipped, while stack / pitch / mod state stays
                //    frozen until the next note-on.
                if idle {
                    self.base_l[..n].fill(0.0);
                    self.base_r[..n].fill(0.0);
                } else {
                    for sample in 0..n {
                        if sample % PITCH_SMOOTH_QUANTUM == 0 {
                            self.advance_pitch_smoother_one(i);
                        }
                        let (sl, sr) = stack_tick_stereo(&mut self.alloc.stacks[i]);
                        self.base_l[sample] = sl;
                        self.base_r[sample] = sr;
                        if self.ramp_live[i] {
                            self.advance_mod_ramp_one(i);
                        }
                    }
                }

                // 2. Upsample (mandatory anti-image LP) → per-voice OS scratch.
                self.interp_l[i].interpolate(&self.base_l[..n], &mut self.os_l[..osn], f);
                self.interp_r[i].interpolate(&self.base_r[..n], &mut self.os_r[..osn], f);

                // 3 + 4. Ladder at the oversampled rate, accumulate into the bus.
                //         Trim into the saturator's linear region + make up after
                //         (gain-staging, see `FILTER_IN_TRIM`).
                for j in 0..osn {
                    self.bus_l[j] += self.filter_l[i].tick(self.os_l[j] * FILTER_IN_TRIM)
                        * FILTER_OUT_MAKEUP;
                    self.bus_r[j] += self.filter_r[i].tick(self.os_r[j] * FILTER_IN_TRIM)
                        * FILTER_OUT_MAKEUP;
                }
            }

            // 5. One shared decimation past the voice-sum (linear ⇒ exact).
            self.decim_l.decimate(&self.bus_l[..osn], &mut self.dry_l[..n], f);
            self.decim_r.decimate(&self.bus_r[..osn], &mut self.dry_r[..n], f);
        }

        // FX chain per sample, exactly as the OFF path.
        for sample in 0..n {
            let (cl, cr) = self.cleanup.process(self.dry_l[sample], self.dry_r[sample]);
            let (l, r) = self.delay.process(cl, cr);
            let (l, r) = self.reverb.process(l, r);
            let (l, r) = self.master.apply(l, r);
            out_l[sample] = l;
            out_r[sample] = r;
        }
    }

    /// True when stack `i`'s filter has rung out — both L/R ladder kernels'
    /// state magnitudes sit below [`FILTER_QUIESCENT_EPS`]. Only meaningful for
    /// an idle stack (one feeding the filter exact zero): the caller pairs this
    /// with `is_idle()` so the skip keys on the *filter* settling, not the input
    /// going silent (VXN1's `silent-skip-filter-state` lesson — silent ≠
    /// quiescent at high resonance).
    #[inline]
    fn stack_filter_quiescent(&self, i: usize) -> bool {
        self.filter_l[i].state_abs_max() < FILTER_QUIESCENT_EPS
            && self.filter_r[i].state_abs_max() < FILTER_QUIESCENT_EPS
    }

    /// Compute + install this block's frozen ladder coefficients for stack `i`
    /// (ADR 0004 §7). Cutoff modulates in the log/octave domain
    /// (`base · 2^(matrix octaves)`); resonance is an additive `[0, 1]` offset;
    /// both collapse to a per-stack scalar via lane 0. Computed at the
    /// `os_rate` actually used so `compute_g`'s fs-dependent pole detune stays
    /// correct at every oversample factor.
    #[inline]
    fn set_stack_filter_coeffs(&mut self, i: usize, os_rate: f32, fp: crate::shared::FilterParams) {
        // Dedicated key-tracking (VXN-1 `FilterKeyTrack`), added to the matrix
        // cutoff modulation (both in octaves).
        let keytrack_oct = keytrack_octaves(self.alloc.stacks[i].note, fp.keytrack);
        let cutoff_oct = self.dest_vals[i][0][CUTOFF_IDX] + keytrack_oct;
        let cutoff_hz = (fp.cutoff_hz * cutoff_oct.exp2()).clamp(CUTOFF_MIN_HZ, CUTOFF_MAX_HZ);
        let resonance = (fp.resonance + self.dest_vals[i][0][RESONANCE_IDX]).clamp(0.0, 1.0);
        let coeffs = OtaLadderCoeffs::new(cutoff_hz, os_rate, resonance, fp.drive);
        self.filter_l[i].set_coeffs(coeffs);
        self.filter_r[i].set_coeffs(coeffs);
        self.filter_l[i].set_response(fp.mode, fp.slope);
        self.filter_r[i].set_response(fp.mode, fp.slope);
    }

    /// Single-stack pitch-smoother advance — the body of
    /// [`Self::advance_pitch_smoothers`] for one slot, used by the stack-major
    /// filter path so each stack advances inside its own block loop. Per-stack
    /// smoother state is independent, so this reproduces the OFF path's
    /// per-voice result exactly.
    #[inline]
    fn advance_pitch_smoother_one(&mut self, i: usize) {
        if self.alloc.stacks[i].is_idle() {
            return;
        }
        if self.pitch_smoothers[i].converged(&self.pitch_targets[i], PITCH_SMOOTH_EPS_ST) {
            return;
        }
        let st = self.pitch_smoothers[i].tick(&self.pitch_targets[i]);
        let stack = &mut self.alloc.stacks[i];
        project_pitch_state(stack, st);
        stack.apply_pitch_mult();
    }

    /// Single-stack mod-ramp advance — the body of
    /// [`Self::advance_mod_ramps`] for one slot (stack-major filter path).
    #[inline]
    fn advance_mod_ramp_one(&mut self, i: usize) {
        let stack = &mut self.alloc.stacks[i];
        let lvl_inc = &self.level_mod_inc[i];
        let pl_inc = &self.pan_l_inc[i];
        let pr_inc = &self.pan_r_inc[i];
        for op_i in 0..vxn2_dsp::algo::N_OPS {
            for k in 0..STACK_LANES {
                stack.op_level_mod[op_i][k] += lvl_inc[op_i][k];
                stack.pan_l[op_i][k] += pl_inc[op_i][k];
                stack.pan_r[op_i][k] += pr_inc[op_i][k];
            }
        }
    }

    /// Advance the live level/pan ramps one sample (ticket 0074): straight
    /// lane-strided adds into the stacks' `op_level_mod` / `pan_l` / `pan_r`.
    /// Lives engine-side so the `stack_tick_*` hot path keeps its exact
    /// pre-0074 shape; only ramping slots pay anything.
    fn advance_mod_ramps(&mut self) {
        for i in 0..N_STACKS {
            if !self.ramp_live[i] {
                continue;
            }
            let stack = &mut self.alloc.stacks[i];
            let lvl_inc = &self.level_mod_inc[i];
            let pl_inc = &self.pan_l_inc[i];
            let pr_inc = &self.pan_r_inc[i];
            for op_i in 0..vxn2_dsp::algo::N_OPS {
                for k in 0..STACK_LANES {
                    stack.op_level_mod[op_i][k] += lvl_inc[op_i][k];
                    stack.pan_l[op_i][k] += pl_inc[op_i][k];
                    stack.pan_r[op_i][k] += pr_inc[op_i][k];
                }
            }
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

    const SR: f32 = 48_000.0;
    const BLK: usize = 64;

    /// Full key-tracking + a C0-floored base cutoff lands the cutoff exactly on
    /// the played note's pitch (VXN-1 parity): `CUTOFF_MIN_HZ · 2^offset ==
    /// midi_to_hz(note)`. Also: centred on C0 (zero at note 12) and linear in
    /// amount.
    #[test]
    fn keytrack_full_lands_cutoff_on_note_pitch() {
        for note in [24u8, 36, 48, 60, 72, 96] {
            let cutoff = CUTOFF_MIN_HZ * keytrack_octaves(note, 1.0).exp2();
            let pitch = vxn2_dsp::op::midi_to_hz(note);
            assert!(
                (cutoff - pitch).abs() / pitch < 1e-3,
                "note {note}: key-tracked cutoff {cutoff} ≠ pitch {pitch}",
            );
        }
        assert_eq!(keytrack_octaves(12, 1.0), 0.0, "not centred on C0");
        assert!((keytrack_octaves(24, 0.5) - 0.5).abs() < 1e-6, "amount not linear");
        assert_eq!(keytrack_octaves(60, 0.0), 0.0, "zero amount must not track");
    }

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
        // Upper bound loosened -9 → -8 dBFS with the 0079 feedback
        // recalibration: FB 6 dropped from the chaotic zone (scale 2.0) to a
        // stable 0.5, which concentrates op6's energy tonally and lifts the
        // patch ~1 dB.
        assert!(
            (-24.0..=-8.0).contains(&attack_db),
            "early-sustain RMS {attack_db} dBFS outside [-24, -8]"
        );

        // Mid-sustain near t ≈ 1.0 s.
        let blocks_so_far = 4 + (((SR * 0.1) as usize) / BLK) + blocks_per_window;
        let target_blocks = ((SR * 1.0) as usize) / BLK;
        let _ = render_and_rms(&mut e, target_blocks.saturating_sub(blocks_so_far));
        let (_, sustain_db, _) = render_and_rms(&mut e, blocks_per_window);
        assert!(
            (-24.0..=-8.0).contains(&sustain_db),
            "sustain RMS {sustain_db} dBFS outside [-24, -8]"
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
        modulated.matrix.slots[0] = MatrixSlot {
            source: SourceId::Lfo1,
            dest: DestId::Op1Level,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        modulated.note_on(60, 100);

        let mut baseline = Engine::new(SR, BLK);
        baseline.params.mod_params.lfo1.rate_hz = 5.0;
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

    /// Level + pan matrix routes ramp to each block's target instead of
    /// stepping at block edges (ticket 0074): after every `process_block`
    /// the stack's level mod has converged on that block's accumulator
    /// value, the ramp flag is live while the LFO moves, and a static
    /// patch keeps the flag off entirely.
    #[test]
    fn level_pan_mod_ramps_converge_each_block() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        let mut e = Engine::new(SR, BLK);
        e.params.mod_params.lfo1.rate_hz = 6.0;
        e.params.patch.stack.density = 4; // give pan spread lanes to move
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::Lfo1,
            dest: DestId::Op1Level,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        e.matrix.slots[1] = MatrixSlot {
            source: SourceId::ModWheel,
            dest: DestId::Op1Pan,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        e.set_mod_wheel(0.7);
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        e.process_block(&mut l, &mut r); // fresh-note block snaps

        let slot = (0..N_STACKS)
            .find(|&i| !e.alloc.stacks[i].is_idle())
            .expect("note is held");
        let level_idx = DestId::Op1Level.idx().unwrap();
        // Replicate the engine's multiplicative projection (ticket 0078):
        // `target = clamp(eg·(1+m), 0, 1) − eg`, taken against the op's
        // post-tick EG level (0077), which is what `eg.level` holds after
        // `process_block` returns. The ramp must converge each block on this
        // target — no smoothing.
        let target = |e: &Engine, k: usize| {
            let eg = e.alloc.stacks[slot].ops[0].eg.level;
            let m = self::tests_dest_val(e, slot, k, level_idx);
            (eg * (1.0 + m)).clamp(0.0, 1.0) - eg
        };
        let mut saw_ramp = false;
        for _ in 0..30 {
            e.process_block(&mut l, &mut r);
            saw_ramp |= e.ramp_live[slot];
            for k in 0..STACK_LANES {
                let got = e.alloc.stacks[slot].op_level_mod[0][k];
                assert!(
                    (got - target(&e, k)).abs() < 1e-3,
                    "lane {k}: level mod {got} hasn't converged on target {}",
                    target(&e, k)
                );
            }
        }
        assert!(saw_ramp, "moving LFO must keep the ramp flag live");
    }

    /// Helper: read the block dest accumulator (private field access from
    /// the test module).
    fn tests_dest_val(e: &Engine, slot: usize, lane: usize, dest_idx: usize) -> f32 {
        e.dest_vals[slot][lane][dest_idx]
    }

    /// With no moving level/pan route and settled EGs the ramp flag must
    /// clear so a static sound doesn't pay the per-sample advance. The
    /// combined ramp (0077) legitimately runs while any EG marches — the
    /// default E.PIANO's modulator tails decay for ~10 s — so this test
    /// pins every EG to a flat sustain instead.
    #[test]
    fn static_patch_keeps_mod_ramps_inactive() {
        let mut e = Engine::new(SR, BLK);
        for op in &mut e.params.patch.voice.ops {
            op.eg.r = [99, 99, 99, 99];
            op.eg.l = [99, 99, 99, 0];
        }
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        // Rate-99 attacks land on their targets within milliseconds; after
        // 0.5 s every EG sits exactly on its sustain level.
        for _ in 0..(SR as usize / 2 / BLK) {
            e.process_block(&mut l, &mut r);
        }
        let slot = (0..N_STACKS)
            .find(|&i| !e.alloc.stacks[i].is_idle())
            .expect("note is held");
        assert!(
            !e.ramp_live[slot] && !e.any_ramp_live,
            "static patch must not pay the per-sample ramp once EGs settle"
        );
    }

    /// Multiplicative level mod (ticket 0078): a released voice must decay
    /// to silence even with a full-depth positive LFO on a carrier level.
    /// Under the old additive semantics the LFO refilled what release
    /// drained — the voice droned at the LFO level until the allocator's
    /// idle detection cut it at full amplitude (a loud click on every
    /// chord release, found in a DAW bounce).
    #[test]
    fn released_voice_closes_under_positive_level_mod() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        let mut e = Engine::new(SR, BLK);
        e.params.delay.on = false;
        e.params.delay.mix = 0.0;
        e.params.reverb.on = false;
        e.params.reverb.mix = 0.0;
        e.params.mod_params.lfo1.rate_hz = 5.0;
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::Lfo1,
            dest: DestId::Op1Level,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        for _ in 0..(SR as usize / 4 / BLK) {
            e.process_block(&mut l, &mut r);
        }
        e.note_off(60);
        // 0.5 s after release: well past every op's release tail. The voice
        // must be silent (no LFO-held zombie) and the render must contain no
        // idle-cut step.
        let mut peak_tail = 0.0_f32;
        let blocks = SR as usize / 2 / BLK;
        for b in 0..blocks {
            e.process_block(&mut l, &mut r);
            if b > blocks / 2 {
                for &x in &l {
                    peak_tail = peak_tail.max(x.abs());
                }
            }
        }
        assert!(
            peak_tail < 1e-3,
            "released voice still audible under positive level mod: peak {peak_tail}"
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

    /// Semitone-dest depths take the cubic taper at slot-cook time, on both
    /// depth sources (CLAP `mtx_depths` for slots 0..N_CLAP_DEPTH_SLOTS,
    /// raw row depth above). Non-pitch dests stay linear.
    #[test]
    fn apply_block_params_tapers_semitone_depths() {
        use crate::matrix::{DestId, SourceId};
        use crate::shared::MatrixRowRaw;

        let mut e = Engine::new(SR, BLK);
        // CLAP-automatable slot: depth comes from mtx_depths.
        e.params.matrix_rows[0] = MatrixRowRaw {
            source: SourceId::Lfo1 as u8,
            dest: DestId::GlobalPitch as u8,
            curve: 0,
            active: true,
            depth: 0.0,
        };
        e.params.mtx_depths[0] = 0.5;
        // Non-pitch dest at the same depth: passthrough.
        e.params.matrix_rows[1] = MatrixRowRaw {
            source: SourceId::Lfo1 as u8,
            dest: DestId::Op1Level as u8,
            curve: 0,
            active: true,
            depth: 0.0,
        };
        e.params.mtx_depths[1] = 0.5;
        // Non-CLAP slot: depth rides in the row.
        let hi = N_CLAP_DEPTH_SLOTS;
        e.params.matrix_rows[hi] = MatrixRowRaw {
            source: SourceId::ModWheel as u8,
            dest: DestId::Op2Pitch as u8,
            curve: 0,
            active: true,
            depth: -0.5,
        };
        e.apply_block_params();

        assert!((e.matrix.slots[0].depth - 0.125).abs() < 1e-7);
        assert!((e.matrix.slots[1].depth - 0.5).abs() < 1e-7);
        assert!((e.matrix.slots[hi].depth - -0.125).abs() < 1e-7);
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

    // ── E007 filter render path (ticket 0084) ────────────────────────────

    use crate::shared::FilterParams;
    use vxn2_dsp::filter::{FilterMode, FilterSlope};

    /// Render `blocks` blocks of a held middle-C and return summed L+R energy.
    fn render_energy(e: &mut Engine, blocks: usize) -> f64 {
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        let mut energy = 0.0_f64;
        for _ in 0..blocks {
            e.process_block(&mut l, &mut r);
            for i in 0..BLK {
                energy += (l[i] as f64) * (l[i] as f64) + (r[i] as f64) * (r[i] as f64);
            }
        }
        energy
    }

    /// With the filter OFF the render path is unchanged: cutoff / mode / drive
    /// must have zero effect on the output (bit-identical to a default engine).
    #[test]
    fn filter_off_ignores_filter_params() {
        let mut baseline = Engine::new(SR, BLK);
        baseline.note_on(60, 100);

        let mut tweaked = Engine::new(SR, BLK);
        tweaked.params_mut().filter = FilterParams {
            enable: false,
            cutoff_hz: 120.0,
            resonance: 0.9,
            mode: FilterMode::Hp,
            slope: FilterSlope::Pole2,
            drive: 8.0,
            oversample: 8,
            keytrack: 0.0,
        };
        tweaked.note_on(60, 100);

        let (mut bl, mut br) = ([0.0_f32; BLK], [0.0_f32; BLK]);
        let (mut tl, mut tr) = ([0.0_f32; BLK], [0.0_f32; BLK]);
        for _ in 0..40 {
            baseline.process_block(&mut bl, &mut br);
            tweaked.process_block(&mut tl, &mut tr);
            for i in 0..BLK {
                assert_eq!(bl[i], tl[i], "filter-off L diverged at block sample {i}");
                assert_eq!(br[i], tr[i], "filter-off R diverged");
            }
        }
    }

    /// A low-cutoff lowpass removes energy relative to a wide-open cutoff, for
    /// every oversample factor (the ladder is actually in the signal path).
    #[test]
    fn filter_on_lowpass_attenuates() {
        for f in [1usize, 2, 4, 8] {
            let mk = |cutoff: f32| {
                let mut e = Engine::new(SR, BLK);
                e.params_mut().filter = FilterParams {
                    enable: true,
                    cutoff_hz: cutoff,
                    resonance: 0.0,
                    mode: FilterMode::Lp,
                    slope: FilterSlope::Pole4,
                    drive: 1.0,
                    oversample: f,
                    keytrack: 0.0,
                };
                e.note_on(60, 110);
                e
            };
            let mut open = mk(20_000.0);
            let mut shut = mk(150.0);
            let e_open = render_energy(&mut open, 60);
            let e_shut = render_energy(&mut shut, 60);
            assert!(
                e_shut < 0.5 * e_open,
                "{f}× LP@150 ({e_shut:.4}) not clearly darker than LP@20k ({e_open:.4})"
            );
        }
    }

    /// Self-oscillation at resonance = 1 stays finite and bounded at every
    /// oversample factor (the `tanh` saturator caps the loop).
    #[test]
    fn filter_on_self_osc_is_bounded() {
        for f in [1usize, 2, 4, 8] {
            let mut e = Engine::new(SR, BLK);
            e.params_mut().filter = FilterParams {
                enable: true,
                cutoff_hz: 1500.0,
                resonance: 1.0,
                mode: FilterMode::Lp,
                slope: FilterSlope::Pole4,
                drive: 2.0,
                oversample: f,
                keytrack: 0.0,
            };
            e.note_on(60, 127);
            let mut l = [0.0_f32; BLK];
            let mut r = [0.0_f32; BLK];
            let mut peak = 0.0_f32;
            for _ in 0..((SR as usize) / 2 / BLK) {
                e.process_block(&mut l, &mut r);
                for i in 0..BLK {
                    assert!(l[i].is_finite() && r[i].is_finite(), "{f}× non-finite output");
                    peak = peak.max(l[i].abs()).max(r[i].abs());
                }
            }
            assert!(peak < 100.0, "{f}× self-osc blew up: peak {peak}");
        }
    }

    /// Every mode × slope produces finite, non-trivial output on a held note.
    #[test]
    fn filter_on_all_modes_render() {
        for mode in FilterMode::ALL {
            for slope in [FilterSlope::Pole2, FilterSlope::Pole4] {
                let mut e = Engine::new(SR, BLK);
                e.params_mut().filter = FilterParams {
                    enable: true,
                    cutoff_hz: 2000.0,
                    resonance: 0.3,
                    mode,
                    slope,
                    drive: 1.0,
                    oversample: 4,
                    keytrack: 0.0,
                };
                e.note_on(60, 110);
                let energy = render_energy(&mut e, 60);
                assert!(energy.is_finite(), "{mode:?}/{slope:?} non-finite");
                assert!(energy > 1e-6, "{mode:?}/{slope:?} produced silence");
            }
        }
    }

    /// A matrix `Cutoff` route audibly changes the filtered output: mod wheel
    /// at 0 vs 1 (driving Cutoff up many octaves) yields different energy.
    #[test]
    fn matrix_cutoff_modulates_filter() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};
        let mk = |wheel: f32| {
            let mut e = Engine::new(SR, BLK);
            e.params_mut().filter = FilterParams {
                enable: true,
                cutoff_hz: 200.0, // low base so upward modulation has room
                resonance: 0.0,
                mode: FilterMode::Lp,
                slope: FilterSlope::Pole4,
                drive: 1.0,
                oversample: 4,
                keytrack: 0.0,
            };
            e.matrix.slots[0] = MatrixSlot {
                source: SourceId::ModWheel,
                dest: DestId::Cutoff,
                depth: 1.0,
                curve: CurveKind::Lin,
            };
            e.set_mod_wheel(wheel);
            e.note_on(60, 110);
            e
        };
        let mut closed = mk(0.0);
        let mut open = mk(1.0);
        let e_closed = render_energy(&mut closed, 60);
        let e_open = render_energy(&mut open, 60);
        // Opening the cutoff (mod wheel up) passes more energy.
        assert!(
            e_open > 1.5 * e_closed,
            "Cutoff route had no effect: closed {e_closed:.5}, open {e_open:.5}"
        );
    }

    /// No allocation / panic across a filtered block with several voices and a
    /// non-power-of-two block length (resamplers tolerate any `n`).
    #[test]
    fn filtered_render_handles_odd_block_len() {
        let mut e = Engine::new(SR, 100);
        e.params_mut().filter = FilterParams {
            enable: true,
            cutoff_hz: 4000.0,
            resonance: 0.5,
            mode: FilterMode::Bp,
            slope: FilterSlope::Pole4,
            drive: 1.5,
            oversample: 8,
            keytrack: 0.0,
        };
        e.note_on(60, 100);
        e.note_on(64, 100);
        e.note_on(67, 100);
        let mut l = [0.0_f32; 100];
        let mut r = [0.0_f32; 100];
        for _ in 0..50 {
            e.process_block(&mut l, &mut r);
            for i in 0..100 {
                assert!(l[i].is_finite() && r[i].is_finite());
            }
        }
    }

    // ── E007 quiescence-skip (ticket 0085) ───────────────────────────────

    /// Per-block stereo peak, for tracking a release tail.
    fn block_peak(e: &mut Engine, l: &mut [f32; BLK], r: &mut [f32; BLK]) -> f32 {
        e.process_block(l, r);
        let mut p = 0.0_f32;
        for i in 0..BLK {
            assert!(l[i].is_finite() && r[i].is_finite());
            p = p.max(l[i].abs()).max(r[i].abs());
        }
        p
    }

    /// A resonant filter keeps ringing after the amp envelope hits zero. The
    /// quiescence-skip rings that tail *out* (feeding zeros through the ladder)
    /// rather than clipping it at idle — so a high-resonance release outlasts a
    /// low-resonance one. If the ring were cut at idle both would die together
    /// on the amp release. FX are silenced so we measure the filter, not reverb.
    #[test]
    fn resonant_release_tail_outlasts_non_resonant() {
        let tail_blocks = |resonance: f32| -> usize {
            let mut e = Engine::new(SR, BLK);
            e.params_mut().reverb.mix = 0.0;
            e.params_mut().delay.mix = 0.0;
            e.params_mut().filter = FilterParams {
                enable: true,
                cutoff_hz: 180.0,
                resonance,
                mode: FilterMode::Lp,
                slope: FilterSlope::Pole4,
                drive: 1.0,
                oversample: 2,
                keytrack: 0.0,
            };
            e.apply_block_params();
            e.note_on(60, 110);
            let (mut l, mut r) = ([0.0_f32; BLK], [0.0_f32; BLK]);
            for _ in 0..60 {
                block_peak(&mut e, &mut l, &mut r);
            }
            e.note_off(60);
            let mut last_audible = 0;
            for b in 0..6000 {
                if block_peak(&mut e, &mut l, &mut r) > 1e-4 {
                    last_audible = b;
                }
            }
            last_audible
        };
        let hi = tail_blocks(0.98);
        let lo = tail_blocks(0.0);
        assert!(
            hi > lo + 60,
            "resonant tail ({hi} blocks) not clearly longer than non-resonant ({lo})"
        );
    }

    /// A self-oscillating voice (resonance = 1) must never be skipped while it
    /// rings: its ladder state never decays, so `stack_filter_quiescent` stays
    /// false and the limit cycle keeps sounding long after note-off.
    #[test]
    fn self_oscillation_survives_note_off() {
        let mut e = Engine::new(SR, BLK);
        e.params_mut().reverb.mix = 0.0;
        e.params_mut().delay.mix = 0.0;
        e.params_mut().filter = FilterParams {
            enable: true,
            cutoff_hz: 1500.0,
            resonance: 1.0,
            mode: FilterMode::Lp,
            slope: FilterSlope::Pole4,
            drive: 2.0,
            oversample: 2,
            keytrack: 0.0,
        };
        e.apply_block_params();
        e.note_on(60, 127);
        let (mut l, mut r) = ([0.0_f32; BLK], [0.0_f32; BLK]);
        for _ in 0..60 {
            block_peak(&mut e, &mut l, &mut r);
        }
        e.note_off(60);
        // Render ~0.5 s past note-off, then sample the very tail.
        for _ in 0..((SR as usize) / 2 / BLK) {
            block_peak(&mut e, &mut l, &mut r);
        }
        let mut tail = 0.0_f32;
        for _ in 0..40 {
            tail = tail.max(block_peak(&mut e, &mut l, &mut r));
        }
        assert!(
            tail > 1e-3,
            "self-oscillation silenced after note-off (tail peak {tail}) — wrongly skipped"
        );
    }

    /// Re-entry after a skip is click-free: a fully-settled stack is skipped
    /// (state frozen ≈ 0), and note-on resets it, so the re-triggered note
    /// attacks from silence with no discontinuous spike on the first block.
    #[test]
    fn retrigger_after_skip_is_click_free() {
        let mut e = Engine::new(SR, BLK);
        e.params_mut().reverb.mix = 0.0;
        e.params_mut().delay.mix = 0.0;
        e.params_mut().filter = FilterParams {
            enable: true,
            cutoff_hz: 800.0,
            resonance: 0.6,
            mode: FilterMode::Lp,
            slope: FilterSlope::Pole4,
            drive: 1.0,
            oversample: 4,
            keytrack: 0.0,
        };
        e.apply_block_params();
        e.note_on(60, 110);
        let (mut l, mut r) = ([0.0_f32; BLK], [0.0_f32; BLK]);
        for _ in 0..40 {
            block_peak(&mut e, &mut l, &mut r);
        }
        e.note_off(60);
        // Run well past release so the stack settles and the skip engages.
        let mut settled = false;
        for _ in 0..4000 {
            if block_peak(&mut e, &mut l, &mut r) < 1e-4 {
                settled = true;
                break;
            }
        }
        assert!(settled, "stack never settled below the skip floor");

        // Re-trigger: the new note must attack from silence — the first block
        // carries no click (no large first sample from stale filter state).
        e.note_on(60, 110);
        e.process_block(&mut l, &mut r);
        assert!(
            l[0].abs() < 0.05 && r[0].abs() < 0.05,
            "click on re-trigger: first sample L={} R={}",
            l[0],
            r[0]
        );
        for i in 0..BLK {
            assert!(l[i].is_finite() && r[i].is_finite());
        }
    }
}
