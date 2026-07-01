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
use vxn2_dsp::algo::pitch_stack_component;
use vxn2_dsp::delay::StereoDelay;
use vxn2_dsp::dynamics::DynamicsBlock;
use vxn2_dsp::limiter::StereoLimiter;
use vxn2_dsp::op::RatioMode;
use vxn2_dsp::phaser::StereoPhaser;
use vxn2_dsp::filter::{OtaLadderCoeffs, OtaLadderKernel};
use vxn2_dsp::halfband::{Interpolator, Oversampler};
use vxn2_dsp::hpf::HpfKernel;
use vxn2_dsp::reverb::FdnReverb;
use vxn2_dsp::stack::{STACK_LANES, stack_tick_stereo};

use crate::alloc::{N_STACKS, PolyAlloc};
use crate::default_patch;
use crate::master::MasterState;
use crate::matrix::{
    CurveKind, DestId, LaneSourceVals, LaneSources, MatrixSlot, MatrixTable, N_CLAP_DEPTH_SLOTS,
    N_DESTS, N_PITCH_DESTS, N_SLOTS, N_SOURCES, PITCH_DESTS, PatchSources, PitchSmoother, SourceId,
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
const FILTER_DRIVE_IDX: usize = DestId::FilterDrive.idx().unwrap();
const LFO1_RATE_IDX: usize = DestId::Lfo1Rate.idx().unwrap();
const LFO2_RATE_IDX: usize = DestId::Lfo2Rate.idx().unwrap();
const STACK_DETUNE_IDX: usize = DestId::StackDetune.idx().unwrap();
const STACK_SPREAD_IDX: usize = DestId::StackSpread.idx().unwrap();
/// Accumulator column of `Op1StackPitch`; the six stack-pitch dests are
/// contiguous from here (E022 0069). `OpNStackPitch` column = base + (N-1).
const OP_STACK_PITCH_BASE_IDX: usize = DestId::Op1StackPitch.idx().unwrap();
/// Per-block one-pole factor smoothing *dynamic* stack-detune/spread changes
/// (E008 0093). Fresh notes snap (immediate, zipper-free for static sources
/// like key/velocity); only block-to-block motion from a moving source
/// (mod-env, LFO) is ramped, keeping the re-cooked detune zipper-free at
/// musical rates. ~0.5 ⇒ converges within a few blocks (~ms).
const STACK_MACRO_SMOOTH: f32 = 0.5;
/// Row of `Lfo2Phase` inside the per-stack `PitchSmoother` (the smoother's
/// row order is `PITCH_DESTS`: `[GlobalPitch, Lfo2Phase, Op1..Op6]`). The
/// matrix `*→lfo2_phase` route (E008 0091) reads this smoother row and applies
/// it as a per-lane LFO2 phase offset.
const LFO2_PHASE_SMOOTHER_ROW: usize = 1;
const _: () = assert!(matches!(
    PITCH_DESTS[LFO2_PHASE_SMOOTHER_ROW],
    DestId::Lfo2Phase
));
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
/// played note's pitch (`note_to_hz`), i.e. the cutoff tracks the keyboard 1:1.
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
/// Equal-power crossfade time when the musical filter is toggled on/off. The
/// filter-enable edge swaps two render bodies whose dry buses differ by the
/// resampler group delay (OS path) *and* by the saturator's level/timbre step
/// — a hard switch pops on both counts. For the duration of one fade the engine
/// renders the dry bus *both* ways (raw sum vs filtered) from a single stack
/// tick and equal-power blends them, so neither discontinuity lands as a click.
/// ~8 ms is long enough to bury the ~0.6 ms OS group-delay shift and the level
/// step, short enough to feel instant when the user clicks the toggle.
const FILTER_XFADE_MS: f32 = 8.0;
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

/// Per-stack ramp bookkeeping (ticket 0119). One struct per slot, replacing the
/// five lockstep parallel `Vec`s the engine used to carry (`level_mod_inc`,
/// `pan_l_inc`, `pan_r_inc`, `phase_mod_inc`, `prev_eg_level`) — adding a new
/// ramp type now touches only this struct, not five parallel index sites.
///
/// `level_mod` / `pan_l` / `pan_r` are per-sample f32 increments; `phase_mod` is
/// a signed Q32 increment (wrapping, so the cyclic phase ramp is exact). All
/// four glide their `stack.*` field linearly across the block while the slot's
/// `ramp_live` flag is set. `prev_eg` is the EG level the previous block's ramp
/// targeted, used to rebase the level ramp across the EG's block-edge march
/// (0077). `STACK_LANES` (8) and `N_OPS` (6) are both ≤ 32, so `derive(Default)`
/// (and `Clone`, for `vec![_; N_STACKS]`) cover the array fields.
#[derive(Clone, Default)]
struct RampState {
    level_mod: [[f32; STACK_LANES]; vxn2_dsp::algo::N_OPS],
    pan_l: [[f32; STACK_LANES]; vxn2_dsp::algo::N_OPS],
    pan_r: [[f32; STACK_LANES]; vxn2_dsp::algo::N_OPS],
    phase_mod: [[i32; STACK_LANES]; vxn2_dsp::algo::N_OPS],
    prev_eg: [f32; vxn2_dsp::algo::N_OPS],
}

/// Block-rate outputs of [`Engine::cook_stacks_block`] that
/// [`Engine::process_block`] consumes after the per-stack loop: the FX-mix
/// accumulators (averaged at lane 0 across active stacks), the patch-global
/// LFO1-rate octave accumulator (cached for next block, one-block latency), and
/// the active-stack count used as the averaging divisor. Per-stack ramp state is
/// written straight into `self.ramps` / `self.ramp_live` inside the cook.
#[derive(Default)]
struct StackBlockSummary {
    fx_delay_mix_sum: f32,
    fx_reverb_mix_sum: f32,
    lfo1_rate_oct_sum: f32,
    fx_active: u32,
}

/// Top-level audio engine. Owns every sub-engine plus the per-block
/// parameter snapshot.
pub struct Engine {
    pub alloc: PolyAlloc,
    pub matrix: MatrixTable,
    pub patch_mod: PatchMod,
    pub cleanup: CleanupFilter,
    /// Dynamics block (comp + sat), inserted **first** in the FX bus (E028) so
    /// it evens FM transients before phaser / delay / reverb accumulate them.
    /// Bypassed bit-exactly when `dyn-on = 0`.
    pub dynamics: DynamicsBlock,
    /// Stereo phaser, inserted between dynamics and delay in the FX bus (E025).
    /// Bypassed bit-exactly when `phaser-on = 0`.
    pub phaser: StereoPhaser,
    pub delay: StereoDelay,
    pub reverb: FdnReverb,
    pub master: MasterState,
    /// Optional brickwall limiter on the master bus (last in the FX chain).
    /// Run only when `master.limiter_on` is set; bypassed otherwise (VXN1
    /// parity).
    pub limiter: StereoLimiter,
    /// Whether the limiter ran last block, so it can be reset on the off→on
    /// edge (clears stale lookahead instead of leaking an old transient).
    limiter_was_on: bool,
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
    /// Per-stack × per-lane LFO2 phase offset (fraction of a cycle) last
    /// applied by the matrix `*→lfo2_phase` route (E008 0091). Each block the
    /// engine adds the *delta* vs this value to `lfo2.phase[k]` so a static
    /// offset settles to a fixed scatter (no runaway); on a fresh note it
    /// resets to 0 (note_on already zeroed the per-lane phases) so the offset
    /// snaps in with the note rather than gliding from the previous voice.
    prev_lfo2_phase_off: [[f32; STACK_LANES]; N_STACKS],
    /// LFO1 rate offset (octaves) aggregated from the matrix `*→lfo1-rate`
    /// route at the end of the previous block, applied to this block's LFO1
    /// `eval` as `2^oct` (E008 0092, one-block latency). `lfo1-rate` is a
    /// patch-global dest; the value is averaged at lane 0 across active stacks
    /// exactly like the FX-mix dests. 0 when un-targeted → multiplier 1.0.
    lfo1_rate_oct: f32,
    /// Per-stack smoothed `stack-detune` / `stack-spread` modulation amounts
    /// (E008 0093). Detune is applied this block (folded into
    /// `apply_pitch_mult`); spread feeds *next* block's `VoiceSpread` source
    /// scaling (one-block latency, since the source is evaluated before the
    /// matrix). Both snap on a fresh note and one-pole toward the target
    /// otherwise; 0 when un-targeted → bit-identical off-path.
    stack_detune_mod: [f32; N_STACKS],
    stack_spread_mod: [f32; N_STACKS],
    /// Per-stack ramp state (one [`RampState`] per slot): the per-sample
    /// level / pan / phase increments that glide `stack.op_level_mod`,
    /// `stack.pan_l` / `pan_r`, and `stack.op_phase_mod_q32` to each block's
    /// matrix targets (tickets 0074 / E023), plus `prev_eg` — the previous
    /// block's EG level, used to rebase the level ramp across the EG's
    /// block-edge march (0077). Collapsed from five lockstep parallel `Vec`s
    /// into one struct so adding a ramp type touches only [`RampState`]
    /// (ticket 0119). Engine-owned so the `stack_tick_*` hot path stays
    /// untouched; the render loop advances them once per sample while live.
    ramps: Vec<RampState>,
    /// Which slots carry a live ramp this block; `any_ramp_live` is the
    /// whole-engine OR so a patch with static effective levels pays one
    /// branch per sample.
    ramp_live: [bool; N_STACKS],
    any_ramp_live: bool,

    /// Stack-pitch component masks (E022 0069): `stack_pitch_masks[n]` is the
    /// 6-bit op set an `Op(n+1)StackPitch` route bends — op n+1 plus its whole
    /// ratio-coherent FM component, with fixed-freq ops walled off
    /// ([`vxn2_dsp::algo::pitch_stack_component`]). Pure function of
    /// `(algo, wall_mask)`; recomputed only when that key changes (gated by
    /// `stack_pitch_key`), so a ratio *value* tweak — same key — never
    /// re-resolves. Masks are 6 bytes; reused across every lane/sample.
    stack_pitch_masks: [u8; vxn2_dsp::algo::N_OPS],
    /// Cache key `(algo, wall_mask)` guarding `stack_pitch_masks`. The
    /// `(u8::MAX, u8::MAX)` sentinel forces a first-cook recompute.
    stack_pitch_key: (u8, u8),

    // ── Optional per-voice filter (E007 / ADR 0004) ──────────────────────
    // Two scalar OTA-C ladder kernels per stack (L/R) — the filter runs on a
    // stack's summed stereo pair. Plus one interpolating resampler per stack
    // (per-voice upsample, stateful) and a single shared decimator per channel
    // (deferred decimation past the voice-sum). All allocated once; untouched
    // while `filter-enable` is off.
    filter_l: Vec<OtaLadderKernel>,
    filter_r: Vec<OtaLadderKernel>,
    // ── Static high-pass stage (v13) ─────────────────────────────────────
    // Two scalar one-pole HP kernels per stack (L/R), run on each stack's
    // summed stereo pair at base rate (never oversampled) *ahead* of the
    // musical filter. Global cutoff (not a mod dest), so the coefficient is
    // computed once per block and broadcast. Bypassed (untouched) while the
    // cutoff sits at its 20 Hz floor — the default — so the common case pays
    // nothing.
    hp_l: Vec<HpfKernel>,
    hp_r: Vec<HpfKernel>,
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
    // Block-rate smoothing of the *base* filter scalars (the UI cutoff /
    // resonance / drive knobs), to kill zipper noise when they're swept. The
    // per-stack matrix mod accumulators are already per-sample-ramped; only the
    // global base param jumped at block boundaries. Cutoff and drive smooth in
    // the log2 domain (musically linear, matching their exponential taper);
    // resonance is linear. `filter_smooth_primed` snaps the glide to its target
    // on the first filtered block after a reset or a filter-disable, so toggling
    // the filter on never sweeps up from a stale value.
    filter_cutoff_log2: f32,
    filter_reso: f32,
    filter_drive_log2: f32,
    filter_smooth_primed: bool,
    /// `filter-enable` as of last block, so [`Self::process_block`] can spot the
    /// toggle edge and arm the declick crossfade. Seeded from the live param so
    /// a patch that boots with the filter already on doesn't fade in from dry.
    filter_was_enabled: bool,
    /// Equal-power crossfade window length in samples (`FILTER_XFADE_MS`).
    filter_xfade_len: usize,
    /// Samples left in the active filter-toggle crossfade; 0 ⇒ steady state, a
    /// single render body runs. While > 0 the dual-render xfade body runs and
    /// decrements this by the block length until the fade completes.
    filter_xfade_remaining: usize,
    /// OFF-path (raw, unfiltered) dry bus, rendered alongside the filtered
    /// `dry_l/r` only during a toggle crossfade (`block_size`). Untouched in
    /// steady state.
    dry_alt_l: Vec<f32>,
    dry_alt_r: Vec<f32>,
}

impl Engine {
    pub fn new(sample_rate: f32, block_size: usize) -> Self {
        let mut e = Self {
            alloc: PolyAlloc::new(sample_rate),
            matrix: default_patch::default_matrix(),
            patch_mod: PatchMod::new(0xDEAD_BEEF_DEAD_BEEF),
            cleanup: CleanupFilter::new(sample_rate),
            dynamics: DynamicsBlock::new(sample_rate),
            phaser: StereoPhaser::new(sample_rate),
            delay: StereoDelay::new(sample_rate),
            reverb: FdnReverb::new(sample_rate),
            master: MasterState::new(sample_rate),
            limiter: StereoLimiter::new(sample_rate),
            limiter_was_on: false,
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
            prev_lfo2_phase_off: [[0.0; STACK_LANES]; N_STACKS],
            lfo1_rate_oct: 0.0,
            stack_detune_mod: [0.0; N_STACKS],
            stack_spread_mod: [0.0; N_STACKS],
            ramps: vec![RampState::default(); N_STACKS],
            ramp_live: [false; N_STACKS],
            any_ramp_live: false,
            stack_pitch_masks: [0; vxn2_dsp::algo::N_OPS],
            stack_pitch_key: (u8::MAX, u8::MAX),
            filter_l: vec![OtaLadderKernel::new(); N_STACKS],
            filter_r: vec![OtaLadderKernel::new(); N_STACKS],
            hp_l: vec![HpfKernel::new(); N_STACKS],
            hp_r: vec![HpfKernel::new(); N_STACKS],
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
            filter_cutoff_log2: 0.0,
            filter_reso: 0.0,
            filter_drive_log2: 0.0,
            filter_smooth_primed: false,
            filter_was_enabled: EngineParams::default().filter.enable,
            filter_xfade_len: (FILTER_XFADE_MS * 0.001 * sample_rate) as usize,
            filter_xfade_remaining: 0,
            dry_alt_l: vec![0.0; block_size],
            dry_alt_r: vec![0.0; block_size],
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
        self.alloc.clear();
        self.cleanup.reset();
        self.dynamics.clear();
        self.phaser.clear();
        self.delay.reset();
        self.reverb.reset();
        // Master gain snaps to the current param at the end of reset (see the
        // snap below, after apply_block_params re-pushes the target) so a
        // transport restart doesn't glide the level in from a stale value.
        self.limiter.reset();
        self.limiter_was_on = false;
        // Re-prime the base-filter smoothers: the first filtered block after a
        // reset snaps to the live param instead of gliding from a stale value.
        self.filter_smooth_primed = false;
        self.patch_mod.on_transport_restart();
        // Drop any matrix LFO-rate modulation (E008 0092); a fresh patch
        // re-derives it from the matrix accumulator.
        self.patch_mod.lfo1.rate_mult = 1.0;
        self.lfo1_rate_oct = 0.0;
        // Zero the pitch smoothers — a voice played after reset must not
        // glide in from pre-reset modulation state.
        let zero = [[0.0; STACK_LANES]; N_PITCH_DESTS];
        for i in 0..N_STACKS {
            self.pitch_smoothers[i].snap_to(&zero);
            self.pitch_targets[i] = zero;
            self.mod_seq[i] = u64::MAX;
            self.prev_lfo2_phase_off[i] = [0.0; STACK_LANES];
            self.stack_detune_mod[i] = 0.0;
            self.stack_spread_mod[i] = 0.0;
            self.ramp_live[i] = false;
            self.ramps[i].prev_eg = [0.0; vxn2_dsp::algo::N_OPS];
            self.filter_l[i].reset();
            self.filter_r[i].reset();
            self.hp_l[i].reset();
            self.hp_r[i].reset();
            self.interp_l[i].reset();
            self.interp_r[i].reset();
        }
        self.decim_l.reset();
        self.decim_r.reset();
        self.any_ramp_live = false;
        // Abort any in-flight filter-toggle crossfade and reseed the edge
        // tracker, so the first block after reset renders the current enable
        // state outright rather than fading in from a stale dry bus.
        self.filter_xfade_remaining = 0;
        self.filter_was_enabled = self.params.filter.enable;
        self.apply_block_params();
        // apply_block_params re-pushed the master gain target; snap to it so
        // post-reset playback starts at the correct level with no glide.
        self.master.snap(&self.params.master);
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
        self.phaser.set_from(&self.params.phaser);
        self.dynamics.set_from(&self.params.dynamics);
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
        // Apply the live assign mode before the block's note events so a
        // runtime Poly → Solo flip gates the held chord off instead of
        // leaving it sustaining under the monophonic allocator.
        self.alloc.set_mode(self.params.alloc.assign_mode);
        // Live-swap each stack's algorithm + patch-level feedback so a
        // picker change or feedback-fader move repatches a held note on the
        // next block (route_fn + fb_scale are otherwise only refreshed by
        // note_on).
        let voice = &self.params.patch.voice;
        for i in 0..self.alloc.stacks.len() {
            self.alloc.stacks[i].set_algo_live(voice.algo);
            self.alloc.stacks[i].set_feedback_live(voice.feedback);
        }
        // Re-resolve the stack-pitch component masks if the algorithm or any
        // op's Ratio/Fixed mode changed (E022 0069). Folded into the same cook
        // that live-swaps the algo above; gated on the `(algo, wall_mask)` key
        // so a ratio-*value* tweak (mode unchanged) leaves the masks alone.
        self.recompute_stack_pitch_masks();
    }

    /// Rebuild [`Self::stack_pitch_masks`] from the current algorithm and the
    /// per-op Ratio/Fixed modes, but only when the `(algo, wall_mask)` key
    /// actually changed. A wall bit marks a fixed-frequency op: it does not
    /// track key, so it is excluded from every component and severs traversal
    /// ([`pitch_stack_component`]). Pure integer work — safe in the cook.
    fn recompute_stack_pitch_masks(&mut self) {
        let voice = &self.params.patch.voice;
        let algo = voice.algo;
        let mut wall_mask = 0u8;
        for (op, p) in voice.ops.iter().enumerate() {
            if p.ratio_mode == RatioMode::Fixed {
                wall_mask |= 1 << op;
            }
        }
        let key = (algo, wall_mask);
        if key == self.stack_pitch_key {
            return;
        }
        self.stack_pitch_key = key;
        for n in 0..vxn2_dsp::algo::N_OPS {
            self.stack_pitch_masks[n] = pitch_stack_component(algo, wall_mask, (n + 1) as u8);
        }
    }

    /// True if any active matrix slot drives one of the six stack-pitch dests.
    /// Block-rate gate (mirrors [`Self::dest_targeted`]) so the un-targeted
    /// scatter is skipped entirely and the off-path stays bit-identical.
    #[inline]
    fn stack_pitch_targeted(&self) -> bool {
        self.matrix.slots.iter().any(|s| {
            matches!(
                s.dest,
                DestId::Op1StackPitch
                    | DestId::Op2StackPitch
                    | DestId::Op3StackPitch
                    | DestId::Op4StackPitch
                    | DestId::Op5StackPitch
                    | DestId::Op6StackPitch
            ) && s.source != SourceId::None
                && s.depth != 0.0
        })
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

    /// True if any active matrix slot drives `dest` (source set + nonzero
    /// depth). Block-rate gate for the deferred rate/re-cook dests (E008) so an
    /// un-targeted dest pays no extra math and the LFO tick stays bit-identical.
    #[inline]
    fn dest_targeted(&self, dest: DestId) -> bool {
        self.matrix
            .slots
            .iter()
            .any(|s| s.dest == dest && s.source != SourceId::None && s.depth != 0.0)
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

        // LFO-rate matrix routes (E008 0092), gated by a block-rate scan so
        // the un-targeted path is free + bit-identical. LFO1 rate is
        // patch-global: apply last block's aggregated octave offset as `2^oct`
        // *before* `eval_block` (one-block latency sidesteps rate-on-self).
        let lfo1_rate_targeted = self.dest_targeted(DestId::Lfo1Rate);
        let lfo2_rate_targeted = self.dest_targeted(DestId::Lfo2Rate);
        self.patch_mod.lfo1.rate_mult = if lfo1_rate_targeted {
            self.lfo1_rate_oct.exp2()
        } else {
            1.0
        };

        // Stack-macro routes (E008 0093), gated so the un-targeted path skips
        // the re-cook and stays bit-identical. Detune is applied this block;
        // spread feeds next block's VoiceSpread source (one-block latency).
        let stack_detune_targeted = self.dest_targeted(DestId::StackDetune);
        let stack_spread_targeted = self.dest_targeted(DestId::StackSpread);
        // Stack-pitch scatter gate (E022 0069): skip the whole scatter when no
        // route targets a stack-pitch dest, keeping the off-path bit-identical.
        let stack_pitch_targeted = self.stack_pitch_targeted();

        let mb = self
            .patch_mod
            .eval_block(&self.params.mod_params, self.tempo_bpm, dt);
        for s in &mut self.alloc.stacks {
            s.eg_tick(dt);
        }

        let patch_sources = PatchSources::from_modblock(&mb, self.mod_wheel, self.aftertouch);
        let StackBlockSummary {
            fx_delay_mix_sum,
            fx_reverb_mix_sum,
            lfo1_rate_oct_sum,
            fx_active,
        } = self.cook_stacks_block(
            n,
            dt,
            filter_enabled,
            lfo2_rate_targeted,
            stack_detune_targeted,
            stack_spread_targeted,
            stack_pitch_targeted,
            &patch_sources,
        );

        // Cache the LFO1-rate octave offset for next block. Reset to 0 when no
        // slot targets it or no stack is active, so a released note + held
        // mod-wheel doesn't leave a stale rate.
        self.lfo1_rate_oct = if lfo1_rate_targeted && fx_active > 0 {
            lfo1_rate_oct_sum / fx_active as f32
        } else {
            0.0
        };

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

        // Static high-pass stage (v13): a global cutoff, so one coefficient is
        // computed at the base sample rate (the HP is deliberately *not*
        // oversampled) and broadcast to every stack's L/R kernel. Bypassed
        // entirely while the cutoff sits at its 20 Hz floor (the default) — the
        // common case touches nothing. Applied per stack ahead of the musical
        // filter in both render bodies.
        let hp_active = self.params.hp.active();
        if hp_active {
            let cutoff = self.params.hp.cutoff_hz;
            for i in 0..N_STACKS {
                self.hp_l[i].set_cutoff(cutoff, self.sample_rate);
                self.hp_r[i].set_cutoff(cutoff, self.sample_rate);
            }
        }

        // Filter-toggle edge → arm the declick crossfade (ADR 0004 §10). On the
        // off→on edge also snap the cutoff smoother and reset the filter +
        // resampler state, so the ON dry bus rings up from silence rather than
        // from whatever stale tail the kernels last held — the fade then masks
        // the resampler group-delay shift and the saturator level step.
        if filter_enabled != self.filter_was_enabled {
            self.filter_xfade_remaining = self.filter_xfade_len;
            if filter_enabled {
                self.filter_smooth_primed = false;
                for i in 0..N_STACKS {
                    self.filter_l[i].reset();
                    self.filter_r[i].reset();
                    self.interp_l[i].reset();
                    self.interp_r[i].reset();
                }
                self.decim_l.reset();
                self.decim_r.reset();
            }
            self.filter_was_enabled = filter_enabled;
        }

        if self.filter_xfade_remaining > 0 && self.filter_xfade_len > 0 {
            // Toggle in flight: render both dry buses from one stack tick and
            // equal-power blend. `filter_enabled` is the fade *target* — true ⇒
            // OFF→ON (filter rising), false ⇒ ON→OFF (filter falling).
            self.render_block_filter_xfade(out_l, out_r, n, hp_active, filter_enabled);
        } else if filter_enabled {
            // ON path — stack-major oversampled filter (ADR 0004 §3–§5).
            self.render_block_filtered(out_l, out_r, n, hp_active);
        } else {
            // Filter is off: re-prime the base-filter smoothers so re-enabling
            // snaps to the live param rather than sweeping up from the value
            // the cutoff sat at when the filter was last switched off.
            self.filter_smooth_primed = false;
            self.render_block_off(out_l, out_r, n, hp_active);
        }

        // Master limiter — last in the chain, after master gain, applied to the
        // finished block on both render paths (VXN1 parity). Clear stale
        // lookahead on the off→on edge so re-engaging can't leak an old
        // transient. Off by default → an unchanged patch is bit-identical.
        let limiter_on = self.params.master.limiter_on;
        if limiter_on {
            if !self.limiter_was_on {
                self.limiter.reset();
            }
            self.limiter
                .process_block(&mut out_l[..n], &mut out_r[..n]);
        }
        self.limiter_was_on = limiter_on;
    }


    /// Mod-matrix cook: the per-stack loop extracted from `process_block`
    /// (ticket 0119). Per active stack it ticks LFO2, fans the patch / stack /
    /// lane scalars into the lane lookup, runs `eval_dests` against the single
    /// per-patch matrix table, projects the per-op level / pan / phase / pitch
    /// destinations onto the stack, and computes this block's per-sample ramp
    /// increments. Per-stack ramp state lands in `self.ramps` / `self.ramp_live`;
    /// the block-rate FX-mix / LFO1-rate aggregates are returned in the summary
    /// for `process_block` to apply.
    ///
    /// The loop runs a strict 12-stage order — a reorder injects a one-block-
    /// latency bug invisible to most tests (the render-hash `tests/baseline.rs`
    /// is the guard). Each stage is marked `// == STAGE N: … ==` in the body:
    ///
    /// ```text
    ///  1  Idle skip          release ramp + forget EG; early-out for idle slots
    ///  2  Fresh-note detect  clears macro mods BEFORE stage 4 (one-block latency)
    ///  3  LFO2 tick          per-voice; once per block; read by stages 4 & 11
    ///  4  Source fan-out     VoiceSpread uses last block's spread -> before stage 5
    ///  5  Matrix eval        sources->dests, then stack-pitch scatter (after dests,
    ///                        before stage 7's smoother capture)
    ///  6  Target projection  per-op level/pan/phase targets from dest_vals
    ///  7  Pitch smoother     capture targets + fresh snap & filter/HP reset;
    ///                        before stage 9 (fresh path reads snapped state)
    ///  8  Level + EG rebase  multiplicative level projection; rebase BEFORE stage 9
    ///                        (the ramp interpolates the rebased level)
    ///  9  Ramp compute       fresh snap vs continuing glide for level/pan/phase
    /// 10  Feedback + detune   detune folds into the pitch sum -> before apply_pitch_mult
    /// 11  LFO2 phase + rate   deferred; applied AFTER stage 3's eval (one-block latency)
    /// 12  Spread + FX agg     next-block VoiceSpread (one-block) + lane-0 FX/LFO1 sums
    /// ```
    // Block-rate gates + the patch-source snapshot are passed in from
    // `process_block` (computed once before the loop); grouping them buys no
    // clarity over named args.
    #[allow(clippy::too_many_arguments)]
    fn cook_stacks_block(
        &mut self,
        n: usize,
        dt: f32,
        filter_enabled: bool,
        lfo2_rate_targeted: bool,
        stack_detune_targeted: bool,
        stack_spread_targeted: bool,
        stack_pitch_targeted: bool,
        patch_sources: &PatchSources,
    ) -> StackBlockSummary {
        let voice = &self.params.patch.voice;
        // Dest indices are module-level consts (`GLOBAL_PITCH_IDX` etc.);
        // layout is op-major (Pitch, Level, Pan per op — stride 3), then
        // global pitch / lfo / stack / FX, asserted at compile time.
        let patch_feedback = voice.feedback;

        let mut fx_delay_mix_sum = 0.0_f32;
        let mut fx_reverb_mix_sum = 0.0_f32;
        let mut lfo1_rate_oct_sum = 0.0_f32;
        let mut fx_active = 0u32;

        for i in 0..self.alloc.stacks.len() {
            // == STAGE 1: Idle skip — release the slot's ramp, forget its EG (early-out). ==
            if self.alloc.stacks[i].is_idle() {
                self.ramp_live[i] = false;
                // Forget the last rendered EG level so a future fresh note reusing
                // this slot rebases its onset from silence, not a stale level left
                // by a hard-silenced (declicked) voice.
                self.ramps[i].prev_eg = [0.0; vxn2_dsp::algo::N_OPS];
                continue;
            }
            // == STAGE 2: Fresh-note detection — clears macro mods before the VoiceSpread
            //          source is built (one-block latency). ==
            // Fresh-note detection up front (E008 0093 needs it before the
            // VoiceSpread source is built so the spread-mod doesn't glide in
            // from the previous voice on a reused stack). A bumped allocation
            // generation means a new note reused this slot.
            let seq = self.alloc.slot_seq(i);
            let fresh = seq != self.mod_seq[i];
            if fresh {
                self.mod_seq[i] = seq;
                self.stack_detune_mod[i] = 0.0;
                self.stack_spread_mod[i] = 0.0;
            }

            // == STAGE 3: LFO2 tick — per-voice, advanced once per block. ==
            // LFO2 is per-voice (per-stack, lane-packed). Tick it once per
            // block here — note_on initialises phase/env but nothing else
            // advanced it.
            let lfo2_lanes =
                self.alloc.stacks[i]
                    .meta.lfo2
                    .eval(&voice.lfo2, self.tempo_bpm, dt);

            // == STAGE 4: Source fan-out — stack scalars + lane inputs; VoiceSpread uses
            //          last block's smoothed spread (one-block latency), so it must
            //          precede the matrix eval below. ==
            let stack = &self.alloc.stacks[i];
            // Pitch EG → normalized [-1, 1] shape (E008 0094): divide the raw
            // semitone output by its full-scale swing (`peg_depth`) so the
            // pitch dest's ±24 st gain sets the excursion — no hidden 24×
            // re-scale of absolute semitones. peg_depth ≈ 0 ⇒ EG output is 0
            // anyway, so the source reads 0.
            let pitch_eg = {
                let depth = voice.peg_depth;
                if depth.abs() > 1e-6 {
                    stack.meta.pitch_eg.level_st / depth
                } else {
                    0.0
                }
            };
            let stack_scalars = StackScalarSources {
                pitch_eg,
                mod_env: stack.meta.mod_env.level,
                velocity: (stack.meta.velocity as f32) * (1.0 / 127.0),
                key: (stack.meta.note as f32) * (1.0 / 127.0),
            };
            // `voice_spread` is the raw symmetric lane position in [-1, +1].
            // We scale by `cached_spread` (the stack-spread macro captured at
            // note-on) before exposing it to the matrix so the spread fader
            // gates how widely matrix slots see the lanes. spread = 0 → all
            // lanes read 0 from the VoiceSpread source. The matrix `stack-spread`
            // route (E008 0093) further scales this by `(1 + spread_mod)` using
            // last block's smoothed amount (one-block latency).
            let spread_gain = stack.meta.cached_spread * (1.0 + self.stack_spread_mod[i]);
            let scaled_voice_spread = {
                let mut a = [0.0_f32; STACK_LANES];
                for k in 0..STACK_LANES {
                    a[k] = spread_gain * stack.meta.voice_spread[k];
                }
                a
            };
            let lane_inputs = LaneSources {
                lfo2: lfo2_lanes,
                voice_idx: {
                    let mut a = [0.0_f32; STACK_LANES];
                    let denom = (STACK_LANES - 1) as f32;
                    for k in 0..STACK_LANES {
                        a[k] = stack.meta.voice_idx[k] as f32 / denom;
                    }
                    a
                },
                voice_spread: scaled_voice_spread,
                voice_rand: stack.meta.voice_rand,
            };
            // == STAGE 5: Matrix eval (sources -> dests) + stack-pitch scatter. Scatter
            //          must run after eval_dests and before the smoother capture. ==
            eval_sources(
                patch_sources,
                &stack_scalars,
                &lane_inputs,
                &mut self.lane_sources,
            );
            eval_dests(
                &self.matrix,
                &self.lane_sources,
                &mut self.dest_vals[i],
            );
            // Fan each stack-pitch accumulator across its ratio-coherent
            // component into the per-op pitch columns (E022 0069), before the
            // smoother captures pitch targets below. Gated so the common
            // (no stack-pitch route) path is untouched.
            if stack_pitch_targeted {
                scatter_stack_pitch(&mut self.dest_vals[i], &self.stack_pitch_masks);
            }

            // == STAGE 6: Per-op level/pan/phase target projection from dest_vals. ==
            // Project per-op level + pan destinations into the stack.
            // Indices: OpiLevel=i*3+1, OpiPan=i*3+2. Neither applies as a
            // block constant: level ramps linearly to this block's target
            // via per-sample increments, and pan ramps the folded equal-
            // power gains the same way (ticket 0074). Pitch-shaped
            // destinations ride the per-stack PitchSmoother instead (0063).
            let mut level_targets = [[0.0_f32; STACK_LANES]; vxn2_dsp::algo::N_OPS];
            // E023 phase dests: read the per-op phase offset (cycles) and fold to
            // a Q32 target. The dests are appended after the contiguous
            // pitch/level/pan block, so they index off `Op1Phase`, not `op_i*3`.
            let mut phase_targets_q32 = [[0_u32; STACK_LANES]; vxn2_dsp::algo::N_OPS];
            let phase_base = DestId::Op1Phase.idx().unwrap();
            let stack = &mut self.alloc.stacks[i];
            for op_i in 0..vxn2_dsp::algo::N_OPS {
                let level_idx = op_i * 3 + 1;
                let pan_idx = op_i * 3 + 2;
                let phase_idx = phase_base + op_i;
                for k in 0..STACK_LANES {
                    level_targets[op_i][k] = self.dest_vals[i][k][level_idx];
                    stack.modulation.op_pan_mod[op_i][k] = self.dest_vals[i][k][pan_idx];
                    // Wrap the (possibly multi-route) cycle offset into [0,1) and
                    // scale to Q32; `as u32` wraps cleanly at the cycle boundary.
                    let pcyc = self.dest_vals[i][k][phase_idx].rem_euclid(1.0);
                    phase_targets_q32[op_i][k] = (pcyc * vxn2_dsp::op::PM_SCALE_Q32) as u32;
                }
            }
            // == STAGE 7: Pitch-smoother target capture + fresh-note snap & filter/HP
            //          reset. Must precede the ramp compute (STAGE 9 reads snapped
            //          state on a fresh note). ==
            // Capture this block's pitch-dest targets. A slot whose
            // allocation generation changed since the last block carries a
            // fresh note — snap every smoothing/ramp state (pitch smoother,
            // level + pan ramps) so the new voice doesn't glide in from the
            // previous voice's modulation.
            self.pitch_targets[i] = self.pitch_smoothers[i].targets_from(&self.dest_vals[i]);
            if fresh {
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
                // HP stage is independent of the musical filter — clear it on a
                // fresh note too so the new voice doesn't inherit the previous
                // one's one-pole state. Inert (cheap) while the HP is bypassed.
                self.hp_l[i].reset();
                self.hp_r[i].reset();
            }
            let stack = &mut self.alloc.stacks[i];
            project_pitch_state(stack, self.pitch_smoothers[i].current());
            // == STAGE 8: Level multiplicative projection + EG block-edge rebase. Rebase
            //          must run before STAGE 9 — the ramp interpolates the rebased
            //          effective level. ==
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
            let prev_eg = &mut self.ramps[i].prev_eg;
            for op_i in 0..vxn2_dsp::algo::N_OPS {
                let eg = stack.core.ops[op_i].eg.level;
                for k in 0..STACK_LANES {
                    let eff = (eg * (1.0 + level_targets[op_i][k])).clamp(0.0, 1.0);
                    level_targets[op_i][k] = eff - eg;
                    // Keep the rendered level (`eg + op_level_mod`) continuous
                    // across the EG's block-edge march; the delta is folded into
                    // this block's ramp instead of stepping. This runs for the
                    // note's FIRST block too (not gated on `!fresh`) so the onset
                    // ramp starts from the slot's *previous* rendered level:
                    //   - idle/poly slot: a released voice settles `op_level_mod`
                    //     and `eg` to ~0, so a brand-new note ramps up from
                    //     silence (the fast-attack onset-click fix);
                    //   - solo steal: the op-amp EG continues its level across
                    //     `note_on` (click-free retrigger, see `eg::note_on`), and
                    //     the outgoing note's `op_level_mod` is still live, so the
                    //     new note ramps from the previous note's level instead of
                    //     dipping to 0 first — which itself was a click.
                    stack.core.op_level_mod[op_i][k] += prev_eg[op_i] - eg;
                }
                prev_eg[op_i] = eg;
            }
            // == STAGE 9: Ramp-increment compute — fresh snap vs continuing glide for the
            //          level/pan/phase ramps. ==
            if fresh {
                // Phase + pan snap to the new voice's values (no glide in from
                // the previous allocation). The level onset, by contrast, ramps
                // from the rebased previous rendered level (above) to this
                // block's effective target — so a fresh poly note fades in from
                // silence and a solo steal glides from the outgoing note's
                // level, neither stepping at sample 0 (onset click). Same
                // per-sample ramp the matrix mod / EG rebase use on later
                // blocks (0077).
                stack.core.op_phase_mod_q32 = phase_targets_q32;
                stack.refresh_pan_with_mod();
                let inv = 1.0 / n as f32;
                let r = &mut self.ramps[i];
                let mut any = false;
                for op_i in 0..vxn2_dsp::algo::N_OPS {
                    for k in 0..STACK_LANES {
                        let mut dl =
                            (level_targets[op_i][k] - stack.core.op_level_mod[op_i][k]) * inv;
                        if dl.abs() < RAMP_SNAP_EPS {
                            stack.core.op_level_mod[op_i][k] = level_targets[op_i][k];
                            dl = 0.0;
                        }
                        r.level_mod[op_i][k] = dl;
                        r.pan_l[op_i][k] = 0.0;
                        r.pan_r[op_i][k] = 0.0;
                        r.phase_mod[op_i][k] = 0;
                        any |= dl != 0.0;
                    }
                }
                self.ramp_live[i] = any;
            } else {
                let inv = 1.0 / n as f32;
                let (pan_l_t, pan_r_t) = stack.pan_targets();
                let mut any = false;
                let r = &mut self.ramps[i];
                for op_i in 0..vxn2_dsp::algo::N_OPS {
                    for k in 0..STACK_LANES {
                        // A ramp lands within f32 rounding of its target, so
                        // a settled value never compares exactly equal — snap
                        // inside RAMP_SNAP_EPS (≈ −120 dB) so a static sound
                        // releases the per-sample advance.
                        let mut dl =
                            (level_targets[op_i][k] - stack.core.op_level_mod[op_i][k]) * inv;
                        if dl.abs() < RAMP_SNAP_EPS {
                            stack.core.op_level_mod[op_i][k] = level_targets[op_i][k];
                            dl = 0.0;
                        }
                        let mut pl = (pan_l_t[op_i][k] - stack.core.pan_l[op_i][k]) * inv;
                        if pl.abs() < RAMP_SNAP_EPS {
                            stack.core.pan_l[op_i][k] = pan_l_t[op_i][k];
                            pl = 0.0;
                        }
                        let mut pr = (pan_r_t[op_i][k] - stack.core.pan_r[op_i][k]) * inv;
                        if pr.abs() < RAMP_SNAP_EPS {
                            stack.core.pan_r[op_i][k] = pan_r_t[op_i][k];
                            pr = 0.0;
                        }
                        // Phase ramp (E023): shortest-arc Q32 delta to the new
                        // target, split linearly across the block. `wrapping_sub
                        // as i32` picks the ≤ half-cycle direction so a small
                        // matrix move never wraps the long way round. A whole
                        // block of `delta / n` steps lands within `n` LSB of the
                        // target; the next block re-derives from the current
                        // value, so residue self-corrects (no drift).
                        let delta =
                            phase_targets_q32[op_i][k].wrapping_sub(stack.core.op_phase_mod_q32[op_i][k])
                                as i32;
                        let pq = delta / n as i32;
                        if pq == 0 {
                            stack.core.op_phase_mod_q32[op_i][k] = phase_targets_q32[op_i][k];
                        }
                        r.level_mod[op_i][k] = dl;
                        r.pan_l[op_i][k] = pl;
                        r.pan_r[op_i][k] = pr;
                        r.phase_mod[op_i][k] = pq;
                        any |= dl != 0.0 || pl != 0.0 || pr != 0.0 || pq != 0;
                    }
                }
                self.ramp_live[i] = any;
            }
            // == STAGE 10: Feedback mod + stack-detune apply + apply_pitch_mult (detune is
            //           folded into the pitch sum, so it must precede apply_pitch_mult). ==
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
            // Stack-detune (E008 0093): re-derive the per-lane detune offset
            // from this block's lane-0 accumulator and fold it into the pitch
            // sum below. Snap on a fresh note (static sources like key/velocity
            // land immediately, zipper-free); one-pole the block-to-block
            // motion of a dynamic source otherwise. Gated: when un-targeted,
            // zero the offset once so the pitch path stays bit-identical.
            if stack_detune_targeted {
                let target = self.dest_vals[i][0][STACK_DETUNE_IDX];
                self.stack_detune_mod[i] = if fresh {
                    target
                } else {
                    self.stack_detune_mod[i]
                        + STACK_MACRO_SMOOTH * (target - self.stack_detune_mod[i])
                };
                stack.set_detune_mod(self.stack_detune_mod[i]);
            } else if self.stack_detune_mod[i] != 0.0 {
                self.stack_detune_mod[i] = 0.0;
                stack.set_detune_mod(0.0);
            }

            // Refresh pitch from the new offsets so the per-sample loop
            // reads phase_inc that includes this block's matrix output.
            // Cost: per active stack 6×8 powf — affordable at ≤16 stacks.
            // (Pan gains are handled by the ramp above — ticket 0074.)
            stack.apply_pitch_mult();

            // == STAGE 11: LFO2 phase offset + LFO2 rate — deferred, one-block latency
            //           (applied after this block's lfo2.eval in STAGE 3). ==
            // LFO2 phase offset (E008 0091, `*→lfo2_phase`). The smoothed
            // per-lane Lfo2Phase value rides the same PitchSmoother as the
            // pitch dests (it is `is_pitch_shaped`); read its row and apply
            // the *delta* vs last block as a wrapping Q32 add to each lane's
            // LFO2 phase. Delta-not-absolute so a static offset settles to a
            // fixed scatter instead of running away. This lands *after* this
            // block's `lfo2.eval` (top of loop), so it takes effect on the
            // next block — a one-block latency, consistent with the other
            // deferred dests and inaudible at musical rates (LFO2's note-on
            // delay/fade covers the onset). The guard keeps the off-path
            // bit-identical: with no `lfo2-phase` slot the smoother row stays
            // 0, every delta is 0, and `lfo2.phase` is never touched.
            let lfo2_off_tgt = self.pitch_smoothers[i].current()[LFO2_PHASE_SMOOTHER_ROW];
            let prev = &mut self.prev_lfo2_phase_off[i];
            if fresh {
                // note_on reset every lane's phase to the shape zero-crossing
                // (no offset baked in); drop tracking to 0 so the full target
                // offset snaps onto the fresh phase rather than gliding from
                // the previous voice on the reused stack.
                *prev = [0.0; STACK_LANES];
            }
            let lfo2 = &mut self.alloc.stacks[i].meta.lfo2;
            for k in 0..STACK_LANES {
                let target = lfo2_off_tgt[k];
                let delta = target - prev[k];
                if delta != 0.0 {
                    lfo2.add_phase_offset(k, delta);
                }
                prev[k] = target;
            }
            // LFO2 per-stack rate (E008 0092). `lfo2-rate` is a per-stack dest;
            // read this block's lane-0 accumulator (in octaves) and stash the
            // multiplier for *next* block's `eval` (one-block latency). Gated:
            // an un-targeted stack keeps `rate_mult = 1.0` (bit-identical tick).
            lfo2.rate_mult = if lfo2_rate_targeted {
                self.dest_vals[i][0][LFO2_RATE_IDX].exp2()
            } else {
                1.0
            };

            // == STAGE 12: Stack-spread update (next block's VoiceSpread, one-block latency)
            //           + FX-mix / LFO1-rate aggregation at lane 0. ==
            // Stack-spread (E008 0093): update the per-stack smoothed amount
            // for *next* block's VoiceSpread source scaling (one-block latency
            // — the source is built before the matrix eval). Snap on fresh,
            // one-pole otherwise; zero when un-targeted.
            if stack_spread_targeted {
                let target = self.dest_vals[i][0][STACK_SPREAD_IDX];
                self.stack_spread_mod[i] = if fresh {
                    target
                } else {
                    self.stack_spread_mod[i]
                        + STACK_MACRO_SMOOTH * (target - self.stack_spread_mod[i])
                };
            } else {
                self.stack_spread_mod[i] = 0.0;
            }

            // FX dests aggregate at lane 0 across active stacks. Lane 0
            // sees patch-source contributions exactly once; per-stack
            // sources (velocity, mod env, …) average naturally across the
            // active stacks below.
            fx_delay_mix_sum += self.dest_vals[i][0][DELAY_MIX_IDX];
            fx_reverb_mix_sum += self.dest_vals[i][0][REVERB_MIX_IDX];
            // LFO1 rate is patch-global — aggregate at lane 0 like the FX mixes
            // and cache for next block (E008 0092, one-block latency).
            lfo1_rate_oct_sum += self.dest_vals[i][0][LFO1_RATE_IDX];
            fx_active += 1;
        }

        StackBlockSummary {
            fx_delay_mix_sum,
            fx_reverb_mix_sum,
            lfo1_rate_oct_sum,
            fx_active,
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
    fn render_block_filtered(
        &mut self,
        out_l: &mut [f32],
        out_r: &mut [f32],
        n: usize,
        hp_active: bool,
    ) {
        let mut fp = self.params.filter;
        // Block-rate smoothing of the base cutoff / resonance / drive knobs.
        // Cooking ladder coeffs is deliberately block-rate (ADR 0004 §7), so a
        // raw UI sweep steps the coefficients once per block — audible zipper.
        // A one-pole glide (~30 ms) spreads each jump across many blocks so the
        // per-block step is inaudible, while the steady state still lands
        // exactly on the param. Cutoff / drive glide in log2 (musically linear);
        // resonance linear. First block after reset / filter-off snaps.
        const FILTER_SMOOTH_MS: f32 = 30.0;
        let target_cutoff_log2 = fp.cutoff_hz.max(1.0).log2();
        let target_reso = fp.resonance;
        let target_drive_log2 = fp.drive.max(1.0e-4).log2();
        if self.filter_smooth_primed {
            let c = 1.0 - (-(n as f32) / (FILTER_SMOOTH_MS * 0.001 * self.sample_rate)).exp();
            self.filter_cutoff_log2 += c * (target_cutoff_log2 - self.filter_cutoff_log2);
            self.filter_reso += c * (target_reso - self.filter_reso);
            self.filter_drive_log2 += c * (target_drive_log2 - self.filter_drive_log2);
        } else {
            self.filter_cutoff_log2 = target_cutoff_log2;
            self.filter_reso = target_reso;
            self.filter_drive_log2 = target_drive_log2;
            self.filter_smooth_primed = true;
        }
        fp.cutoff_hz = self.filter_cutoff_log2.exp2();
        fp.resonance = self.filter_reso;
        fp.drive = self.filter_drive_log2.exp2();
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
                    let (mut sl, mut sr) = stack_tick_stereo(&mut self.alloc.stacks[i]);
                    // HP ahead of the musical filter, at base rate (the ladder
                    // runs at base rate here too — this is the 1× path).
                    if hp_active {
                        sl = self.hp_l[i].tick(sl);
                        sr = self.hp_r[i].tick(sr);
                    }
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
                    // HP runs on the base-rate scratch *before* the upsample, so
                    // the high-pass itself is never oversampled (its design
                    // intent — a static one-pole at the host rate). Idle stacks
                    // feed silence and don't tick the HP, matching the OFF path.
                    if hp_active {
                        for sample in 0..n {
                            self.base_l[sample] = self.hp_l[i].tick(self.base_l[sample]);
                            self.base_r[sample] = self.hp_r[i].tick(self.base_r[sample]);
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

        self.apply_fx_block(n, out_l, out_r);
    }

    /// OFF-path render: the tuned sample-major sum loop. With the HP bypassed
    /// (default) this is byte-for-byte the original sum; when the HP is engaged
    /// each active stack's stereo pair is high-passed before it folds into the
    /// dry bus. Sums every active stack into `dry_l/r`, then the shared FX chain.
    /// Every `PITCH_SMOOTH_QUANTUM` samples the pitch smoothers advance one step
    /// toward this block's targets and the affected stacks re-cook `phase_inc` —
    /// converged smoothers (no active pitch route) skip the recook entirely.
    fn render_block_off(&mut self, out_l: &mut [f32], out_r: &mut [f32], n: usize, hp_active: bool) {
        for sample in 0..n {
            if sample % PITCH_SMOOTH_QUANTUM == 0 {
                self.advance_pitch_smoothers();
            }
            let mut dry_l = 0.0_f32;
            let mut dry_r = 0.0_f32;
            for i in 0..N_STACKS {
                if self.alloc.stacks[i].is_idle() {
                    continue;
                }
                let (mut sl, mut sr) = stack_tick_stereo(&mut self.alloc.stacks[i]);
                if hp_active {
                    sl = self.hp_l[i].tick(sl);
                    sr = self.hp_r[i].tick(sr);
                }
                dry_l += sl;
                dry_r += sr;
            }
            if self.any_ramp_live {
                self.advance_mod_ramps();
            }
            self.dry_l[sample] = dry_l;
            self.dry_r[sample] = dry_r;
        }
        self.apply_fx_block(n, out_l, out_r);
    }

    /// Shared post-dry FX chain (cleanup → dynamics → phaser → delay → reverb →
    /// master), run per sample over `dry_l/r` into `out_l/r`. Identical on
    /// every render body, so the dry bus is the only thing the three paths
    /// produce differently.
    fn apply_fx_block(&mut self, n: usize, out_l: &mut [f32], out_r: &mut [f32]) {
        for sample in 0..n {
            let (cl, cr) = self.cleanup.process(self.dry_l[sample], self.dry_r[sample]);
            let (cl, cr) = self.dynamics.process(cl, cr);
            let (cl, cr) = self.phaser.process(cl, cr);
            let (l, r) = self.delay.process(cl, cr);
            let (l, r) = self.reverb.process(l, r);
            let (l, r) = self.master.apply(l, r);
            out_l[sample] = l;
            out_r[sample] = r;
        }
    }

    /// Declick render for the filter-enable toggle (ADR 0004 §10). Renders the
    /// dry bus *both* ways from a single stack tick — the raw HP'd sum (OFF
    /// dry, into `dry_alt_l/r`) and the filtered signal at the configured
    /// oversample factor (ON dry, into `dry_l/r`) — then equal-power blends them
    /// across `filter_xfade_len` samples before the shared FX chain. Rendering
    /// both from one tick is what makes the blend valid: the two buses are the
    /// same source material, differing only by the filter (its group delay and
    /// level/timbre step), so the crossfade hides exactly the discontinuity the
    /// hard switch would expose. `to_on` is the fade *target* — true ⇒ OFF→ON
    /// (filter rising 0→1), false ⇒ ON→OFF (filter falling 1→0).
    ///
    /// Edge-only: runs for one ~8 ms window per toggle, so the per-stack double
    /// pass and the second dry buffer never touch the steady-state hot paths.
    fn render_block_filter_xfade(
        &mut self,
        out_l: &mut [f32],
        out_r: &mut [f32],
        n: usize,
        hp_active: bool,
        to_on: bool,
    ) {
        // Block-rate base-knob smoothing — identical to `render_block_filtered`,
        // so the cutoff/reso/drive the ON dry sees mid-fade tracks the steady
        // path it hands off to (no second discontinuity at fade end).
        let mut fp = self.params.filter;
        const FILTER_SMOOTH_MS: f32 = 30.0;
        let target_cutoff_log2 = fp.cutoff_hz.max(1.0).log2();
        let target_reso = fp.resonance;
        let target_drive_log2 = fp.drive.max(1.0e-4).log2();
        if self.filter_smooth_primed {
            let c = 1.0 - (-(n as f32) / (FILTER_SMOOTH_MS * 0.001 * self.sample_rate)).exp();
            self.filter_cutoff_log2 += c * (target_cutoff_log2 - self.filter_cutoff_log2);
            self.filter_reso += c * (target_reso - self.filter_reso);
            self.filter_drive_log2 += c * (target_drive_log2 - self.filter_drive_log2);
        } else {
            self.filter_cutoff_log2 = target_cutoff_log2;
            self.filter_reso = target_reso;
            self.filter_drive_log2 = target_drive_log2;
            self.filter_smooth_primed = true;
        }
        fp.cutoff_hz = self.filter_cutoff_log2.exp2();
        fp.resonance = self.filter_reso;
        fp.drive = self.filter_drive_log2.exp2();
        let f = fp.oversample.clamp(1, MAX_OVERSAMPLE);
        let osn = n * f;
        let os_rate = self.sample_rate * f as f32;

        self.dry_l[..n].fill(0.0);
        self.dry_r[..n].fill(0.0);
        self.dry_alt_l[..n].fill(0.0);
        self.dry_alt_r[..n].fill(0.0);
        if f > 1 {
            self.bus_l[..osn].fill(0.0);
            self.bus_r[..osn].fill(0.0);
        }

        for i in 0..N_STACKS {
            let idle = self.alloc.stacks[i].is_idle();
            // Idle + filter rung out ⇒ both buses get exact zero from this
            // stack; skipping is exact (mirrors the ON path's quiescence-skip).
            if idle && self.stack_filter_quiescent(i) {
                continue;
            }
            self.set_stack_filter_coeffs(i, os_rate, fp);

            // Render this stack's base block once — the single source the two
            // dry buses share. Idle-but-ringing stacks feed zeros: the ON bus
            // rings the resonant tail out through the filter, the OFF bus gets
            // nothing (matching `render_block_off`'s `is_idle` skip).
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
                // HP ahead of the filter, at base rate — the same HP'd signal
                // both buses consume (one tick of the HP kernel per sample).
                if hp_active {
                    for sample in 0..n {
                        self.base_l[sample] = self.hp_l[i].tick(self.base_l[sample]);
                        self.base_r[sample] = self.hp_r[i].tick(self.base_r[sample]);
                    }
                }
                // OFF dry: raw HP'd stack sum (exactly `render_block_off`).
                for sample in 0..n {
                    self.dry_alt_l[sample] += self.base_l[sample];
                    self.dry_alt_r[sample] += self.base_r[sample];
                }
            }

            // ON dry: filtered at the configured oversample factor (exactly
            // `render_block_filtered`'s two branches).
            if f == 1 {
                for sample in 0..n {
                    self.dry_l[sample] +=
                        self.filter_l[i].tick(self.base_l[sample] * FILTER_IN_TRIM) * FILTER_OUT_MAKEUP;
                    self.dry_r[sample] +=
                        self.filter_r[i].tick(self.base_r[sample] * FILTER_IN_TRIM) * FILTER_OUT_MAKEUP;
                }
            } else {
                self.interp_l[i].interpolate(&self.base_l[..n], &mut self.os_l[..osn], f);
                self.interp_r[i].interpolate(&self.base_r[..n], &mut self.os_r[..osn], f);
                for j in 0..osn {
                    self.bus_l[j] +=
                        self.filter_l[i].tick(self.os_l[j] * FILTER_IN_TRIM) * FILTER_OUT_MAKEUP;
                    self.bus_r[j] +=
                        self.filter_r[i].tick(self.os_r[j] * FILTER_IN_TRIM) * FILTER_OUT_MAKEUP;
                }
            }
        }
        if f > 1 {
            self.decim_l.decimate(&self.bus_l[..osn], &mut self.dry_l[..n], f);
            self.decim_r.decimate(&self.bus_r[..osn], &mut self.dry_r[..n], f);
        }

        // Raised-cosine (equal-gain) blend OFF (`dry_alt`) ↔ ON (`dry`) into
        // `dry_l/r`. The two buses are the *same* source pre/post filter —
        // strongly correlated — so equal-gain (weights sum to 1) holds the
        // amplitude without the +3 dB bump an equal-power curve would add. The
        // raised cosine matters more than the gain law here: its derivative is
        // zero at *both* endpoints, so neither the engage start nor the steady
        // handoff leaves a slope corner. An equal-power `cos` weight has slope
        // −π/2 at t=1; when it clamps to 0 at handoff — right where the ON
        // signal is full-amplitude — that corner reads as a click (the exact
        // failure this curve fixes).
        //
        // `t` spans the closed interval [0,1] across the `len`-sample window
        // (denominator `len-1`), so the last fade sample lands exactly on the
        // target before the steady body takes over; samples past the window end
        // clamp to the full target.
        let len = self.filter_xfade_len as f32;
        let start = (self.filter_xfade_len - self.filter_xfade_remaining) as f32;
        let span = (len - 1.0).max(1.0);
        for sample in 0..n {
            let t = ((start + sample as f32) / span).min(1.0);
            // Smooth 0→1 ramp with zero slope at both ends.
            let rise = 0.5 - 0.5 * (core::f32::consts::PI * t).cos();
            // `to_on`: ON weight rises 0→1. Else ON weight falls 1→0.
            let (w_off, w_on) = if to_on { (1.0 - rise, rise) } else { (rise, 1.0 - rise) };
            self.dry_l[sample] = w_off * self.dry_alt_l[sample] + w_on * self.dry_l[sample];
            self.dry_r[sample] = w_off * self.dry_alt_r[sample] + w_on * self.dry_r[sample];
        }
        self.filter_xfade_remaining = self.filter_xfade_remaining.saturating_sub(n);

        self.apply_fx_block(n, out_l, out_r);
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
        let keytrack_oct = keytrack_octaves(self.alloc.stacks[i].meta.note, fp.keytrack);
        let cutoff_oct = self.dest_vals[i][0][CUTOFF_IDX] + keytrack_oct;
        let cutoff_hz = (fp.cutoff_hz * cutoff_oct.exp2()).clamp(CUTOFF_MIN_HZ, CUTOFF_MAX_HZ);
        let resonance = (fp.resonance + self.dest_vals[i][0][RESONANCE_IDX]).clamp(0.0, 1.0);
        // Drive modulates in the log/octave domain (matrix gain 4.0 → ±4 oct),
        // matching the param's exponential taper; clamp to the [0.1, 16] range.
        let drive =
            (fp.drive * self.dest_vals[i][0][FILTER_DRIVE_IDX].exp2()).clamp(0.1, 16.0);
        let coeffs = OtaLadderCoeffs::new(cutoff_hz, os_rate, resonance, drive);
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
        let r = &self.ramps[i];
        for op_i in 0..vxn2_dsp::algo::N_OPS {
            for k in 0..STACK_LANES {
                stack.core.op_level_mod[op_i][k] += r.level_mod[op_i][k];
                stack.core.pan_l[op_i][k] += r.pan_l[op_i][k];
                stack.core.pan_r[op_i][k] += r.pan_r[op_i][k];
                stack.core.op_phase_mod_q32[op_i][k] =
                    stack.core.op_phase_mod_q32[op_i][k].wrapping_add(r.phase_mod[op_i][k] as u32);
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
            let r = &self.ramps[i];
            for op_i in 0..vxn2_dsp::algo::N_OPS {
                for k in 0..STACK_LANES {
                    stack.core.op_level_mod[op_i][k] += r.level_mod[op_i][k];
                    stack.core.pan_l[op_i][k] += r.pan_l[op_i][k];
                    stack.core.pan_r[op_i][k] += r.pan_r[op_i][k];
                    stack.core.op_phase_mod_q32[op_i][k] =
                        stack.core.op_phase_mod_q32[op_i][k].wrapping_add(r.phase_mod[op_i][k] as u32);
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

/// Scatter each `OpNStackPitch` accumulator into the per-op pitch columns of
/// every op in component N (E022 0069). The **same** semitone delta is added
/// to every member — no depth scaling — so the FM ratios within the branch are
/// preserved: pitch moves, timbre holds. Runs in the cook (per stack, per
/// block) *before* the pitch smoother reads the per-op pitch columns, so the
/// scattered value rides the existing smoothing + `project_pitch_state` with
/// no audio-inner-loop change ([[vxn2-stack-soa]] packing untouched). A zero
/// mask (walled / fixed target → empty component) is a clean no-op.
#[inline]
fn scatter_stack_pitch(
    dest_vals: &mut [[f32; N_DESTS]; STACK_LANES],
    masks: &[u8; vxn2_dsp::algo::N_OPS],
) {
    for n in 0..vxn2_dsp::algo::N_OPS {
        let mask = masks[n];
        if mask == 0 {
            continue;
        }
        let src_col = OP_STACK_PITCH_BASE_IDX + n;
        for k in 0..STACK_LANES {
            let delta = dest_vals[k][src_col];
            if delta == 0.0 {
                continue;
            }
            let mut m = mask;
            while m != 0 {
                let op = m.trailing_zeros() as usize;
                // Per-op pitch column is op-major stride 3 (Pitch, Level, Pan).
                dest_vals[k][op * 3] += delta;
                m &= m - 1;
            }
        }
    }
}

/// Copy a smoother's current pitch state into the stack's per-lane pitch-mod
/// fields. [`crate::matrix::PITCH_DESTS`] order: `[GlobalPitch, Lfo2Phase,
/// Op1Pitch .. Op6Pitch]` — `Lfo2Phase` (row 1) is *not* projected here: it
/// is consumed directly in `process_block` as a per-lane LFO2 phase offset
/// (E008 0091), so this fn skips row 1 and projects only the pitch dests.
fn project_pitch_state(
    stack: &mut vxn2_dsp::stack::Stack,
    st: &[[f32; STACK_LANES]; N_PITCH_DESTS],
) {
    for k in 0..STACK_LANES {
        stack.modulation.global_pitch_mod_st[k] = st[0][k];
    }
    for op_i in 0..vxn2_dsp::algo::N_OPS {
        for k in 0..STACK_LANES {
            stack.modulation.op_pitch_mod_st[op_i][k] = st[2 + op_i][k];
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
    /// note_to_hz(note)`. Also: centred on C0 (zero at note 12) and linear in
    /// amount.
    #[test]
    fn keytrack_full_lands_cutoff_on_note_pitch() {
        for note in [24u8, 36, 48, 60, 72, 96] {
            let cutoff = CUTOFF_MIN_HZ * keytrack_octaves(note, 1.0).exp2();
            let pitch = vxn_core_utils::note_to_hz(note as f32);
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
        // Snap the smoothed master gain to −60 dB (reset preserves params and
        // snaps) so we measure steady-state attenuation, not the anti-zipper
        // glide down from the −6 dB default.
        e2.reset();
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
        // 0125 (exponential EG ramps): the default patch is percussive — every
        // carrier's L3 = 0, so the note decays toward silence. Under the old
        // linear-amplitude march it sat on a ~-8..-24 dBFS plateau at t ≈ 1 s;
        // the DX7 exponential march descends on a constant dB/sec slope and is
        // far lower mid-tail (~-40 dBFS) for the *same* total decay duration.
        // Bound it as a decaying-but-still-ringing tail (below the attack body,
        // above the noise floor) rather than a fixed plateau — the absolute
        // level is the manual listening pass + 0126 loudness re-sweep's job.
        assert!(
            sustain_db < attack_db && sustain_db > -55.0,
            "t≈1s RMS {sustain_db} dBFS not a decaying-but-ringing tail \
             (want below attack {attack_db} dBFS and above -55)"
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
                        if s.core.op_level_mod[0][k].abs() > 1e-6 {
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

    /// Wiring sanity for the E023 per-op phase dests. A matrix slot routing
    /// LFO1 → Op1Phase should write a non-zero, ramping `op_phase_mod_q32`
    /// after a few blocks, and the render should diverge from a baseline with
    /// no phase route. (A carrier's steady magnitude spectrum is phase-deaf,
    /// but the time-domain samples shift with the offset, so the buffers
    /// differ.)
    #[test]
    fn matrix_lfo1_to_op_phase_modulates_audio() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        let mut modulated = Engine::new(SR, BLK);
        modulated.params.mod_params.lfo1.rate_hz = 5.0;
        modulated.matrix.slots[0] = MatrixSlot {
            source: SourceId::Lfo1,
            dest: DestId::Op1Phase,
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

        let blocks = (SR as usize) / 5 / BLK;
        let mut diff_sum = 0.0_f64;
        let mut found_nonzero_phase_mod = false;
        for _ in 0..blocks {
            modulated.process_block(&mut lm, &mut rm);
            baseline.process_block(&mut lb, &mut rb);
            for s in &modulated.alloc.stacks {
                if !s.is_idle() {
                    for k in 0..STACK_LANES {
                        if s.core.op_phase_mod_q32[0][k] != 0 {
                            found_nonzero_phase_mod = true;
                        }
                    }
                }
            }
            for i in 0..BLK {
                diff_sum += ((lm[i] - lb[i]).abs() + (rm[i] - rb[i]).abs()) as f64;
            }
        }
        assert!(
            found_nonzero_phase_mod,
            "matrix never populated op_phase_mod_q32 — phase-dest wiring broken"
        );
        assert!(
            diff_sum > 1e-3,
            "phase-modulated render identical to baseline (diff_sum = {diff_sum}) — phase dest not applied to audio"
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
            let eg = e.alloc.stacks[slot].core.ops[0].eg.level;
            let m = self::tests_dest_val(e, slot, k, level_idx);
            (eg * (1.0 + m)).clamp(0.0, 1.0) - eg
        };
        let mut saw_ramp = false;
        for _ in 0..30 {
            e.process_block(&mut l, &mut r);
            saw_ramp |= e.ramp_live[slot];
            for k in 0..STACK_LANES {
                let got = e.alloc.stacks[slot].core.op_level_mod[0][k];
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
        use vxn2_dsp::stack::VoicePhase;

        let mut e = Engine::new(SR, BLK);
        e.params.alloc.assign_mode = AssignMode::Solo;
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];

        // The live solo voice (gated, not mid-declick) carrying `note`.
        let live = |e: &Engine, note: u8| {
            e.alloc
                .stacks
                .iter()
                .position(|s| s.meta.gate && s.meta.note == note && s.meta.phase != VoicePhase::Declick)
        };

        e.note_on(60, 100);
        e.process_block(&mut l, &mut r);
        e.note_on(64, 90);
        e.process_block(&mut l, &mut r);
        assert!(live(&e, 64).is_some(), "a live voice plays the new note");

        // Release the top note while 60 is still held → fallback to 60.
        e.note_off(64);
        e.process_block(&mut l, &mut r);
        assert!(
            live(&e, 60).is_some(),
            "released solo note must fall back to the held note"
        );

        // It is audibly sounding, not a gated corpse.
        let mut peak = 0.0_f32;
        for _ in 0..(SR as usize) / 10 / BLK {
            e.process_block(&mut l, &mut r);
            for i in 0..BLK {
                peak = peak.max(l[i].abs()).max(r[i].abs());
            }
        }
        assert!(peak > 1e-3, "fallback note silent (peak = {peak})");

        // Releasing the last note finally gates every voice off.
        e.note_off(60);
        for _ in 0..(SR as usize) / 4 / BLK {
            e.process_block(&mut l, &mut r);
        }
        assert!(
            e.alloc.stacks.iter().all(|s| !s.meta.gate),
            "all keys up → voices released"
        );
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
            (e.alloc.stacks[slot].modulation.global_pitch_mod_st[0] - target).abs() < 1e-5,
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
            let im = modulated.alloc.stacks[am].core.ops[0].phase_inc[0];
            let ib = baseline.alloc.stacks[ab].core.ops[0].phase_inc[0];
            if im != ib {
                diverged = true;
                break;
            }
        }
        assert!(diverged, "GlobalPitch matrix slot did not shift phase_inc");
    }

    // --- Lfo2Phase dest (E008 0091) --------------------------------------

    fn active_stack(e: &Engine) -> usize {
        e.alloc.stacks.iter().position(|s| !s.is_idle()).unwrap()
    }

    /// `voice-rand → lfo2-phase` (depth > 0) scatters the 8 lanes' LFO2
    /// phases — the canonical supersaw-shimmer route. `voice_rand[k]` is
    /// seeded distinct per lane at note-on, so the per-lane offsets decorrelate.
    #[test]
    fn matrix_voice_rand_to_lfo2_phase_decorrelates_lanes() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, MatrixTable, SourceId};

        let mut e = Engine::new(SR, BLK);
        e.matrix = MatrixTable::default();
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::VoiceRand,
            dest: DestId::Lfo2Phase,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        // One-block latency: offset applied end of block 1, visible from 2.
        for _ in 0..4 {
            e.process_block(&mut l, &mut r);
        }
        let a = active_stack(&e);
        let phases = e.alloc.stacks[a].meta.lfo2.phase;
        let mut distinct = std::collections::HashSet::new();
        for p in phases {
            distinct.insert(p);
        }
        assert!(
            distinct.len() >= STACK_LANES - 1,
            "lfo2 phases not scattered: {phases:?}"
        );
    }

    /// Off-path bit-identity: with no `lfo2-phase` slot, note-on locks all
    /// lanes to the shape zero-crossing and the shared `inc` advances them
    /// in lock-step — the offset code never touches `lfo2.phase`.
    #[test]
    fn matrix_no_lfo2_phase_slot_keeps_lanes_phase_locked() {
        let mut e = Engine::new(SR, BLK);
        // The default patch ships a `voice-rand → lfo2-phase` slot (now live);
        // clear the table so this asserts the genuine off-path.
        e.matrix = crate::matrix::MatrixTable::default();
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        for _ in 0..4 {
            e.process_block(&mut l, &mut r);
        }
        let a = active_stack(&e);
        let phases = e.alloc.stacks[a].meta.lfo2.phase;
        for k in 1..STACK_LANES {
            assert_eq!(phases[k], phases[0], "lane {k} drifted without a slot");
        }
    }

    /// A static `lfo2-phase` mod holds a fixed per-lane scatter across blocks
    /// — the delta-not-absolute application means the offset settles, it
    /// doesn't accumulate. The inter-lane phase differences (all lanes share
    /// `inc`) must be identical early and late.
    #[test]
    fn matrix_lfo2_phase_static_offset_does_not_run_away() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, MatrixTable, SourceId};

        let mut e = Engine::new(SR, BLK);
        e.matrix = MatrixTable::default();
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::VoiceRand,
            dest: DestId::Lfo2Phase,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        for _ in 0..5 {
            e.process_block(&mut l, &mut r);
        }
        let a = active_stack(&e);
        let early: [u32; STACK_LANES] = std::array::from_fn(|k| {
            e.alloc.stacks[a].meta.lfo2.phase[k].wrapping_sub(e.alloc.stacks[a].meta.lfo2.phase[0])
        });
        for _ in 0..40 {
            e.process_block(&mut l, &mut r);
        }
        let late: [u32; STACK_LANES] = std::array::from_fn(|k| {
            e.alloc.stacks[a].meta.lfo2.phase[k].wrapping_sub(e.alloc.stacks[a].meta.lfo2.phase[0])
        });
        assert_eq!(early, late, "static lfo2-phase offset ran away over blocks");
    }

    /// A coarser (patch-global) source into `lfo2-phase` is coherent: it
    /// broadcasts the *same* offset to every lane (no decorrelation), shifting
    /// the whole stack's LFO2 phase off the no-slot baseline. mod-wheel = 1.0
    /// × depth 0.25 = a quarter-cycle (0x4000_0000) Q32 shift, applied once.
    #[test]
    fn matrix_mod_wheel_to_lfo2_phase_broadcasts_equal_offset() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, MatrixTable, SourceId};

        let mut modulated = Engine::new(SR, BLK);
        // Clear the default `voice-rand → lfo2-phase` slot so only the
        // patch-global broadcast route is in play.
        modulated.matrix = MatrixTable::default();
        modulated.matrix.slots[0] = MatrixSlot {
            source: SourceId::ModWheel,
            dest: DestId::Lfo2Phase,
            depth: 0.25,
            curve: CurveKind::Lin,
        };
        modulated.set_mod_wheel(1.0);
        modulated.note_on(60, 100);

        let mut baseline = Engine::new(SR, BLK);
        baseline.matrix = MatrixTable::default();
        baseline.note_on(60, 100);

        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        for _ in 0..4 {
            modulated.process_block(&mut l, &mut r);
            baseline.process_block(&mut l, &mut r);
        }
        let am = active_stack(&modulated);
        let ab = active_stack(&baseline);
        let mp = modulated.alloc.stacks[am].meta.lfo2.phase;
        let bp = baseline.alloc.stacks[ab].meta.lfo2.phase;
        // Broadcast: every lane carries the same offset.
        for k in 1..STACK_LANES {
            assert_eq!(mp[k], mp[0], "lane {k} decorrelated under a patch-global source");
        }
        // …and that offset is a quarter cycle off the baseline.
        assert_eq!(mp[0].wrapping_sub(bp[0]), 0x4000_0000, "expected +¼ cycle shift");
    }

    /// Fresh note on a reused stack snaps the offset tracking to 0 (note-on
    /// re-locks the phases) so it doesn't glide in from the previous voice;
    /// after retrigger the scatter is stable, finite, and re-derived.
    #[test]
    fn matrix_lfo2_phase_fresh_note_snaps_offset() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, MatrixTable, SourceId};

        let mut e = Engine::new(SR, BLK);
        e.matrix = MatrixTable::default();
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::VoiceRand,
            dest: DestId::Lfo2Phase,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        e.note_on(60, 100);
        for _ in 0..6 {
            e.process_block(&mut l, &mut r);
        }
        // Retrigger the same note → reused stack slot hits the `fresh` branch.
        e.note_off(60);
        e.process_block(&mut l, &mut r);
        e.note_on(60, 100);
        for _ in 0..6 {
            e.process_block(&mut l, &mut r);
        }
        let a = active_stack(&e);
        // Static offset stable after retrigger (no accumulation across notes).
        let d1: [u32; STACK_LANES] = std::array::from_fn(|k| {
            e.alloc.stacks[a].meta.lfo2.phase[k].wrapping_sub(e.alloc.stacks[a].meta.lfo2.phase[0])
        });
        for _ in 0..20 {
            e.process_block(&mut l, &mut r);
        }
        let d2: [u32; STACK_LANES] = std::array::from_fn(|k| {
            e.alloc.stacks[a].meta.lfo2.phase[k].wrapping_sub(e.alloc.stacks[a].meta.lfo2.phase[0])
        });
        assert_eq!(d1, d2, "offset unstable after fresh retrigger");
        // Phases remain finite (no NaN propagation) — trivially true for u32,
        // but assert the scatter survived the retrigger.
        let mut distinct = std::collections::HashSet::new();
        for p in e.alloc.stacks[a].meta.lfo2.phase {
            distinct.insert(p);
        }
        assert!(distinct.len() >= STACK_LANES - 1, "scatter lost after retrigger");
    }

    // --- Lfo1Rate / Lfo2Rate dests (E008 0092) ---------------------------

    /// `mod-wheel → lfo1-rate` sweeps LFO1 speed in the log domain: depth 1 ×
    /// mod-wheel 1.0 × gain 4 = +4 octaves → `rate_mult ≈ 16`. Patch-global,
    /// one-block latency, and only live while a voice plays (the accumulator
    /// is aggregated across active stacks like the FX mixes).
    #[test]
    fn matrix_mod_wheel_to_lfo1_rate_sweeps_log_domain() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, MatrixTable, SourceId};

        let mut e = Engine::new(SR, BLK);
        e.matrix = MatrixTable::default();
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::ModWheel,
            dest: DestId::Lfo1Rate,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        e.set_mod_wheel(1.0);
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        // Block 1 aggregates the offset; block 2 applies it (one-block latency).
        for _ in 0..3 {
            e.process_block(&mut l, &mut r);
        }
        let m = e.patch_mod.lfo1.rate_mult;
        assert!(
            (m - 16.0).abs() < 0.5,
            "lfo1 rate_mult {m} not ≈ 16 (+4 oct)"
        );
    }

    /// `velocity → lfo2-rate` sweeps each voice's LFO2 speed per-stack: two
    /// voices at different velocities get different `rate_mult` (per-stack
    /// independence).
    #[test]
    fn matrix_velocity_to_lfo2_rate_is_per_stack() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, MatrixTable, SourceId};

        let mut e = Engine::new(SR, BLK);
        e.matrix = MatrixTable::default();
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::Velocity,
            dest: DestId::Lfo2Rate,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        e.note_on(60, 30);
        e.note_on(67, 120);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        for _ in 0..3 {
            e.process_block(&mut l, &mut r);
        }
        let mults: Vec<f32> = e
            .alloc
            .stacks
            .iter()
            .filter(|s| !s.is_idle())
            .map(|s| s.meta.lfo2.rate_mult)
            .collect();
        assert!(mults.len() >= 2, "expected two active voices, got {}", mults.len());
        // The two velocities map to distinct octave offsets → distinct mults.
        let mut distinct = std::collections::HashSet::new();
        for m in &mults {
            distinct.insert(m.to_bits());
        }
        assert!(distinct.len() >= 2, "per-stack lfo2 rate not independent: {mults:?}");
        // Both swept up from unity (positive velocity → positive octaves).
        for m in &mults {
            assert!(*m > 1.0, "rate_mult {m} not swept up");
        }
    }

    /// Gated + bit-identical: with no rate slot, both LFO rate multipliers
    /// stay exactly 1.0 (the eval path takes its un-modulated branch).
    #[test]
    fn matrix_no_lfo_rate_slot_keeps_rate_mult_unity() {
        let mut e = Engine::new(SR, BLK);
        e.matrix = crate::matrix::MatrixTable::default();
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        for _ in 0..4 {
            e.process_block(&mut l, &mut r);
        }
        assert_eq!(e.patch_mod.lfo1.rate_mult, 1.0);
        let a = active_stack(&e);
        assert_eq!(e.alloc.stacks[a].meta.lfo2.rate_mult, 1.0);
    }

    /// Self-rate feedback (`lfo1 → lfo1-rate`, flagged incoherent by 0090) is
    /// well-defined under the one-block latency — it must stay finite and
    /// bounded by the Hz clamp, never run away or NaN.
    #[test]
    fn matrix_lfo1_self_rate_feedback_is_bounded() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, MatrixTable, SourceId};

        let mut e = Engine::new(SR, BLK);
        e.matrix = MatrixTable::default();
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::Lfo1,
            dest: DestId::Lfo1Rate,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        for _ in 0..200 {
            e.process_block(&mut l, &mut r);
            let m = e.patch_mod.lfo1.rate_mult;
            assert!(m.is_finite() && m > 0.0, "lfo1 rate_mult diverged: {m}");
        }
    }

    // --- Unit sanification (E008 0094) -----------------------------------

    /// `pitch-eg → global-pitch` no longer double-scales: a full-scale EG at
    /// unity depth reaches ±24 st (the dest's gain), NOT `peg_depth × 24`. With
    /// `peg_depth = 2`, the old raw-semitone path gave 48 st; the normalized
    /// source gives 24.
    #[test]
    fn matrix_pitch_eg_into_pitch_no_double_scale() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        let gp_idx = DestId::GlobalPitch.idx().unwrap();
        let run = |peg_l: i8| -> f32 {
            let mut e = Engine::new(SR, BLK);
            e.matrix = crate::matrix::MatrixTable::default();
            e.matrix.slots[0] = MatrixSlot {
                source: SourceId::PitchEg,
                dest: DestId::GlobalPitch,
                depth: 1.0,
                curve: CurveKind::Lin,
            };
            // Full-scale EG, fast rates, and a 2-semitone configured swing.
            e.params.patch.voice.peg_depth = 2.0;
            e.params.patch.voice.pitch_eg.l = [peg_l, peg_l, peg_l, peg_l];
            e.params.patch.voice.pitch_eg.r = [99, 99, 99, 99];
            e.note_on(60, 100);
            let mut l = [0.0_f32; BLK];
            let mut r = [0.0_f32; BLK];
            for _ in 0..40 {
                e.process_block(&mut l, &mut r);
            }
            let a = active_stack(&e);
            e.dest_vals[a][0][gp_idx]
        };
        // Full positive EG (shape +1) → +24 st (±2 oct), not +48.
        let pos = run(99);
        assert!(
            (pos - 24.0).abs() < 1.0,
            "pitch-eg→global-pitch = {pos} st, expected ≈24 (not 48 = double-scale)"
        );
        // Full negative EG (shape −1) → −24 st — sign preserved, normalized.
        let neg = run(-99);
        assert!((neg + 24.0).abs() < 1.0, "negative EG = {neg} st, expected ≈ −24");
    }

    /// The normalized pitch-EG source stays within `[-1, 1]` even at a large
    /// configured `peg_depth` — the shape is unit-bounded; the dest gain owns
    /// the excursion.
    #[test]
    fn pitch_eg_source_is_normalized_shape() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        // Route into a gain-1 dest (op1-pan) so the dest accumulator reads the
        // raw source shape, undistorted by a gain.
        let pan_idx = DestId::Op1Pan.idx().unwrap();
        let mut e = Engine::new(SR, BLK);
        e.matrix = crate::matrix::MatrixTable::default();
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::PitchEg,
            dest: DestId::Op1Pan,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        e.params.patch.voice.peg_depth = 7.0; // large swing
        e.params.patch.voice.pitch_eg.l = [99, 99, 99, 99];
        e.params.patch.voice.pitch_eg.r = [99, 99, 99, 99];
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        let mut peak = 0.0_f32;
        for _ in 0..40 {
            e.process_block(&mut l, &mut r);
            let a = active_stack(&e);
            peak = peak.max(e.dest_vals[a][0][pan_idx].abs());
        }
        assert!(peak <= 1.0 + 1e-4, "pitch-eg source not normalized: peak {peak}");
        assert!(peak > 0.9, "pitch-eg source did not reach full shape: peak {peak}");
    }

    // --- StackDetune / StackSpread dests (E008 0093) ---------------------

    /// Spin up a stacked voice (density 4, real detune + spread) so the
    /// `stack-detune`/`stack-spread` macros have something to scale.
    fn stacked_engine() -> Engine {
        let mut e = Engine::new(SR, BLK);
        e.params.patch.stack.density = 4;
        e.params.patch.stack.detune_cents_max = 30.0;
        e.params.patch.stack.spread = 0.5;
        e.matrix = crate::matrix::MatrixTable::default();
        e
    }

    fn phase_incs(e: &Engine, slot: usize) -> [[u32; STACK_LANES]; vxn2_dsp::algo::N_OPS] {
        std::array::from_fn(|op| std::array::from_fn(|k| e.alloc.stacks[slot].core.ops[op].phase_inc[k]))
    }

    /// `key → stack-detune` re-cooks the per-lane detune → `phase_inc` shifts
    /// off the un-routed baseline (keytrack detune). Static source ⇒ snaps on
    /// the fresh note.
    #[test]
    fn matrix_key_to_stack_detune_shifts_phase_inc() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        let mut modulated = stacked_engine();
        modulated.matrix.slots[0] = MatrixSlot {
            source: SourceId::Key,
            dest: DestId::StackDetune,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        modulated.note_on(72, 100);

        let mut baseline = stacked_engine();
        baseline.note_on(72, 100);

        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        for _ in 0..3 {
            modulated.process_block(&mut l, &mut r);
            baseline.process_block(&mut l, &mut r);
        }
        let am = active_stack(&modulated);
        let ab = active_stack(&baseline);
        assert_ne!(
            phase_incs(&modulated, am),
            phase_incs(&baseline, ab),
            "stack-detune did not re-cook phase_inc"
        );
    }

    /// `velocity → stack-spread` widens the `VoiceSpread` source: a
    /// `voice-spread → op1-pan` slot reading it pans wider than with the
    /// spread route absent (the AC's "source tracks the modulated spread").
    #[test]
    fn matrix_velocity_to_stack_spread_widens_voice_spread_source() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        let pan_span = |e: &Engine| -> f32 {
            let a = active_stack(e);
            let pans = e.alloc.stacks[a].core.pan_l[0];
            let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
            for k in 0..4 {
                lo = lo.min(pans[k]);
                hi = hi.max(pans[k]);
            }
            hi - lo
        };

        let mut modulated = stacked_engine();
        modulated.matrix.slots[0] = MatrixSlot {
            source: SourceId::Velocity,
            dest: DestId::StackSpread,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        modulated.matrix.slots[1] = MatrixSlot {
            source: SourceId::VoiceSpread,
            dest: DestId::Op1Pan,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        modulated.note_on(60, 120);

        let mut baseline = stacked_engine();
        baseline.matrix.slots[1] = MatrixSlot {
            source: SourceId::VoiceSpread,
            dest: DestId::Op1Pan,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        baseline.note_on(60, 120);

        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        for _ in 0..6 {
            modulated.process_block(&mut l, &mut r);
            baseline.process_block(&mut l, &mut r);
        }
        assert!(
            pan_span(&modulated) > pan_span(&baseline) + 1e-4,
            "stack-spread did not widen the VoiceSpread source (mod={}, base={})",
            pan_span(&modulated),
            pan_span(&baseline)
        );
    }

    /// Gated/bit-identical: with no stack-macro slot, `phase_inc` matches the
    /// baseline exactly and the per-stack mod state stays 0 (no extra powf).
    #[test]
    fn matrix_no_stack_macro_slot_is_bit_identical() {
        let mut e = stacked_engine();
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        for _ in 0..4 {
            e.process_block(&mut l, &mut r);
        }
        let a = active_stack(&e);
        assert_eq!(e.stack_detune_mod[a], 0.0);
        assert_eq!(e.stack_spread_mod[a], 0.0);
        for k in 0..STACK_LANES {
            assert_eq!(e.alloc.stacks[a].modulation.detune_mod_st[k], 0.0);
        }
    }

    /// Per-stack independence: two notes at different keys get different
    /// `stack-detune` amounts.
    #[test]
    fn matrix_stack_detune_is_per_stack() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        let mut e = stacked_engine();
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::Key,
            dest: DestId::StackDetune,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        e.note_on(36, 100);
        e.note_on(96, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        for _ in 0..3 {
            e.process_block(&mut l, &mut r);
        }
        let mods: Vec<f32> = (0..N_STACKS)
            .filter(|&i| !e.alloc.stacks[i].is_idle())
            .map(|i| e.stack_detune_mod[i])
            .collect();
        assert!(mods.len() >= 2, "expected two voices");
        let mut distinct = std::collections::HashSet::new();
        for m in &mods {
            distinct.insert(m.to_bits());
        }
        assert!(distinct.len() >= 2, "stack-detune not per-stack: {mods:?}");
    }

    /// Smoothing: a dynamic source change ramps the detune amount over blocks
    /// rather than stepping the full target in one — the documented zipper
    /// guard. Static (fresh) sources still snap (covered above).
    #[test]
    fn matrix_stack_detune_dynamic_change_is_ramped() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        let mut e = stacked_engine();
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::ModWheel,
            dest: DestId::StackDetune,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        e.set_mod_wheel(0.0);
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        for _ in 0..4 {
            e.process_block(&mut l, &mut r);
        }
        let a = active_stack(&e);
        assert_eq!(e.stack_detune_mod[a], 0.0, "should settle at 0 with wheel down");
        // Jump the wheel mid-note: the amount must ramp, not snap.
        e.set_mod_wheel(1.0);
        e.process_block(&mut l, &mut r);
        let after_one = e.stack_detune_mod[a];
        assert!(
            after_one > 0.0 && after_one < 1.0,
            "dynamic detune not ramped (got {after_one}, expected partway to 1.0)"
        );
        for _ in 0..20 {
            e.process_block(&mut l, &mut r);
        }
        assert!(
            (e.stack_detune_mod[a] - 1.0).abs() < 1e-2,
            "detune did not converge to target"
        );
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
        let pm = modulated.alloc.stacks[a].core.pan_l[0][0];
        let pb = baseline.alloc.stacks[b].core.pan_l[0][0];
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

    /// A non-CLAP KS-curve write on the shared store threads through
    /// `snapshot_params` → `read_op` into the per-op DSP params the audio
    /// path reads, so the curve actually changes the level response.
    #[test]
    fn shared_ks_curve_writes_reach_engine_op_params() {
        use vxn2_dsp::ks::KsCurve;

        let shared = SharedParams::new();
        // Default is left NegLin / right NegExp; flip op3 (index 2) to the
        // boost variants.
        shared.set_ks_curve_raw(2, 0, KsCurve::PosLin as u8);
        shared.set_ks_curve_raw(2, 1, KsCurve::PosExp as u8);

        let mut e = Engine::new(SR, BLK);
        e.snapshot_params(&shared);
        let op = e.params.patch.voice.ops[2];
        assert_eq!(op.ks_l_curve, KsCurve::PosLin);
        assert_eq!(op.ks_r_curve, KsCurve::PosExp);
        // An untouched op keeps the legacy frozen default.
        let op0 = e.params.patch.voice.ops[0];
        assert_eq!(op0.ks_l_curve, KsCurve::NegLin);
        assert_eq!(op0.ks_r_curve, KsCurve::NegExp);
    }

    /// A non-CLAP EG-curve write on the shared store threads through
    /// `snapshot_params` → `read_op` into the per-op DSP params, so the curve
    /// actually selects the level→amplitude mapping (ticket 0124).
    #[test]
    fn shared_eg_curve_writes_reach_engine_op_params() {
        use vxn2_dsp::eg::EgCurve;

        let shared = SharedParams::new();
        // Default is Exp on every op; flip op4 (index 3) to Lin.
        shared.set_eg_curve_raw(3, EgCurve::Lin as u8);

        let mut e = Engine::new(SR, BLK);
        e.snapshot_params(&shared);
        assert_eq!(e.params.patch.voice.ops[3].eg_curve, EgCurve::Lin);
        // An untouched op keeps the default (Exp).
        assert_eq!(e.params.patch.voice.ops[0].eg_curve, EgCurve::Exp);
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
        let fb_op = spec_of(e.alloc.stacks[s].meta.algo).structural_fb_op as usize;
        let want = fb_scale(4.0);
        // ModWheel is a patch-level source: every lane gets the same amount.
        for (k, got) in e.alloc.stacks[s].core.ops[fb_op - 1].fb_scale.iter().enumerate() {
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
        let fb_op = spec_of(e.alloc.stacks[s].meta.algo).structural_fb_op as usize;
        let got = e.alloc.stacks[s].core.ops[fb_op - 1].fb_scale;
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

    // --- Stack-pitch scatter + mask gating (E022 0069) -------------------

    /// Per-op pitch accumulator column (op-major stride 3).
    fn op_pitch_col(op_1indexed: usize) -> usize {
        (op_1indexed - 1) * 3
    }

    /// Drive one block with a `ModWheel → Op{target}StackPitch` route at the
    /// given algo / wall set, and return the active stack's per-lane dest
    /// accumulator (post-scatter). `mod_wheel = 0.5`, depth 1.0, Lin → each
    /// targeted op's pitch column gains `0.5 × 24 = 12` semitones.
    fn run_stack_pitch(
        algo: u8,
        fixed_ops: &[usize],
        target_op: usize,
    ) -> [[f32; N_DESTS]; STACK_LANES] {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, MatrixTable, SourceId};
        let dest = DestId::from_u8(DestId::Op1StackPitch as u8 + (target_op as u8 - 1));
        let mut e = Engine::new(SR, BLK);
        e.params.patch.voice.algo = algo;
        for &op in fixed_ops {
            e.params.patch.voice.ops[op - 1].ratio_mode = RatioMode::Fixed;
        }
        // Cook masks from the voice params *before* installing the matrix
        // (apply_block_params rebuilds the table from rows, so set it after).
        e.apply_block_params();
        e.matrix = MatrixTable::default();
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::ModWheel,
            dest,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        e.mod_wheel = 0.5;
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        e.process_block(&mut l, &mut r);
        e.dest_vals[active_stack(&e)]
    }

    /// Algo 1 chain (6→5→4→3): a stack-pitch route on op3 bends the whole
    /// component {3,4,5,6} by an identical 12 st; the separate (2→1) pair is
    /// untouched.
    #[test]
    fn stack_pitch_scatters_equal_delta_across_component() {
        let dv = run_stack_pitch(1, &[], 3);
        for k in 0..STACK_LANES {
            for op in [3, 4, 5, 6] {
                assert!(
                    (dv[k][op_pitch_col(op)] - 12.0).abs() < 1e-3,
                    "op{op} pitch {} != 12",
                    dv[k][op_pitch_col(op)]
                );
            }
            // Ops outside the component get no bend.
            assert_eq!(dv[k][op_pitch_col(1)], 0.0);
            assert_eq!(dv[k][op_pitch_col(2)], 0.0);
        }
    }

    /// The scatter is additive with an existing per-op pitch route: an
    /// `Op4Pitch` route stacks on top of the bend on op4 only.
    #[test]
    fn stack_pitch_additive_with_per_op_pitch() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, MatrixTable, SourceId};
        let mut e = Engine::new(SR, BLK);
        e.params.patch.voice.algo = 1;
        e.apply_block_params();
        e.matrix = MatrixTable::default();
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::ModWheel,
            dest: DestId::Op3StackPitch,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        // Per-op pitch on op4, gain 24, but cubic-tapered depth 1.0 → 1.0.
        e.matrix.slots[1] = MatrixSlot {
            source: SourceId::ModWheel,
            dest: DestId::Op4Pitch,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        e.mod_wheel = 0.5;
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        e.process_block(&mut l, &mut r);
        let dv = e.dest_vals[active_stack(&e)];
        for k in 0..STACK_LANES {
            // op4 = stack bend (12) + per-op pitch (12) = 24.
            assert!((dv[k][op_pitch_col(4)] - 24.0).abs() < 1e-3);
            // op3/5/6 only the stack bend.
            assert!((dv[k][op_pitch_col(3)] - 12.0).abs() < 1e-3);
            assert!((dv[k][op_pitch_col(5)] - 12.0).abs() < 1e-3);
        }
    }

    /// A fixed op mid-chain walls the graph: a bend on op3 (algo 1) with op5
    /// fixed reaches op4 but not across op5; op6 is severed.
    #[test]
    fn stack_pitch_wall_splits_component() {
        let dv = run_stack_pitch(1, &[5], 3);
        for k in 0..STACK_LANES {
            assert!((dv[k][op_pitch_col(3)] - 12.0).abs() < 1e-3);
            assert!((dv[k][op_pitch_col(4)] - 12.0).abs() < 1e-3);
            // op5 walled (excluded), op6 severed past the wall.
            assert_eq!(dv[k][op_pitch_col(5)], 0.0);
            assert_eq!(dv[k][op_pitch_col(6)], 0.0);
        }
    }

    /// A stack-pitch route whose target op is itself fixed → empty component →
    /// no pitch change anywhere (the accumulator scatters nothing).
    #[test]
    fn stack_pitch_fixed_target_is_noop() {
        let dv = run_stack_pitch(1, &[3], 3);
        for k in 0..STACK_LANES {
            for op in 1..=6 {
                assert_eq!(
                    dv[k][op_pitch_col(op)], 0.0,
                    "op{op} bent despite fixed target"
                );
            }
        }
    }

    /// Masks re-resolve on a Ratio↔Fixed toggle and on an algo change, but a
    /// ratio-*value* tweak (same modes) does NOT re-resolve — the cache key is
    /// unchanged and the masks are byte-identical (E022 0069 acceptance).
    #[test]
    fn stack_pitch_masks_recook_gating() {
        let mut e = Engine::new(SR, BLK);
        e.params.patch.voice.algo = 1;
        e.apply_block_params();
        let key0 = e.stack_pitch_key;
        let masks0 = e.stack_pitch_masks;

        // Ratio-value tweak: change op4's num — mode unchanged → no re-resolve.
        e.params.patch.voice.ops[3].num = 7;
        e.apply_block_params();
        assert_eq!(e.stack_pitch_key, key0, "ratio-value tweak re-resolved");
        assert_eq!(e.stack_pitch_masks, masks0);

        // Ratio↔Fixed toggle on op5: re-resolves (op5 becomes a wall).
        e.params.patch.voice.ops[4].ratio_mode = RatioMode::Fixed;
        e.apply_block_params();
        assert_ne!(e.stack_pitch_key, key0, "ratio-mode toggle did not re-cook");
        assert_ne!(e.stack_pitch_masks, masks0);
        let key1 = e.stack_pitch_key;

        // Algo change: re-resolves.
        e.params.patch.voice.algo = 22;
        e.apply_block_params();
        assert_ne!(e.stack_pitch_key, key1, "algo change did not re-cook");
    }

    // --- E022 0071: render-level ratio-lock + shared-modulator spread ----
    //
    // Wall-split, fixed-target no-op, and recook gating are asserted in the
    // 0069 block above (they exercise the same cook + render path). These two
    // add the frequency-domain ratio-lock proof and the shared-modulator case.

    /// Render a harmonic algo-1 patch (ops 4/5/6 at ratios 2/3/4) until the
    /// pitch smoother settles, optionally with a `ModWheel → Op{target}
    /// StackPitch` route (depth 1.0, mod_wheel 0.5 → +12 st = one octave).
    /// Returns each op's settled lane-0 `phase_inc` (∝ frequency).
    fn harmonic_phase_incs(stack_pitch_target: Option<usize>) -> [u32; 6] {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, MatrixTable, SourceId};
        let mut e = Engine::new(SR, BLK);
        e.params.patch.voice.algo = 1;
        e.params.patch.voice.ops[3].num = 2;
        e.params.patch.voice.ops[4].num = 3;
        e.params.patch.voice.ops[5].num = 4;
        e.apply_block_params();
        e.matrix = MatrixTable::default();
        if let Some(t) = stack_pitch_target {
            let dest = DestId::from_u8(DestId::Op1StackPitch as u8 + (t as u8 - 1));
            e.matrix.slots[0] = MatrixSlot {
                source: SourceId::ModWheel,
                dest,
                depth: 1.0,
                curve: CurveKind::Lin,
            };
            e.mod_wheel = 0.5;
        }
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        for _ in 0..80 {
            e.process_block(&mut l, &mut r);
        }
        let a = active_stack(&e);
        let mut out = [0u32; 6];
        for op in 0..6 {
            out[op] = e.alloc.stacks[a].core.ops[op].phase_inc[0];
        }
        out
    }

    /// Ratio-lock: a static stack-pitch bend scales every op in the branch by
    /// the *same* frequency factor (one octave ≈ ×2), so the FM ratios are
    /// invariant — pitch moves, timbre holds. Ops outside the component are
    /// untouched.
    #[test]
    fn stack_pitch_ratio_lock_preserves_fm_ratios() {
        let base = harmonic_phase_incs(None);
        let bent = harmonic_phase_incs(Some(3)); // bend op3's component {3,4,5,6}
        let mut factors = Vec::new();
        for op in [3usize, 4, 5, 6] {
            let f = bent[op - 1] as f64 / base[op - 1] as f64;
            factors.push(f);
            assert!((f - 2.0).abs() < 0.02, "op{op} factor {f} not ~2.0 (octave)");
        }
        // Identical factor across the component ⇒ ratios preserved.
        for w in factors.windows(2) {
            assert!(
                (w[0] - w[1]).abs() < 1e-3,
                "ratio drift across branch: {factors:?}"
            );
        }
        // The separate (2→1) component is unbent.
        for op in [1usize, 2] {
            let f = bent[op - 1] as f64 / base[op - 1] as f64;
            assert!((f - 1.0).abs() < 1e-3, "op{op} bent unexpectedly: {f}");
        }
    }

    /// Shared-modulator spread: algo 22's op6 fans into carriers {3,4,5}, so a
    /// stack-pitch route on op6 legitimately bends the whole {3,4,5,6}
    /// component (documented large component); the separate (2→1) pair stays.
    #[test]
    fn stack_pitch_shared_modulator_spreads_to_all_carriers() {
        let dv = run_stack_pitch(22, &[], 6);
        for k in 0..STACK_LANES {
            for op in [3, 4, 5, 6] {
                assert!(
                    (dv[k][op_pitch_col(op)] - 12.0).abs() < 1e-3,
                    "shared-mod op{op} not bent"
                );
            }
            assert_eq!(dv[k][op_pitch_col(1)], 0.0);
            assert_eq!(dv[k][op_pitch_col(2)], 0.0);
        }
    }
}
