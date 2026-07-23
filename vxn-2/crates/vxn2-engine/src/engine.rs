//! Top-level engine: assembles the kernel and exposes a block-based `process`
//! surface the CLAP shell and the integration test below bind against.
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
use vxn2_dsp::halfband::{Interpolator, Oversampler, roundtrip_latency_base_samples};
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

// Matrix dest-accumulator indices, resolved at compile time (no live
// `unwrap()` in the hot path). The op-major stride-3 layout (Pitch, Level, Pan
// per op) is asserted alongside.
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
/// contiguous from here. `OpNStackPitch` column = base + (N-1).
const OP_STACK_PITCH_BASE_IDX: usize = DestId::Op1StackPitch.idx().unwrap();
/// Per-block one-pole factor smoothing *dynamic* stack-detune/spread changes.
/// Fresh notes snap (immediate, zipper-free for static sources like
/// key/velocity); only block-to-block motion from a moving source (mod-env,
/// LFO) is ramped, keeping the re-cooked detune zipper-free at musical rates.
/// ~0.5 ⇒ converges within a few blocks (~ms).
const STACK_MACRO_SMOOTH: f32 = 0.5;
/// Row of `Lfo2Phase` inside the per-stack `PitchSmoother` (the smoother's
/// row order is `PITCH_DESTS`: `[GlobalPitch, Lfo2Phase, Op1..Op6]`). The
/// matrix `*→lfo2_phase` route reads this smoother row and applies it as a
/// per-lane LFO2 phase offset.
const LFO2_PHASE_SMOOTHER_ROW: usize = 1;
const _: () = assert!(matches!(
    PITCH_DESTS[LFO2_PHASE_SMOOTHER_ROW],
    DestId::Lfo2Phase
));
/// Lowest cutoff the ladder is driven to — C0 (MIDI 12), ≈16.35 Hz. Lets a
/// fully key-tracked, C0-based cutoff reach bass pitches.
const CUTOFF_MIN_HZ: f32 = 16.3516;
/// Highest cutoff the ladder is driven to (the `filter-cutoff` param ceiling).
const CUTOFF_MAX_HZ: f32 = 20_000.0;
/// Key-tracking centre note — C0 (MIDI 12). At full key-track the cutoff shifts
/// `(note − 12)/12` octaves, so a C0-floored cutoff tracks the played pitch.
const KEYTRACK_CENTRE_NOTE: f32 = 12.0;

/// Filter saturator headroom / gain-staging. Filtering the post-stack-sum runs
/// hot (rms ≈ 1.6, peaks ≈ 4–5), so the ladder's per-stage `tanh` compresses
/// deep into its knee — ≈ −7 dB at default drive even at density 1. We trim the
/// signal into the saturator's near-linear region and make it up after, so
/// `drive = 1` is ≈ transparent at the passband (cutoff open) and `drive` stays
/// the knob that pushes into saturation. Equivalent to a `tanh` headroom of
/// `1/TRIM`; self-oscillation stays bounded (the kernel limit cycle is ±~1,
/// scaled by the make-up). Lives engine-side so the kernel and its unit tests
/// are untouched.
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
/// Largest oversample factor the filter path supports. The shipped factor is
/// fixed at [`OVERSAMPLE_FACTOR`] (4×), but the OS scratch / bus buffers are
/// sized for 8× so the kernel/unit tests can still drive the ladder at higher
/// factors without overflowing them.
const MAX_OVERSAMPLE: usize = 8;

/// The one shipped oversample factor: the filter and the dynamics FX share a
/// single 4× span. Used to build the dynamics block at the oversampled rate and
/// to size the dynamics-only span (`run_dynamics_os`).
pub(crate) const OVERSAMPLE_FACTOR: usize = 4;

/// The engine's constant processing latency, in samples at the host rate — the
/// oversampled span's resampler round-trip. [`SpanDelay`] holds the dry/bypass
/// path at this same delay, so the group delay is identical whether the
/// filter/dynamics span is engaged or not. That constancy is what makes it safe
/// to report to the host: a *changing* latency would force a host `activate`
/// restart on every filter/dynamics toggle (an audible dropout). The CLAP shell
/// reports this once via the latency extension.
pub const LATENCY_SAMPLES: u32 = roundtrip_latency_base_samples(OVERSAMPLE_FACTOR);

/// Quiescence floor for the per-stack filter skip, in ladder state magnitude.
/// An idle stack feeds the filter exact zero, so once *every*
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
/// that have converged skip the recook entirely.
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

/// Per-stack ramp bookkeeping. One struct per slot, so adding a new ramp type
/// touches only this struct, not several parallel index sites.
///
/// `level_mod` / `pan_l` / `pan_r` are per-sample f32 increments; `phase_mod` is
/// a signed Q32 increment (wrapping, so the cyclic phase ramp is exact). All
/// four glide their `stack.*` field linearly across the block while the slot's
/// `ramp_live` flag is set. `prev_eg` is the EG level the previous block's ramp
/// targeted, used to rebase the level ramp across the EG's block-edge march. It
/// is **per-lane**: each unison lane carries its own amp EG, so with an
/// `eg-rate` spread the lanes march at different speeds and each needs its own
/// block-edge rebase. `STACK_LANES` (8) and `N_OPS` (6) are both ≤ 32, so
/// `derive(Default)` (and `Clone`, for `vec![_; N_STACKS]`) cover the array
/// fields.
#[derive(Clone, Default)]
struct RampState {
    level_mod: [[f32; STACK_LANES]; vxn2_dsp::algo::N_OPS],
    pan_l: [[f32; STACK_LANES]; vxn2_dsp::algo::N_OPS],
    pan_r: [[f32; STACK_LANES]; vxn2_dsp::algo::N_OPS],
    phase_mod: [[i32; STACK_LANES]; vxn2_dsp::algo::N_OPS],
    prev_eg: [[f32; STACK_LANES]; vxn2_dsp::algo::N_OPS],
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

/// Block-rate gates passed into [`Engine::cook_stacks_block`]: whether any
/// matrix route targets each of the four re-cooked stack/LFO destinations,
/// scanned once in [`Engine::process_block`]. Grouped because four bare
/// positional bools at the call site are a transpose hazard — named fields
/// (`flags.stack_detune`) read unambiguously.
#[derive(Clone, Copy)]
struct TargetFlags {
    lfo2_rate: bool,
    stack_detune: bool,
    stack_spread: bool,
    stack_pitch: bool,
}

/// Lifecycle of the oversampled filter+dynamics span. Exactly one variant holds
/// per block, which is what makes the two declick crossfades —
/// the filter toggle and the dynamics-disengage settle — mutually exclusive *by
/// construction* rather than by careful flag ordering. Dynamics activeness is
/// deliberately not an axis: it's owned by `DynamicsBlock`'s own wet fade and
/// enters [`Engine::advance_os_span`] as an input, so folding it in would only
/// duplicate that state and multiply the variants.
#[derive(Clone, Copy, PartialEq, Debug)]
enum OsSpan {
    /// Both legs off — base-rate render, no resampling.
    Bypassed,
    /// Filter off, dynamics live — the global-upsample dynamics-only span.
    DynOnly,
    /// Filter on (dynamics folds into its per-stack span when live).
    Filtered,
    /// Filter-toggle declick in flight. `remaining` samples of the window left;
    /// `to_on` is the fade target (true ⇒ resolves to `Filtered`, false ⇒ to
    /// `DynOnly`/`Bypassed`).
    FilterFade { remaining: u32, to_on: bool },
    /// Dynamics-driven span engage/disengage with the filter off. The dyn-only
    /// span adds the resampler's ~L-sample group delay, so switching it in or out
    /// steps the latency (0↔L) and clicks unless bridged. Over `remaining`
    /// samples we render *both* the base mix (latency 0) and the OS mix (latency
    /// L, via `run_dynamics_os`) and raised-cosine blend between them: `to_os`
    /// true fades base→OS (engaging), false fades OS→base (disengaging), so the
    /// group-delay step is smeared across the window either way.
    SpanFade { remaining: u32, to_os: bool },
}

impl OsSpan {
    /// Whether the filter param was on as of this state — steady `Filtered`, or
    /// the target of an in-flight `FilterFade`. Used to spot the toggle edge.
    fn filter_on(self) -> bool {
        matches!(self, OsSpan::Filtered | OsSpan::FilterFade { to_on: true, .. })
    }

    /// Decrement an in-flight fade by one rendered block; steady states are
    /// inert. Resolution to the steady target happens in `advance_os_span` on
    /// the next block, once `remaining` has reached 0.
    fn tick(&mut self, n: usize) {
        let n = n as u32;
        match self {
            OsSpan::FilterFade { remaining, .. } | OsSpan::SpanFade { remaining, .. } => {
                *remaining = remaining.saturating_sub(n);
            }
            _ => {}
        }
    }
}

/// Fixed integer stereo delay of exactly the resampler's round-trip latency
/// ([`roundtrip_latency_base_samples`]). The non-oversampled (base-rate) output
/// is pushed through this so its latency matches the oversampled path's — the
/// whole engine then carries a *constant* group delay whether or not the
/// filter+dynamics span is engaged. That's what stops engaging/disengaging the
/// span from stepping the latency, which is the only thing a declick crossfade
/// can't hide (bridging a latency change means crossfading the signal with a
/// delayed copy of itself — a comb). A synth has no dry reference to phase
/// against, so the fixed ~0.5 ms delay is inaudible; it is deliberately not
/// reported to the host (matches the filter-latency posture).
struct SpanDelay {
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    idx: usize,
}

impl SpanDelay {
    fn new(delay: usize) -> Self {
        let len = delay.max(1);
        Self {
            buf_l: vec![0.0; len],
            buf_r: vec![0.0; len],
            idx: 0,
        }
    }

    fn reset(&mut self) {
        self.buf_l.fill(0.0);
        self.buf_r.fill(0.0);
        self.idx = 0;
    }

    /// Push one stereo sample and return the sample `delay` samples old.
    #[inline]
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let out = (self.buf_l[self.idx], self.buf_r[self.idx]);
        self.buf_l[self.idx] = l;
        self.buf_r[self.idx] = r;
        self.idx += 1;
        if self.idx >= self.buf_l.len() {
            self.idx = 0;
        }
        out
    }
}

/// Top-level audio engine. Owns every sub-engine plus the per-block
/// parameter snapshot.
pub struct Engine {
    pub alloc: PolyAlloc,
    pub matrix: MatrixTable,
    pub patch_mod: PatchMod,
    pub cleanup: CleanupFilter,
    /// Dynamics block (comp + sat), inserted **first** in the FX bus so it evens
    /// FM transients before phaser / delay / reverb accumulate them. Bypassed
    /// bit-exactly when `dyn-on = 0`.
    pub dynamics: DynamicsBlock,
    /// Stereo phaser, inserted between dynamics and delay in the FX bus.
    /// Bypassed bit-exactly when `phaser-on = 0`.
    pub phaser: StereoPhaser,
    pub delay: StereoDelay,
    pub reverb: FdnReverb,
    pub master: MasterState,
    /// Optional brickwall limiter on the master bus (last in the FX chain).
    /// Run only when `master.limiter_on` is set; bypassed otherwise.
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
    /// MIDI CC1 mod wheel, normalised `[0, 1]`. Read by the matrix engine as a
    /// patch-global source; stored here so the CLAP shell can push it without
    /// reaching across the matrix.
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
    /// Per-stack pitch-dest smoothers. Targets refresh at
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
    /// applied by the matrix `*→lfo2_phase` route. Each block the
    /// engine adds the *delta* vs this value to `lfo2.phase[k]` so a static
    /// offset settles to a fixed scatter (no runaway); on a fresh note it
    /// resets to 0 (note_on already zeroed the per-lane phases) so the offset
    /// snaps in with the note rather than gliding from the previous voice.
    prev_lfo2_phase_off: [[f32; STACK_LANES]; N_STACKS],
    /// LFO1 rate offset (octaves) aggregated from the matrix `*→lfo1-rate`
    /// route at the end of the previous block, applied to this block's LFO1
    /// `eval` as `2^oct` (one-block latency). `lfo1-rate` is a
    /// patch-global dest; the value is averaged at lane 0 across active stacks
    /// exactly like the FX-mix dests. 0 when un-targeted → multiplier 1.0.
    lfo1_rate_oct: f32,
    /// Per-stack smoothed `stack-detune` / `stack-spread` modulation amounts.
    /// Detune is applied this block (folded into
    /// `apply_pitch_mult`); spread feeds *next* block's `VoiceSpread` source
    /// scaling (one-block latency, since the source is evaluated before the
    /// matrix). Both snap on a fresh note and one-pole toward the target
    /// otherwise; 0 when un-targeted → bit-identical off-path.
    stack_detune_mod: [f32; N_STACKS],
    stack_spread_mod: [f32; N_STACKS],
    /// Per-stack ramp state (one [`RampState`] per slot): the per-sample
    /// level / pan / phase increments that glide `stack.op_level_mod`,
    /// `stack.pan_l` / `pan_r`, and `stack.op_phase_mod_q32` to each block's
    /// matrix targets, plus `prev_eg` — the previous block's EG level, used to
    /// rebase the level ramp across the EG's block-edge march. One struct per
    /// slot so adding a ramp type touches only [`RampState`]. Engine-owned so
    /// the `stack_tick_*` hot path stays untouched; the render loop advances
    /// them once per sample while live.
    ramps: Vec<RampState>,
    /// Which slots carry a live ramp this block; `any_ramp_live` is the
    /// whole-engine OR so a patch with static effective levels pays one
    /// branch per sample.
    ramp_live: [bool; N_STACKS],
    any_ramp_live: bool,

    /// Stack-pitch component masks: `stack_pitch_masks[n]` is the
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

    // Optional per-voice filter: two scalar OTA-C ladder kernels per stack
    // (L/R) — the filter runs on a stack's summed stereo pair. Plus one
    // interpolating resampler per stack (per-voice upsample, stateful) and a
    // single shared decimator per channel (deferred decimation past the
    // voice-sum). All allocated once; untouched while `filter-enable` is off.
    filter_l: Vec<OtaLadderKernel>,
    filter_r: Vec<OtaLadderKernel>,
    // Static high-pass stage: two scalar one-pole HP kernels per stack (L/R),
    // run on each stack's
    // summed stereo pair at base rate (never oversampled) *ahead* of the
    // musical filter. Global cutoff (not a mod dest), so the coefficient is
    // computed once per block and broadcast. Bypassed (untouched) while the
    // cutoff sits at its 20 Hz floor — the default — so the common case pays
    // nothing.
    hp_l: Vec<HpfKernel>,
    hp_r: Vec<HpfKernel>,
    interp_l: Vec<Interpolator>,
    interp_r: Vec<Interpolator>,
    // Global interpolator pair for the dynamics-only leg of the span (filter
    // off, dynamics on): the summed dry mix is upsampled once here before the 4×
    // dynamics. When the filter is on the per-stack `interp_*` do the upsampling
    // instead and this stays idle. Its downsampling is the *same* shared
    // `decim_*` the filter uses — there is one decimator per channel at the end
    // of the whole filter+dynamics span, whichever legs are engaged.
    interp_mix_l: Interpolator,
    interp_mix_r: Interpolator,
    decim_l: Oversampler,
    decim_r: Oversampler,
    /// Constant-latency delay applied to the base-rate (non-oversampled) output
    /// so the engine's group delay never changes as the span engages/disengages.
    /// See [`SpanDelay`].
    span_delay: SpanDelay,
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
    /// Oversampled-span lifecycle: which render body runs this block, and how far
    /// through a declick crossfade we are. Single source of truth for the filter
    /// toggle edge, the engaged↔disengaged edge (decimator reset), and both
    /// crossfade countdowns — see [`OsSpan`] / [`Self::advance_os_span`]. Seeded
    /// from the live filter param so a patch that boots with the filter on
    /// doesn't fade in from dry.
    os_span: OsSpan,
    /// Crossfade window length in samples (`FILTER_XFADE_MS`). Both the filter
    /// toggle and the span-settle fade count down from this.
    filter_xfade_len: usize,
    /// OFF-path (raw, unfiltered) dry bus, rendered alongside the filtered
    /// `dry_l/r` only during a toggle crossfade (`block_size`). Untouched in
    /// steady state.
    dry_alt_l: Vec<f32>,
    dry_alt_r: Vec<f32>,
    /// Last [`SharedParams::load_epoch`] seen in [`Self::snapshot_params`]. When
    /// it changes, a new preset was loaded in bulk and any voice still ringing
    /// from the old patch is silenced before the new algorithm takes effect.
    last_load_epoch: u64,
    /// Last patch algorithm applied in [`Self::apply_block_params`]. A live algo
    /// change (picker move, not a preset load) re-routes held notes in place —
    /// but a voice still *releasing* from note-off can have an op that is a
    /// modulator become a carrier, exposing its long release tail as a new
    /// audible tone. On the change we declick-kill releasing voices; held voices
    /// re-route and morph. `0` is the never-applied sentinel (algos are 1-based).
    last_algo: u8,
}

impl Engine {
    pub fn new(sample_rate: f32, block_size: usize) -> Self {
        let mut e = Self {
            alloc: PolyAlloc::new(sample_rate),
            matrix: default_patch::default_matrix(),
            patch_mod: PatchMod::new(0xDEAD_BEEF_DEAD_BEEF),
            cleanup: CleanupFilter::new(sample_rate),
            // Dynamics always runs inside the 4× oversampled span (in the
            // filter's per-stack span when the filter is on, else in its own
            // `run_dynamics_os` span), so its detector + smoothers are built at
            // the oversampled rate.
            dynamics: DynamicsBlock::new(sample_rate * OVERSAMPLE_FACTOR as f32),
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
            interp_mix_l: Interpolator::new(),
            interp_mix_r: Interpolator::new(),
            decim_l: Oversampler::new(),
            decim_r: Oversampler::new(),
            span_delay: SpanDelay::new(LATENCY_SAMPLES as usize),
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
            os_span: OsSpan::Bypassed,
            filter_xfade_len: (FILTER_XFADE_MS * 0.001 * sample_rate) as usize,
            dry_alt_l: vec![0.0; block_size],
            dry_alt_r: vec![0.0; block_size],
            // Matches a fresh SharedParams (epoch 0) so the first snapshot after
            // open doesn't read as a preset change and silence the boot patch.
            last_load_epoch: 0,
            last_algo: 0, // never-applied sentinel
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

    /// Normalised `[-1, +1]` pitch bend. ±2 semitones.
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
        // Drop any matrix LFO-rate modulation; a fresh patch re-derives it from
        // the matrix accumulator.
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
            self.ramps[i].prev_eg = [[0.0; STACK_LANES]; vxn2_dsp::algo::N_OPS];
            self.filter_l[i].reset();
            self.filter_r[i].reset();
            self.hp_l[i].reset();
            self.hp_r[i].reset();
            self.interp_l[i].reset();
            self.interp_r[i].reset();
        }
        self.interp_mix_l.reset();
        self.interp_mix_r.reset();
        self.decim_l.reset();
        self.decim_r.reset();
        self.span_delay.reset();
        self.any_ramp_live = false;
        // Seed the span lifecycle to the steady state the live filter param
        // implies, aborting any in-flight crossfade — so the first block after
        // reset renders that state outright rather than fading in from a stale
        // dry bus. (`Bypassed` when the filter is off even if the dynamics is
        // live: the first block then transitions Bypassed→DynOnly with no fade,
        // and the decimator it would reset is already clear from just above.)
        self.os_span = if self.params.filter.enable {
            OsSpan::Filtered
        } else {
            OsSpan::Bypassed
        };
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
        // A bumped load-epoch means the whole patch was just swapped (preset
        // load / reset). Silence any voice still ringing from the old patch
        // *before* apply_block_params live-swaps the algorithm, so its
        // operators — possibly re-roled (modulator → carrier) — can't sound
        // through the new topology.
        let epoch = shared.load_epoch();
        if epoch != self.last_load_epoch {
            self.last_load_epoch = epoch;
            self.alloc.silence_all();
        }
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
                // Inactive slots still carry their scale source through
                // (harmless — an inactive slot's primary source is `None`, so
                // eval skips it before the scale multiply matters).
                scale_src: SourceId::from_u8(row.scale_src),
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
        // A live algo change re-routes voices in place. A still-*releasing* voice
        // (note-off, ringing out) can have an op that was a modulator become a
        // carrier under the new algo, dumping its long release tail onto the audio
        // bus as a surprise tone. Declick-kill those releasing voices before the
        // re-route; held (gated) voices re-route and morph as intended. Preset
        // loads take the `load_epoch` → `silence_all` path instead, so by here
        // those voices are already idle and skipped.
        let algo_changed = voice.algo != self.last_algo;
        self.last_algo = voice.algo;
        for i in 0..self.alloc.stacks.len() {
            if algo_changed
                && !self.alloc.stacks[i].meta.gate
                && !self.alloc.stacks[i].is_idle()
            {
                self.alloc.stacks[i].start_declick();
            }
            self.alloc.stacks[i].set_algo_live(voice.algo);
            self.alloc.stacks[i].set_feedback_live(voice.feedback);
        }
        // Re-resolve the stack-pitch component masks if the algorithm or any
        // op's Ratio/Fixed mode changed. Folded into the same cook
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

    /// True when any live slot targets an amp-EG rate dest. Gates the note-on
    /// rescale so a patch with no `eg-rate` route never touches the cooked EG
    /// rates — the off-path stays bit-identical.
    fn eg_rate_targeted(&self) -> bool {
        self.matrix.slots.iter().any(|s| {
            matches!(
                s.dest,
                DestId::GlobalEgRate
                    | DestId::Op1EgRate
                    | DestId::Op2EgRate
                    | DestId::Op3EgRate
                    | DestId::Op4EgRate
                    | DestId::Op5EgRate
                    | DestId::Op6EgRate
                    | DestId::PitchEgRate
                    | DestId::ModEnvRate
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

    /// Release all voices and clear hold state — used on transport stop.
    pub fn all_notes_off(&mut self) {
        self.alloc.all_notes_off();
    }

    pub fn set_bend(&mut self, semitones: f32) {
        self.alloc.set_bend(semitones);
    }

    /// Sustain pedal (CC64). Poly-only: while held, a poly note-off is
    /// deferred until release. Solo mode keeps last-note-priority unchanged.
    pub fn set_sustain(&mut self, on: bool) {
        self.alloc.set_sustain(on);
    }

    /// Host transport restarted (stop→play). Realign LFO1 phase to the bar
    /// grid so a synced rhythmic shape (pulse / saw) locks to the host beat.
    /// No-op when LFO1 is free-running — a free LFO shouldn't jump on play.
    pub fn on_transport_restart(&mut self) {
        if self.params.mod_params.lfo1.sync {
            self.patch_mod.on_transport_restart();
        }
    }

    /// True if any active matrix slot drives `dest` (source set + nonzero
    /// depth). Block-rate gate for the deferred rate/re-cook dests so an
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

        // LFO-rate matrix routes, gated by a block-rate scan so
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

        // Stack-macro routes, gated so the un-targeted path skips
        // the re-cook and stays bit-identical. Detune is applied this block;
        // spread feeds next block's VoiceSpread source (one-block latency).
        let stack_detune_targeted = self.dest_targeted(DestId::StackDetune);
        let stack_spread_targeted = self.dest_targeted(DestId::StackSpread);
        // Stack-pitch scatter gate: skip the whole scatter when no
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
            TargetFlags {
                lfo2_rate: lfo2_rate_targeted,
                stack_detune: stack_detune_targeted,
                stack_spread: stack_spread_targeted,
                stack_pitch: stack_pitch_targeted,
            },
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

        // Static high-pass stage: a global cutoff, so one coefficient is
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

        // Advance the oversampled-span lifecycle from its two inputs — the filter
        // param and whether the dynamics is contributing — running the resampler
        // resets each transition demands. This is the single decision point for
        // engaged↔disengaged and fade arming; the dispatch below is a pure match.
        self.advance_os_span(filter_enabled, self.dynamics.is_active());

        match self.os_span {
            OsSpan::FilterFade { remaining, to_on } => {
                // Toggle in flight: render both dry buses from one stack tick and
                // raised-cosine blend. `to_on` is the fade *target* — true ⇒
                // OFF→ON (filter rising), false ⇒ ON→OFF (filter falling).
                self.render_block_filter_xfade(out_l, out_r, n, hp_active, remaining, to_on);
            }
            OsSpan::Filtered => {
                // ON path — stack-major oversampled filter (ADR 0004 §3–§5).
                self.render_block_filtered(out_l, out_r, n, hp_active);
            }
            OsSpan::SpanFade { remaining, to_os } => {
                // Filter off, dyn-only span engaging/disengaging: OFF path bridges
                // the group delay over `remaining` samples. Re-prime the base-
                // filter smoother (as any filter-off block does) so re-enabling
                // snaps.
                self.filter_smooth_primed = false;
                self.render_block_off(out_l, out_r, n, hp_active, Some((remaining, to_os)));
            }
            OsSpan::DynOnly | OsSpan::Bypassed => {
                // Filter off: re-prime the base-filter smoother so re-enabling
                // snaps to the live param rather than sweeping up from the value
                // the cutoff sat at when the filter was last switched off.
                self.filter_smooth_primed = false;
                self.render_block_off(out_l, out_r, n, hp_active, None);
            }
        }
        // Count down any in-flight fade for the next block; steady states no-op.
        self.os_span.tick(n);

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

    /// Advance the [`OsSpan`] lifecycle one block from `(filter_on, dyn_active)`
    /// and run the resampler resets each transition requires. Single point of
    /// truth for three edges:
    ///
    /// - **filter toggle** (`filter_on` differs from the state's `filter_on()`):
    ///   arm a `FilterFade` toward the new state; on engage reset the per-stack
    ///   filter + upsamplers and re-prime the cutoff smoother, on disengage give
    ///   the global `interp_mix_*` a fresh start for the dynamics-only leg.
    /// - **dynamics engage/disengage** (filter off, `dyn_active` flipped): arm a
    ///   `SpanFade` in the matching direction to bridge the resampler group delay
    ///   the dyn-only span adds/removes — engaging clicks just as disengaging
    ///   does, so both are bridged.
    /// - **fresh span** (previous state was `Bypassed` — OS legs fully idle — and
    ///   the next runs them): reset the shared `decim_*` + `interp_mix_*`. Re-
    ///   arming a filter fade over an already-running span (engaging the filter
    ///   over live dynamics), or resolving/abandoning a `SpanFade`, is *not* such
    ///   a transition, so the decimator stays continuous — the click fix.
    ///
    /// A completed fade (`remaining` hit 0 last `tick`) resolves to its steady
    /// target here, using the current inputs. Dynamics activeness is only ever an
    /// input; it never appears in the state (see [`OsSpan`]).
    fn advance_os_span(&mut self, filter_on: bool, dyn_active: bool) {
        let len = self.filter_xfade_len as u32;
        let prev = self.os_span;
        let steady = |filter_on: bool, dyn_active: bool| {
            if filter_on {
                OsSpan::Filtered
            } else if dyn_active {
                OsSpan::DynOnly
            } else {
                OsSpan::Bypassed
            }
        };

        // Resolve a fade that ran out on the previous block to its steady target.
        let cur = match prev {
            OsSpan::FilterFade { remaining: 0, to_on } => steady(to_on, dyn_active),
            OsSpan::SpanFade { remaining: 0, .. } => steady(false, dyn_active),
            other => other,
        };

        let next = if filter_on != prev.filter_on() {
            // Filter toggled — arm the declick fade toward the new state (or hard
            // switch if the window rounds to zero at very low sample rates).
            if len > 0 {
                OsSpan::FilterFade { remaining: len, to_on: filter_on }
            } else {
                steady(filter_on, dyn_active)
            }
        } else {
            // No filter edge — only dynamics-driven transitions remain. Engaging
            // or disengaging the dyn-only span steps the latency (0↔L), so both
            // directions bridge through a `SpanFade` (or hard-switch if there's no
            // window).
            match cur {
                OsSpan::Bypassed if dyn_active => {
                    if len > 0 {
                        OsSpan::SpanFade { remaining: len, to_os: true }
                    } else {
                        OsSpan::DynOnly
                    }
                }
                OsSpan::DynOnly if !dyn_active => {
                    if len > 0 {
                        OsSpan::SpanFade { remaining: len, to_os: false }
                    } else {
                        OsSpan::Bypassed
                    }
                }
                // Dynamics came back mid-disengage ⇒ abandon it, resume the span
                // (the decimator kept running through the fade, so this is
                // continuous — no reset).
                OsSpan::SpanFade { to_os: false, .. } if dyn_active => OsSpan::DynOnly,
                keep => keep,
            }
        };

        // A fresh span starts (clean decimator + global upsampler history) only
        // when the OS legs were fully idle last block (`Bypassed`) and the next
        // state runs them. A `SpanFade` — engage *or* disengage — already runs
        // the OS legs, so transitions out of one never reset mid-signal.
        let started = prev == OsSpan::Bypassed && next != OsSpan::Bypassed;
        if started {
            self.decim_l.reset();
            self.decim_r.reset();
            self.interp_mix_l.reset();
            self.interp_mix_r.reset();
        }

        // Per-transition resampler prep on a filter toggle.
        if filter_on != prev.filter_on() {
            if filter_on {
                self.filter_smooth_primed = false;
                for i in 0..N_STACKS {
                    self.filter_l[i].reset();
                    self.filter_r[i].reset();
                    self.interp_l[i].reset();
                    self.interp_r[i].reset();
                }
            } else {
                self.interp_mix_l.reset();
                self.interp_mix_r.reset();
            }
        }

        self.os_span = next;
    }

    /// Mod-matrix cook: the per-stack loop extracted from `process_block`.
    /// Per active stack it ticks LFO2, fans the patch / stack /
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
    // The four block-rate target gates are grouped into `flags` (built once in
    // `process_block`); the remaining args — `n`, `dt`, `filter_enabled`, the
    // patch-source snapshot — stay flat.
    fn cook_stacks_block(
        &mut self,
        n: usize,
        dt: f32,
        filter_enabled: bool,
        flags: TargetFlags,
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

        // eg-rate note-on scale gate: resolved once per block; the
        // per-stack apply below is further gated on `fresh` so the rescale
        // (which multiplies the cooked rates) lands exactly once per note.
        let eg_rate_targeted = self.eg_rate_targeted();

        for i in 0..self.alloc.stacks.len() {
            // == STAGE 1: Idle skip — release the slot's ramp, forget its EG (early-out). ==
            if self.alloc.stacks[i].is_idle() {
                self.ramp_live[i] = false;
                // Forget the last rendered EG level so a future fresh note reusing
                // this slot rebases its onset from silence, not a stale level left
                // by a hard-silenced (declicked) voice.
                self.ramps[i].prev_eg = [[0.0; STACK_LANES]; vxn2_dsp::algo::N_OPS];
                continue;
            }
            // == STAGE 2: Fresh-note detection — clears macro mods before the VoiceSpread
            //          source is built (one-block latency). ==
            // Fresh-note detection up front, needed before the VoiceSpread
            // source is built so the spread-mod doesn't glide in from the
            // previous voice on a reused stack. A bumped allocation generation
            // means a new note reused this slot.
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
            // Pitch EG → normalized [-1, 1] shape: divide the raw
            // semitone output by its full-scale swing (`peg_depth`) so the
            // pitch dest's ±24 st gain sets the excursion — no hidden 24×
            // re-scale of absolute semitones. peg_depth ≈ 0 ⇒ EG output is 0
            // anyway, so the source reads 0.
            let pitch_eg = {
                let depth = voice.peg_depth;
                if depth.abs() > 1e-6 {
                    // Pitch EG is per-lane; the matrix source reads lane 0
                    // to keep its per-stack tier (all lanes are identical unless a
                    // pitch-eg-rate route decorrelates them).
                    stack.meta.pitch_eg[0].level_st / depth
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
            // route further scales this by `(1 + spread_mod)` using
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
            // component into the per-op pitch columns, before the
            // smoother captures pitch targets below. Gated so the common
            // (no stack-pitch route) path is untouched.
            if flags.stack_pitch {
                scatter_stack_pitch(&mut self.dest_vals[i], &self.stack_pitch_masks);
            }

            // == STAGE 5b: eg-rate note-on scale. On the fresh block only,
            //          fold the per-lane eg-rate dest columns into each lane's
            //          cooked amp-EG rates so a `voice-spread → eg-rate` route
            //          makes the unison lanes evolve at different speeds. Note-on
            //          static: applied exactly once (rescale multiplies), reading
            //          the VoiceSpread source that is already valid on the fresh
            //          block. The value is in octaves (`rate · 2^oct`, ±4 oct at
            //          full depth), clamped to ±4 oct so a multi-route sum can't
            //          drive the rate to an absurd extreme. ==
            if fresh && eg_rate_targeted {
                let global_idx = DestId::GlobalEgRate.idx().unwrap();
                let op_eg_base = DestId::Op1EgRate.idx().unwrap();
                let pitch_idx = DestId::PitchEgRate.idx().unwrap();
                let mod_idx = DestId::ModEnvRate.idx().unwrap();
                // Amp EGs: per-op × per-lane (global + per-op route).
                let mut eg_rate_scale = [[1.0_f32; STACK_LANES]; vxn2_dsp::algo::N_OPS];
                // Pitch EG: per-lane (global + pitch route).
                let mut pitch_scale = [1.0_f32; STACK_LANES];
                for k in 0..STACK_LANES {
                    let g = self.dest_vals[i][k][global_idx];
                    for op_i in 0..vxn2_dsp::algo::N_OPS {
                        let oct = (g + self.dest_vals[i][k][op_eg_base + op_i]).clamp(-4.0, 4.0);
                        eg_rate_scale[op_i][k] = oct.exp2();
                    }
                    pitch_scale[k] = (g + self.dest_vals[i][k][pitch_idx]).clamp(-4.0, 4.0).exp2();
                }
                // Mod Env: one-per-voice → lane-0 collapse (global + mod route).
                let mod_oct =
                    (self.dest_vals[i][0][global_idx] + self.dest_vals[i][0][mod_idx]).clamp(-4.0, 4.0);
                let stack = &mut self.alloc.stacks[i];
                stack.rescale_eg_rates(&eg_rate_scale);
                stack.rescale_pitch_eg_rates(&pitch_scale);
                stack.rescale_mod_env_rate(mod_oct.exp2());
            }

            // == STAGE 6: Per-op level/pan/phase target projection from dest_vals. ==
            // Project per-op level + pan destinations into the stack.
            // Indices: OpiLevel=i*3+1, OpiPan=i*3+2. Neither applies as a
            // block constant: level ramps linearly to this block's target
            // via per-sample increments, and pan ramps the folded equal-
            // power gains the same way. Pitch-shaped destinations ride the
            // per-stack PitchSmoother instead.
            let mut level_targets = [[0.0_f32; STACK_LANES]; vxn2_dsp::algo::N_OPS];
            // Phase dests: read the per-op phase offset (cycles) and fold to
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
            // Level modulation is MULTIPLICATIVE on the EG:
            // effective level = `clamp(eg · (1 + m), 0, 1)`, with `m` the
            // matrix accumulator. The tick reads `eg + op_level_mod`, so the
            // engine projects the multiplicative target into that additive
            // offset: `op_level_mod_target = clamp(eg·(1+m), 0, 1) − eg`.
            // `eg = 0` forces eff = 0, so a RELEASED op always closes.
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
            // The EG marches once per block; `op_level_mod` is rebased
            // by the block delta so the sum the tick reads stays continuous
            // across the edge, then ramps to the new target — the EG's motion
            // rides the same per-lane ramp as the matrix mod (no block-rate EG
            // staircase). Static patches with settled EGs pass through
            // bit-exact: m = 0 → target offset 0, rebase +0.
            let prev_eg = &mut self.ramps[i].prev_eg;
            for op_i in 0..vxn2_dsp::algo::N_OPS {
                for k in 0..STACK_LANES {
                    // Per-lane EG level: each lane's own envelope drives its
                    // own effective level and block-edge rebase.
                    let eg = stack.core.ops[op_i].eg[k].level;
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
                    stack.core.op_level_mod[op_i][k] += prev_eg[op_i][k] - eg;
                    prev_eg[op_i][k] = eg;
                }
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
                // blocks.
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
                        // Phase ramp: shortest-arc Q32 delta to the new
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
            // Stack-detune: re-derive the per-lane detune offset
            // from this block's lane-0 accumulator and fold it into the pitch
            // sum below. Snap on a fresh note (static sources like key/velocity
            // land immediately, zipper-free); one-pole the block-to-block
            // motion of a dynamic source otherwise. Gated: when un-targeted,
            // zero the offset once so the pitch path stays bit-identical.
            if flags.stack_detune {
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
            // (Pan gains are handled by the ramp above.)
            stack.apply_pitch_mult();

            // == STAGE 11: LFO2 phase offset + LFO2 rate — deferred, one-block latency
            //           (applied after this block's lfo2.eval in STAGE 3). ==
            // LFO2 phase offset (`*→lfo2_phase`). The smoothed
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
            // LFO2 per-stack rate. `lfo2-rate` is a per-stack dest;
            // read this block's lane-0 accumulator (in octaves) and stash the
            // multiplier for *next* block's `eval` (one-block latency). Gated:
            // an un-targeted stack keeps `rate_mult = 1.0` (bit-identical tick).
            lfo2.rate_mult = if flags.lfo2_rate {
                self.dest_vals[i][0][LFO2_RATE_IDX].exp2()
            } else {
                1.0
            };

            // == STAGE 12: Stack-spread update (next block's VoiceSpread, one-block latency)
            //           + FX-mix / LFO1-rate aggregation at lane 0. ==
            // Stack-spread: update the per-stack smoothed amount
            // for *next* block's VoiceSpread source scaling (one-block latency
            // — the source is built before the matrix eval). Snap on fresh,
            // one-pole otherwise; zero when un-targeted.
            if flags.stack_spread {
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
            // and cache for next block (one-block latency).
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

        // Accumulate the post-HP base mix (the same raw sum the OFF path emits)
        // into `dry_alt`, purely to keep `span_delay` primed: the constant-latency
        // delay must see a continuous base stream in *every* render body, or its
        // history goes stale during a long filter-on stretch and the filter-off
        // toggle's delayed OFF side reads garbage. Output is discarded here.
        self.dry_alt_l[..n].fill(0.0);
        self.dry_alt_r[..n].fill(0.0);

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
                // Quiescence-skip: an idle stack feeds the filter exact
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
                    self.dry_alt_l[sample] += sl; // base mix, for span_delay priming
                    self.dry_alt_r[sample] += sr;
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
                // Quiescence-skip: skip the upsample + ladder for an idle
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

                // Accumulate this stack's post-HP base into the mix (idle stacks
                // contribute their zero-filled scratch), for span_delay priming.
                for sample in 0..n {
                    self.dry_alt_l[sample] += self.base_l[sample];
                    self.dry_alt_r[sample] += self.base_r[sample];
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

            // 4b. Dynamics shares the filter's oversampled span: run comp + tanh
            //     saturation in place over the summed OS bus *before* decimation,
            //     so the detector + saturator see the 4× rate (sharper transient
            //     tracking, less saturation aliasing) and filter→dynamics is a
            //     single span with one resample pair — no decimate-then-
            //     re-upsample. Skipped (bit-exact) when the block is inactive.
            if self.dynamics.is_active() {
                for j in 0..osn {
                    let (l, r) = self.dynamics.process(self.bus_l[j], self.bus_r[j]);
                    self.bus_l[j] = l;
                    self.bus_r[j] = r;
                }
            }

            // 5. One shared decimation past the voice-sum (linear ⇒ exact).
            self.decim_l.decimate(&self.bus_l[..osn], &mut self.dry_l[..n], f);
            self.decim_r.decimate(&self.bus_r[..osn], &mut self.dry_r[..n], f);
        }

        // The fused unity-rate branch has no OS bus for the dynamics to share,
        // so when it runs (a test-only factor — production is pinned to 4×) the
        // dynamics takes its own 4× span over the dry mix instead.
        if f == 1 && self.dynamics.is_active() {
            self.run_dynamics_os(n);
        }

        // Keep the constant-latency delay primed with the base mix (output
        // discarded — the filtered `dry` is what plays). This is what lets the
        // filter-off toggle read a valid delayed OFF side after a long filter-on
        // stretch, and keeps the group delay constant across every render body.
        for sample in 0..n {
            self.span_delay.process(self.dry_alt_l[sample], self.dry_alt_r[sample]);
        }

        self.apply_tail_fx(n, out_l, out_r);
    }

    /// OFF-path render: the tuned sample-major sum loop. With the HP bypassed
    /// (default) this is byte-for-byte the original sum; when the HP is engaged
    /// each active stack's stereo pair is high-passed before it folds into the
    /// dry bus. Sums every active stack into `dry_l/r`, then the shared FX chain.
    /// Every `PITCH_SMOOTH_QUANTUM` samples the pitch smoothers advance one step
    /// toward this block's targets and the affected stacks re-cook `phase_inc` —
    /// converged smoothers (no active pitch route) skip the recook entirely.
    /// `fade` = `Some((remaining, to_os))` (the [`OsSpan::SpanFade`] countdown +
    /// direction) ⇒ the dyn-only span is engaging (`to_os` true) or disengaging
    /// (false); bridge the resampler group delay across the window. `None` ⇒
    /// steady `Bypassed`/`DynOnly` (the latter runs the dyn-only OS span when
    /// `is_active`). The caller ticks the countdown after the render.
    fn render_block_off(
        &mut self,
        out_l: &mut [f32],
        out_r: &mut [f32],
        n: usize,
        hp_active: bool,
        fade: Option<(u32, bool)>,
    ) {
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
        // Constant latency: push the base mix through `span_delay` so its group
        // delay matches the oversampled path (L samples). `dry_alt` becomes the
        // base signal delayed by L — the bypass output *and* the OFF side of the
        // engage/disengage bridge. Both are now latency L, so the whole engine
        // carries one fixed delay whether the span is engaged or not: engaging /
        // disengaging no longer steps the latency, and the bridge blends
        // same-latency signals (no comb — the fix for the hard-saturation blip).
        // Fed every block, so `span_delay` is primed the instant the span drops
        // back to bypass.
        for sample in 0..n {
            let (dl, dr) = self.span_delay.process(self.dry_l[sample], self.dry_r[sample]);
            self.dry_alt_l[sample] = dl;
            self.dry_alt_r[sample] = dr;
        }
        if let Some((remaining, to_os)) = fade {
            // The dyn-only span is engaging or disengaging: mask the resampler's
            // fill transient (its interp/decim start fresh on engage) by
            // raised-cosine blending the base-delayed-L signal with the OS output.
            // Both are latency L, so this only smooths the fill — it does not comb.
            self.run_dynamics_os(n); // dry ← OS mix (latency L)
            let len = self.filter_xfade_len as f32;
            let start = (self.filter_xfade_len - remaining as usize) as f32;
            let span = (len - 1.0).max(1.0);
            for sample in 0..n {
                let t = ((start + sample as f32) / span).min(1.0);
                // Raised-cosine rise, zero slope at both ends. `to_os`: OS weight
                // rises 0→1 (base→OS, engaging); else it falls 1→0 (OS→base).
                let rise = 0.5 - 0.5 * (core::f32::consts::PI * t).cos();
                let w_os = if to_os { rise } else { 1.0 - rise };
                let w_base = 1.0 - w_os;
                self.dry_l[sample] = w_os * self.dry_l[sample] + w_base * self.dry_alt_l[sample];
                self.dry_r[sample] = w_os * self.dry_r[sample] + w_base * self.dry_alt_r[sample];
            }
        } else if self.dynamics.is_active() {
            // DynOnly steady: the OS path already carries latency L. (`dry_alt`
            // holds the delayed base, unused here but keeping `span_delay` primed.)
            self.run_dynamics_os(n);
        } else {
            // Bypassed: emit the base signal delayed to the constant latency.
            self.dry_l[..n].copy_from_slice(&self.dry_alt_l[..n]);
            self.dry_r[..n].copy_from_slice(&self.dry_alt_r[..n]);
        }
        self.apply_tail_fx(n, out_l, out_r);
    }

    /// Dynamics-only leg of the span (filter off, dynamics on): upsample the
    /// summed dry mix through the global `interp_mix_*`, run the 4× comp+sat over
    /// it, and decimate back through the shared `decim_*` — the single decimator
    /// at the end of the whole filter+dynamics span. Only the *upsampler*
    /// differs from the filter-on leg (global mix vs per-stack); the decimation
    /// is one pass either way, and `decim_*` is reset only when the span's
    /// engaged state flips (see `process_block`), never on a filter toggle that
    /// leaves the span engaged — that continuity is what keeps engaging the
    /// filter over live dynamics click-free. Reuses the `bus_*` scratch, which
    /// the caller has finished with by the time this runs.
    fn run_dynamics_os(&mut self, n: usize) {
        let f = OVERSAMPLE_FACTOR;
        let osn = n * f;
        debug_assert!(osn <= self.bus_l.len(), "OS bus overflow: {osn}");
        self.interp_mix_l
            .interpolate(&self.dry_l[..n], &mut self.bus_l[..osn], f);
        self.interp_mix_r
            .interpolate(&self.dry_r[..n], &mut self.bus_r[..osn], f);
        for j in 0..osn {
            let (l, r) = self.dynamics.process(self.bus_l[j], self.bus_r[j]);
            self.bus_l[j] = l;
            self.bus_r[j] = r;
        }
        self.decim_l.decimate(&self.bus_l[..osn], &mut self.dry_l[..n], f);
        self.decim_r.decimate(&self.bus_r[..osn], &mut self.dry_r[..n], f);
    }

    /// Shared post-dry FX tail (cleanup → phaser → delay → reverb → master), run
    /// per sample over `dry_l/r` into `out_l/r`. The dynamics is *not* here — it
    /// runs upstream inside the 4× oversampled span (in the filter's per-stack
    /// span, or `run_dynamics_os`). Cleanup therefore sits just after the
    /// dynamics: safe, since the comp is pure gain and the `tanh` saturator is
    /// odd-symmetric (no DC), and running the 18 kHz guard after the saturator
    /// still strips any harmonics before the spatial FX — which is exactly where
    /// that guard belongs.
    fn apply_tail_fx(&mut self, n: usize, out_l: &mut [f32], out_r: &mut [f32]) {
        for sample in 0..n {
            let (cl, cr) = self.cleanup.process(self.dry_l[sample], self.dry_r[sample]);
            let (cl, cr) = self.phaser.process(cl, cr);
            let (l, r) = self.delay.process(cl, cr);
            let (l, r) = self.reverb.process(l, r);
            let (l, r) = self.master.apply(l, r);
            out_l[sample] = l;
            out_r[sample] = r;
        }
    }

    /// Declick render for the filter-enable toggle (ADR 0004 §10). Renders the
    /// dry bus *both* ways from a single stack tick — raw HP'd sum (OFF) and
    /// filtered (ON) — and raised-cosine blends them across `filter_xfade_len`
    /// samples before the shared FX tail. Rendering both from one tick is what
    /// makes the blend valid: the two buses are the same source material,
    /// differing only by the filter, so the crossfade hides exactly the
    /// discontinuity a hard switch would expose. `to_on` is the fade *target* —
    /// true ⇒ OFF→ON (filter rising), false ⇒ ON→OFF (filter falling).
    ///
    /// Two blend domains, chosen by whether the dynamics is live:
    /// - **dynamics inactive** — the span's engaged state *changes* across this
    ///   toggle (base ↔ OS), so there is a real latency step to bridge. Blend at
    ///   base rate: OFF stays base (latency 0, continuous with the pre-toggle dry
    ///   sum), only ON is oversampled+decimated.
    /// - **dynamics active** — the span is engaged on *both* sides of the toggle
    ///   (the dynamics keeps it live), so latency is constant. Blend at OS rate
    ///   per stack (raw vs filtered through the same interpolator → identical
    ///   latency), run the dynamics once over the summed bus, decimate once. This
    ///   keeps the single span decimator continuous across the toggle — the fix
    ///   for the engage clunk that reusing/​resetting a shared decimator caused.
    ///
    /// Edge-only: runs for one ~8 ms window per toggle, so the per-stack double
    /// pass and the second dry buffer never touch the steady-state hot paths.
    fn render_block_filter_xfade(
        &mut self,
        out_l: &mut [f32],
        out_r: &mut [f32],
        n: usize,
        hp_active: bool,
        remaining: u32,
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

        // Raised-cosine (equal-gain) blend weight, evaluated at a position in the
        // `len`-sample window. Derivative is zero at *both* endpoints, so neither
        // the engage start nor the steady handoff leaves a slope corner (an
        // equal-power `cos` weight has slope −π/2 at t=1 — a click at handoff).
        // `to_on`: ON weight rises 0→1; else ON falls 1→0.
        let len = self.filter_xfade_len as f32;
        let start = (self.filter_xfade_len - remaining as usize) as f32;
        let weight_at = |pos: f32, span: f32| {
            let t = (pos / span).min(1.0);
            let rise = 0.5 - 0.5 * (core::f32::consts::PI * t).cos();
            if to_on { (1.0 - rise, rise) } else { (rise, 1.0 - rise) }
        };

        // With the dynamics live the whole filter+dynamics span stays at the
        // oversampled rate across the toggle (both fade sides carry the same
        // latency the pre-/post-toggle steady states already have). The OFF side
        // is the base mix upsampled through the *continuous* `interp_mix_*` (the
        // dynamics-only leg's own upsampler — its history carries through the
        // engage), the ON side is the per-stack filtered sum; they raised-cosine
        // blend at OS rate, the dynamics runs once over the blend, and one
        // decimation closes the span. With the dynamics inactive there is a
        // genuine latency step (base ↔ OS) for the fade to bridge, so we keep the
        // base-rate blend: OFF stays base rate (matching the pre-toggle dry sum),
        // only ON is decimated. `f == 1` (test-only) has no OS bus → base path.
        let os_blend = self.dynamics.is_active() && f > 1;

        self.dry_l[..n].fill(0.0);
        self.dry_r[..n].fill(0.0);
        // `dry_alt` is the base-rate OFF mix (raw HP'd stack sum). The base path
        // blends it directly; the OS-blend path upsamples it through the
        // continuous `interp_mix_*` as the OFF side (see below).
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
                // OFF mix: raw HP'd stack sum (exactly `render_block_off`). The
                // base path blends this directly; the OS-blend path upsamples it
                // via `interp_mix_*` after the loop.
                for sample in 0..n {
                    self.dry_alt_l[sample] += self.base_l[sample];
                    self.dry_alt_r[sample] += self.base_r[sample];
                }
            }

            if os_blend {
                // OS-blend ON side: per-stack upsample → ladder → accumulate the
                // *filtered* signal into the bus. The per-stack `interp_l` were
                // reset on the engage edge, so this rings up from silence — which
                // is exactly what the rising ON weight wants. The OFF side is
                // added after the loop from the continuous `interp_mix_*`.
                self.interp_l[i].interpolate(&self.base_l[..n], &mut self.os_l[..osn], f);
                self.interp_r[i].interpolate(&self.base_r[..n], &mut self.os_r[..osn], f);
                for j in 0..osn {
                    self.bus_l[j] +=
                        self.filter_l[i].tick(self.os_l[j] * FILTER_IN_TRIM) * FILTER_OUT_MAKEUP;
                    self.bus_r[j] +=
                        self.filter_r[i].tick(self.os_r[j] * FILTER_IN_TRIM) * FILTER_OUT_MAKEUP;
                }
            } else if f == 1 {
                // ON dry, fused unity-rate (test-only).
                for sample in 0..n {
                    self.dry_l[sample] +=
                        self.filter_l[i].tick(self.base_l[sample] * FILTER_IN_TRIM) * FILTER_OUT_MAKEUP;
                    self.dry_r[sample] +=
                        self.filter_r[i].tick(self.base_r[sample] * FILTER_IN_TRIM) * FILTER_OUT_MAKEUP;
                }
            } else {
                // ON dry, oversampled — filtered signal only; blended at base
                // rate with `dry_alt` after decimation.
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

        // Keep `span_delay` advancing exactly one block here too (its delay would
        // drift if any render body skipped a block). The output is unused during a
        // filter toggle — both branches derive their OFF side elsewhere — so this
        // is purely to keep the constant-latency delay in lockstep with the base
        // stream. (The base-blend f > 1 branch below feeds + *uses* it instead, so
        // only prime here when that branch won't.)
        if os_blend || f == 1 {
            for sample in 0..n {
                self.span_delay.process(self.dry_alt_l[sample], self.dry_alt_r[sample]);
            }
        }

        if os_blend {
            // OFF side: upsample the base mix through the *continuous*
            // `interp_mix_*` — the same upsampler the dynamics-only leg
            // (`run_dynamics_os`) feeds on both sides of the toggle, so its
            // history carries straight through the engage with no discontinuity.
            // (The per-stack `interp_l` can't do this: they're reset on engage,
            // so reading the OFF side from them would ramp it up from silence and
            // step off the pre-engage signal — the clunk.) Then raised-cosine
            // blend OFF (`os_*`) ↔ ON (`bus`, the per-stack filtered sum) at the
            // OS rate, run the dynamics once over the blend, and decimate once
            // through the span's single decimator (kept continuous across the
            // toggle).
            self.interp_mix_l
                .interpolate(&self.dry_alt_l[..n], &mut self.os_l[..osn], f);
            self.interp_mix_r
                .interpolate(&self.dry_alt_r[..n], &mut self.os_r[..osn], f);
            let os_span = (len * f as f32 - 1.0).max(1.0);
            let os_start = start * f as f32;
            for j in 0..osn {
                let (w_off, w_on) = weight_at(os_start + j as f32, os_span);
                let (l, r) = self.dynamics.process(
                    w_off * self.os_l[j] + w_on * self.bus_l[j],
                    w_off * self.os_r[j] + w_on * self.bus_r[j],
                );
                self.bus_l[j] = l;
                self.bus_r[j] = r;
            }
            self.decim_l.decimate(&self.bus_l[..osn], &mut self.dry_l[..n], f);
            self.decim_r.decimate(&self.bus_r[..osn], &mut self.dry_r[..n], f);
        } else {
            if f > 1 {
                self.decim_l.decimate(&self.bus_l[..osn], &mut self.dry_l[..n], f);
                self.decim_r.decimate(&self.bus_r[..osn], &mut self.dry_r[..n], f);
                // Constant latency: delay the raw OFF side (`dry_alt`) through
                // `span_delay` so it carries the same L samples the decimated ON
                // side does. Both fade sides are then latency L — the toggle only
                // bridges timbre, never a latency step — and this keeps
                // `span_delay` primed for the drop back to bypass. (Same
                // signal `render_block_off` feeds it, so the stream is continuous.)
                for sample in 0..n {
                    let (dl, dr) =
                        self.span_delay.process(self.dry_alt_l[sample], self.dry_alt_r[sample]);
                    self.dry_alt_l[sample] = dl;
                    self.dry_alt_r[sample] = dr;
                }
            }
            // Base-rate raised-cosine blend OFF (`dry_alt`) ↔ ON (`dry`).
            let span = (len - 1.0).max(1.0);
            for sample in 0..n {
                let (w_off, w_on) = weight_at(start + sample as f32, span);
                self.dry_l[sample] = w_off * self.dry_alt_l[sample] + w_on * self.dry_l[sample];
                self.dry_r[sample] = w_off * self.dry_alt_r[sample] + w_on * self.dry_r[sample];
            }
            // f == 1 with dynamics live: no OS bus above, so the dynamics takes
            // its own span here (decim is a 1× identity, so no double-use).
            if self.dynamics.is_active() {
                self.run_dynamics_os(n);
            }
        }
        self.apply_tail_fx(n, out_l, out_r);
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

    /// Advance the live level/pan ramps one sample: straight lane-strided adds
    /// into the stacks' `op_level_mod` / `pan_l` / `pan_r`. Lives engine-side so
    /// the `stack_tick_*` hot path stays untouched; only ramping slots pay
    /// anything.
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
/// every op in component N. The **same** semitone delta is added
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
/// is consumed directly in `process_block` as a per-lane LFO2 phase offset,
/// so this fn skips row 1 and projects only the pitch dests.
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

    /// Render `n` blocks and collect the L and R samples.
    fn render_blocks(e: &mut Engine, n: usize) -> (Vec<f32>, Vec<f32>) {
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        let mut out_l = Vec::with_capacity(n * BLK);
        let mut out_r = Vec::with_capacity(n * BLK);
        for _ in 0..n {
            e.process_block(&mut l, &mut r);
            out_l.extend_from_slice(&l);
            out_r.extend_from_slice(&r);
        }
        (out_l, out_r)
    }

    /// Build a fresh `Engine` with a single matrix route.
    fn engine_with_route(
        source: crate::matrix::SourceId,
        dest: crate::matrix::DestId,
        depth: f32,
    ) -> Engine {
        use crate::matrix::{CurveKind, MatrixSlot, MatrixTable};
        let mut e = Engine::new(SR, BLK);
        e.matrix = MatrixTable::default();
        e.matrix.slots[0] = MatrixSlot {
            source,
            dest,
            depth,
            curve: CurveKind::Lin,
            scale_src: SourceId::None,
        };
        e
    }

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

    /// Listening-test gate (automated half): the default patch
    /// renders audible, non-clipping audio while held and decays to near-zero
    /// after note-off + reverb tail. RMS windows are 50 ms — long enough to
    /// average out per-cycle ripple, short enough to localise the segment.
    #[test]
    fn default_patch_renders_with_expected_envelope() {
        let mut e = Engine::new(SR, BLK);

        // Pre-settle: 4 blocks so reverb / delay state is at its silent floor
        // before the note fires (the AC's automated half — chord behaviour is
        // in the manual listening test).
        render_blocks(&mut e, 4);
        e.note_on(60, 100);

        // Render the full held + post-release timeline into one flat buffer
        // whose sample index 0 corresponds to the note-on.
        //   [0 .. 2 s)  — note held
        //   [2 s .. 3.6 s) — post-release decay (tail window at 3.5 s needs
        //                    50 ms clearance, so 3.6 s total suffices)
        let t_note_off   = ((SR * 2.0) as usize) / BLK; // block count
        let t_tail_end   = ((SR * 1.6) as usize) / BLK; // post-release blocks
        let (held_l, held_r) = render_blocks(&mut e, t_note_off);
        e.note_off(60);
        let (decay_l, decay_r) = render_blocks(&mut e, t_tail_end);

        // Single flat timeline from note-on at sample 0.
        let l: Vec<f32> = [held_l.as_slice(), decay_l.as_slice()].concat();
        let r: Vec<f32> = [held_r.as_slice(), decay_r.as_slice()].concat();

        // All samples must be finite.
        for (i, (&a, &b)) in l.iter().zip(r.iter()).enumerate() {
            assert!(a.is_finite() && b.is_finite(), "non-finite at sample {i}");
        }

        // Compute RMS + peak over a 50 ms window starting at `from`.
        // Returns (dbfs, peak). Factor of 2 in denominator: L and R are
        // separate slices of the same window length, so total samples = 2·win.
        let win = ((SR * 0.05) as usize / BLK) * BLK; // 37 blocks = 2 368 samples
        let window_dbfs_peak = |buf_l: &[f32], buf_r: &[f32], from: usize| -> (f32, f32) {
            let sl = &buf_l[from..from + win];
            let sr_w = &buf_r[from..from + win];
            let sum_sq: f64 = sl.iter().zip(sr_w.iter())
                .map(|(&a, &b)| (a as f64).powi(2) + (b as f64).powi(2))
                .sum();
            let rms = (sum_sq / (2 * win) as f64).sqrt() as f32;
            let dbfs = if rms > 0.0 { 20.0 * rms.log10() } else { -200.0 };
            let peak = sl.iter().chain(sr_w.iter()).map(|x| x.abs()).fold(0.0_f32, f32::max);
            (dbfs, peak)
        };

        // Named measurement windows (sample offset from note-on):
        let attack_start  = (SR * 0.1) as usize; // skip 100 ms bell-mod peak, then 50 ms
        let sustain_start = (SR * 1.0) as usize; // mid-sustain near t ≈ 1 s
        let tail_start    = (SR * 3.5) as usize; // 1.5 s after note-off

        // Assert 1: early-sustain RMS is audible but not clipping.
        let (attack_db, attack_peak) = window_dbfs_peak(&l, &r, attack_start);
        assert!(attack_peak < 1.0, "default patch clipping: peak {attack_peak}");
        // FB 6 sits in a stable zone (scale 0.5), concentrating op6's energy
        // tonally — the early-sustain body lands near -8 dBFS.
        assert!(
            (-24.0..=-8.0).contains(&attack_db),
            "early-sustain RMS {attack_db} dBFS outside [-24, -8]"
        );

        // Assert 2: mid-sustain tail is decaying but still ringing.
        // The default patch is percussive — every carrier's L3 = 0, so the note
        // decays toward silence. The exponential
        // march descends on a constant dB/sec slope and sits low mid-tail
        // (~-40 dBFS) for a given total decay duration. Bound it as a
        // decaying-but-still-ringing tail (below the attack body, above the
        // noise floor) rather than a fixed plateau.
        let (sustain_db, _) = window_dbfs_peak(&l, &r, sustain_start);
        assert!(
            sustain_db < attack_db && sustain_db > -55.0,
            "t≈1s RMS {sustain_db} dBFS not a decaying-but-ringing tail \
             (want below attack {attack_db} dBFS and above -55)"
        );

        // Assert 3: reverb/delay tail has decayed below audibility.
        // AC: ≤ -60 dBFS. Physical floor at 1.5 s past note-off, with reverb
        // decay 2.4 s (RT60) at mix 0.18 plus ping-pong delay tail at 0.30
        // feedback, lands around -53 dBFS — the AC was optimistic about
        // reverb + delay decay overlap. -45 dBFS still bounds the tail well
        // below audibility (≈ 60 dB below a played note) and keeps the
        // patch's FX defaults intact.
        let (tail_db, _) = window_dbfs_peak(&l, &r, tail_start);
        assert!(
            tail_db <= -45.0,
            "tail RMS {tail_db} dBFS at t=3.5 s still audible (want ≤ -45)"
        );
    }

    /// Wiring sanity for the mod matrix. A
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
            scale_src: SourceId::None,
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

    /// Wiring sanity for the per-op phase dests. A matrix slot routing
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
            scale_src: SourceId::None,
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

    /// `voice-spread → global-eg-rate`: on the fresh block the note-on
    /// eval must fold each lane's spread into that lane's cooked amp-EG rates, so
    /// the unison lanes end up with different march rates. Un-routed engines stay
    /// bit-identical.
    #[test]
    fn eg_rate_dest_diverges_lane_rates_on_fresh_block() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        let mut e = Engine::new(SR, BLK);
        e.params.patch.stack.density = 4;
        e.params.patch.stack.spread = 1.0; // full-width VoiceSpread source
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::VoiceSpread,
            dest: DestId::GlobalEgRate,
            depth: 1.0,
            curve: CurveKind::Lin,
            scale_src: SourceId::None,
        };
        e.note_on(60, 100);

        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        e.process_block(&mut l, &mut r); // fresh block resolves + applies the rescale

        let stack = e
            .alloc
            .stacks
            .iter()
            .find(|s| !s.is_idle())
            .expect("no active voice");
        // Linear spread over 4 lanes is symmetric [-1 .. +1], so lane 0 (scale
        // 2^-1) must march slower than lane 3 (scale 2^+1) — a ~4× ratio.
        let slow = stack.core.ops[0].eg[0].rates_per_sec[1];
        let fast = stack.core.ops[0].eg[3].rates_per_sec[1];
        assert!(
            fast > slow * 3.0,
            "lanes did not diverge: slow(lane0)={slow}, fast(lane3)={fast}"
        );

        // A matching engine with no eg-rate route leaves the rates uniform.
        let mut plain = Engine::new(SR, BLK);
        plain.params.patch.stack.density = 4;
        plain.params.patch.stack.spread = 1.0;
        plain.note_on(60, 100);
        plain.process_block(&mut l, &mut r);
        let ps = plain.alloc.stacks.iter().find(|s| !s.is_idle()).unwrap();
        assert_eq!(
            ps.core.ops[0].eg[0].rates_per_sec[1],
            ps.core.ops[0].eg[3].rates_per_sec[1],
            "un-routed engine had non-uniform lane rates"
        );
    }

    /// `voice-spread → pitch-eg-rate`: the fresh-block eval must scale the
    /// per-lane Pitch EG rates, so after the sweep runs the unison lanes sit at
    /// different points (chorusing). Un-routed engines keep the lanes locked.
    #[test]
    fn pitch_eg_rate_dest_decorrelates_lane_sweeps() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};
        use vxn2_dsp::envelope::PitchEgParams;

        let setup = |route: bool| {
            let mut e = Engine::new(SR, BLK);
            e.params.patch.stack.density = 4;
            e.params.patch.stack.spread = 1.0;
            e.params.patch.voice.peg_depth = 12.0;
            // Rise to +12 st then decay back — a sweep that's mid-flight a while.
            e.params.patch.voice.pitch_eg = PitchEgParams { r: [70, 20, 20, 20], l: [99, 0, 0, 0] };
            if route {
                e.matrix.slots[0] = MatrixSlot {
                    source: SourceId::VoiceSpread,
                    dest: DestId::PitchEgRate,
                    depth: 1.0,
                    curve: CurveKind::Lin,
                    scale_src: SourceId::None,
                };
            }
            e.note_on(60, 100);
            let mut l = [0.0_f32; BLK];
            let mut r = [0.0_f32; BLK];
            for _ in 0..12 {
                e.process_block(&mut l, &mut r);
            }
            let s = e.alloc.stacks.iter().find(|s| !s.is_idle()).unwrap();
            (s.meta.pitch_eg[0].level_st, s.meta.pitch_eg[3].level_st)
        };

        let (r0, r3) = setup(true);
        assert!((r0 - r3).abs() > 1e-2, "pitch sweep not decorrelated: l0={r0}, l3={r3}");
        let (u0, u3) = setup(false);
        assert!((u0 - u3).abs() < 1e-6, "un-routed pitch lanes diverged: l0={u0}, l3={u3}");
    }

    /// Level + pan matrix routes ramp to each block's target instead of
    /// stepping at block edges: after every `process_block`
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
            scale_src: SourceId::None,
        };
        e.matrix.slots[1] = MatrixSlot {
            source: SourceId::ModWheel,
            dest: DestId::Op1Pan,
            depth: 1.0,
            curve: CurveKind::Lin,
            scale_src: SourceId::None,
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
        // Replicate the engine's multiplicative projection:
        // `target = clamp(eg·(1+m), 0, 1) − eg`, taken against the op's
        // post-tick EG level, which is what `eg.level` holds after
        // `process_block` returns. The ramp must converge each block on this
        // target — no smoothing.
        let target = |e: &Engine, k: usize| {
            let eg = e.alloc.stacks[slot].core.ops[0].eg[k].level;
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
    /// combined ramp legitimately runs while any EG marches — the default
    /// patch's modulator tails decay for ~10 s — so this test pins every EG
    /// to a flat sustain instead.
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

    /// Multiplicative level mod: a full-depth positive LFO on a
    /// carrier's level must not *refill* a releasing voice. Under multiplicative
    /// semantics the rendered level is `clamp(eg·(1+m), 0, 1)`, so it rides the
    /// EG down — the mod can at most ~double the natural tail (m ≤ +1), never
    /// exceed it structurally.
    ///
    /// The invariant is therefore a *comparison*, not an absolute-silence
    /// threshold: the modulated release tail must not materially exceed the
    /// unmodulated one. The LFO is
    /// routed to `Op3Level`, a carrier still ringing in the measurement window,
    /// so an additive refill would actually show up here.
    #[test]
    fn released_voice_closes_under_positive_level_mod() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        // Peak of the release tail (0.25–0.5 s after note-off), optionally with
        // a full-depth positive LFO on op3's (a carrier) level.
        fn release_tail_peak(with_mod: bool) -> f32 {
            let mut e = Engine::new(SR, BLK);
            e.params.delay.on = false;
            e.params.delay.mix = 0.0;
            e.params.reverb.on = false;
            e.params.reverb.mix = 0.0;
            e.params.mod_params.lfo1.rate_hz = 5.0;
            if with_mod {
                e.matrix.slots[0] = MatrixSlot {
                    source: SourceId::Lfo1,
                    dest: DestId::Op3Level,
                    depth: 1.0,
                    curve: CurveKind::Lin,
                    scale_src: SourceId::None,
                };
            }
            e.note_on(60, 100);
            let mut l = [0.0_f32; BLK];
            let mut r = [0.0_f32; BLK];
            for _ in 0..(SR as usize / 4 / BLK) {
                e.process_block(&mut l, &mut r);
            }
            e.note_off(60);
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
            peak_tail
        }

        let baseline = release_tail_peak(false);
        let modded = release_tail_peak(true);
        // Multiplicative: `modded ≤ ~2·baseline`. 2.5× gives margin without
        // admitting a refill.
        assert!(
            modded <= baseline * 2.5 + 1e-4,
            "positive level mod refilled a releasing voice: modded {modded} vs baseline {baseline}"
        );
    }

    /// Solo-mode note-off with another key held must fall back to the held
    /// note — and it must do so through `Engine::note_off` (the alloc-level
    /// tests call `alloc.note_off` directly and never exercised this path).
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

    /// LFO1 → GlobalPitch at block size 256 must ramp, not step. The
    /// block-rate target jump `|t − s0|` is what the audio would
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
            scale_src: SourceId::None,
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
    /// offset.
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
            scale_src: SourceId::None,
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
            scale_src: SourceId::None,
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

    fn active_stack(e: &Engine) -> usize {
        e.alloc.stacks.iter().position(|s| !s.is_idle()).unwrap()
    }

    /// `voice-rand → lfo2-phase` (depth > 0) scatters the 8 lanes' LFO2
    /// phases — the canonical supersaw-shimmer route. `voice_rand[k]` is
    /// seeded distinct per lane at note-on, so the per-lane offsets decorrelate.
    #[test]
    fn matrix_voice_rand_to_lfo2_phase_decorrelates_lanes() {
        use crate::matrix::{DestId, SourceId};

        let mut e = engine_with_route(SourceId::VoiceRand, DestId::Lfo2Phase, 1.0);
        e.note_on(60, 100);
        // One-block latency: offset applied end of block 1, visible from 2.
        render_blocks(&mut e, 4);
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
        // The default patch ships a `voice-rand → lfo2-phase` slot; clear the
        // table so this asserts the genuine off-path.
        e.matrix = crate::matrix::MatrixTable::default();
        e.note_on(60, 100);
        render_blocks(&mut e, 4);
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
        use crate::matrix::{DestId, SourceId};

        let mut e = engine_with_route(SourceId::VoiceRand, DestId::Lfo2Phase, 1.0);
        e.note_on(60, 100);
        render_blocks(&mut e, 5);
        let a = active_stack(&e);
        let early: [u32; STACK_LANES] = std::array::from_fn(|k| {
            e.alloc.stacks[a].meta.lfo2.phase[k].wrapping_sub(e.alloc.stacks[a].meta.lfo2.phase[0])
        });
        render_blocks(&mut e, 40);
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
        use crate::matrix::{DestId, MatrixTable, SourceId};

        // Clear the default `voice-rand → lfo2-phase` slot so only the
        // patch-global broadcast route is in play.
        let mut modulated = engine_with_route(SourceId::ModWheel, DestId::Lfo2Phase, 0.25);
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
        use crate::matrix::{DestId, SourceId};

        let mut e = engine_with_route(SourceId::VoiceRand, DestId::Lfo2Phase, 1.0);
        e.note_on(60, 100);
        render_blocks(&mut e, 6);
        // Retrigger the same note → reused stack slot hits the `fresh` branch.
        e.note_off(60);
        render_blocks(&mut e, 1);
        e.note_on(60, 100);
        render_blocks(&mut e, 6);
        let a = active_stack(&e);
        // Static offset stable after retrigger (no accumulation across notes).
        let d1: [u32; STACK_LANES] = std::array::from_fn(|k| {
            e.alloc.stacks[a].meta.lfo2.phase[k].wrapping_sub(e.alloc.stacks[a].meta.lfo2.phase[0])
        });
        render_blocks(&mut e, 20);
        let d2: [u32; STACK_LANES] = std::array::from_fn(|k| {
            e.alloc.stacks[a].meta.lfo2.phase[k].wrapping_sub(e.alloc.stacks[a].meta.lfo2.phase[0])
        });
        assert_eq!(d1, d2, "offset unstable after fresh retrigger");
    }

    /// `mod-wheel → lfo1-rate` sweeps LFO1 speed in the log domain: depth 1 ×
    /// mod-wheel 1.0 × gain 4 = +4 octaves → `rate_mult ≈ 16`. Patch-global,
    /// one-block latency, and only live while a voice plays (the accumulator
    /// is aggregated across active stacks like the FX mixes).
    #[test]
    fn matrix_mod_wheel_to_lfo1_rate_sweeps_log_domain() {
        use crate::matrix::{DestId, SourceId};

        let mut e = engine_with_route(SourceId::ModWheel, DestId::Lfo1Rate, 1.0);
        e.set_mod_wheel(1.0);
        e.note_on(60, 100);
        // Block 1 aggregates the offset; block 2 applies it (one-block latency).
        render_blocks(&mut e, 3);
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
        use crate::matrix::{DestId, SourceId};

        let mut e = engine_with_route(SourceId::Velocity, DestId::Lfo2Rate, 1.0);
        e.note_on(60, 30);
        e.note_on(67, 120);
        render_blocks(&mut e, 3);
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
        render_blocks(&mut e, 4);
        assert_eq!(e.patch_mod.lfo1.rate_mult, 1.0);
        let a = active_stack(&e);
        assert_eq!(e.alloc.stacks[a].meta.lfo2.rate_mult, 1.0);
    }

    /// Self-rate feedback (`lfo1 → lfo1-rate`) is
    /// well-defined under the one-block latency — it must stay finite and
    /// bounded by the Hz clamp, never run away or NaN.
    #[test]
    fn matrix_lfo1_self_rate_feedback_is_bounded() {
        use crate::matrix::{DestId, SourceId};

        let mut e = engine_with_route(SourceId::Lfo1, DestId::Lfo1Rate, 1.0);
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        for _ in 0..200 {
            e.process_block(&mut l, &mut r);
            let m = e.patch_mod.lfo1.rate_mult;
            assert!(m.is_finite() && m > 0.0, "lfo1 rate_mult diverged: {m}");
        }
    }

    /// `pitch-eg → global-pitch` does not double-scale: a full-scale EG at
    /// unity depth reaches ±24 st (the dest's gain), NOT `peg_depth × 24`. With
    /// `peg_depth = 2`, the normalized source gives 24 st, not 48.
    #[test]
    fn matrix_pitch_eg_into_pitch_no_double_scale() {
        use crate::matrix::{DestId, SourceId};

        let gp_idx = DestId::GlobalPitch.idx().unwrap();
        let run = |peg_l: i8| -> f32 {
            let mut e = engine_with_route(SourceId::PitchEg, DestId::GlobalPitch, 1.0);
            // Full-scale EG, fast rates, and a 2-semitone configured swing.
            e.params.patch.voice.peg_depth = 2.0;
            e.params.patch.voice.pitch_eg.l = [peg_l, peg_l, peg_l, peg_l];
            e.params.patch.voice.pitch_eg.r = [99, 99, 99, 99];
            e.note_on(60, 100);
            render_blocks(&mut e, 40);
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
        use crate::matrix::{DestId, SourceId};

        // Route into a gain-1 dest (op1-pan) so the dest accumulator reads the
        // raw source shape, undistorted by a gain.
        let pan_idx = DestId::Op1Pan.idx().unwrap();
        let mut e = engine_with_route(SourceId::PitchEg, DestId::Op1Pan, 1.0);
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
            scale_src: SourceId::None,
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
            scale_src: SourceId::None,
        };
        modulated.matrix.slots[1] = MatrixSlot {
            source: SourceId::VoiceSpread,
            dest: DestId::Op1Pan,
            depth: 1.0,
            curve: CurveKind::Lin,
            scale_src: SourceId::None,
        };
        modulated.note_on(60, 120);

        let mut baseline = stacked_engine();
        baseline.matrix.slots[1] = MatrixSlot {
            source: SourceId::VoiceSpread,
            dest: DestId::Op1Pan,
            depth: 1.0,
            curve: CurveKind::Lin,
            scale_src: SourceId::None,
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
        render_blocks(&mut e, 4);
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
            scale_src: SourceId::None,
        };
        e.note_on(36, 100);
        e.note_on(96, 100);
        render_blocks(&mut e, 3);
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
            scale_src: SourceId::None,
        };
        e.set_mod_wheel(0.0);
        e.note_on(60, 100);
        render_blocks(&mut e, 4);
        let a = active_stack(&e);
        assert_eq!(e.stack_detune_mod[a], 0.0, "should settle at 0 with wheel down");
        // Jump the wheel mid-note: the amount must ramp, not snap.
        e.set_mod_wheel(1.0);
        render_blocks(&mut e, 1);
        let after_one = e.stack_detune_mod[a];
        assert!(
            after_one > 0.0 && after_one < 1.0,
            "dynamic detune not ramped (got {after_one}, expected partway to 1.0)"
        );
        render_blocks(&mut e, 20);
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
            scale_src: SourceId::None,
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
            scale_src: SourceId::None,
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
                scale_src: 0,
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

    /// Loading a new preset (a bumped [`SharedParams::load_epoch`]) must silence
    /// any voice still ringing from the previous patch, so its operators can't
    /// sound through the new — possibly re-roled (modulator → carrier) —
    /// algorithm. A same-epoch snapshot (ordinary automation) must NOT.
    #[test]
    fn preset_load_silences_ringing_voices() {
        let any_sounding = |e: &Engine| {
            e.alloc
                .stacks
                .iter()
                .any(|s| {
                    s.core
                        .ops
                        .iter()
                        .any(|op| op.eg.iter().any(|e| e.level > 1e-4))
                })
        };

        let shared = SharedParams::new();
        let mut e = Engine::new(SR, BLK);
        e.snapshot_params(&shared);
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        for _ in 0..16 {
            e.process_block(&mut l, &mut r);
        }
        assert!(any_sounding(&e), "voice should be sounding before preset load");

        // Simulate a preset load: bump the shared store's load-epoch. The next
        // per-block snapshot must silence the held voice outright.
        shared.reset_to_defaults();
        e.snapshot_params(&shared);
        assert!(
            !any_sounding(&e),
            "all operators must be silenced on the snapshot after a preset load"
        );

        // A fresh note under the new preset, followed by an ordinary same-epoch
        // snapshot, must survive — the silence fires only on an epoch change.
        e.note_on(64, 100);
        for _ in 0..16 {
            e.process_block(&mut l, &mut r);
        }
        e.snapshot_params(&shared);
        assert!(
            any_sounding(&e),
            "a same-epoch snapshot must not silence sounding voices"
        );
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
    /// actually selects the level→amplitude mapping.
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
            scale_src: 0,
        };
        e.params.mtx_depths[0] = 0.5;
        // Non-pitch dest at the same depth: passthrough.
        e.params.matrix_rows[1] = MatrixRowRaw {
            source: SourceId::Lfo1 as u8,
            dest: DestId::Op1Level as u8,
            curve: 0,
            active: true,
            depth: 0.0,
            scale_src: 0,
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
            scale_src: 0,
        };
        e.apply_block_params();

        assert!((e.matrix.slots[0].depth - 0.125).abs() < 1e-7);
        assert!((e.matrix.slots[1].depth - 0.5).abs() < 1e-7);
        assert!((e.matrix.slots[hi].depth - -0.125).abs() < 1e-7);
    }

    /// A live algo change declick-kills a *releasing* voice (so a former
    /// modulator promoted to carrier can't ring out its long release tail), but
    /// leaves a *held* voice gated to re-route and morph.
    #[test]
    fn live_algo_change_declicks_releasing_voice_not_held() {
        use vxn2_dsp::stack::VoicePhase;

        let mut e = Engine::new(SR, BLK);
        // Hot, sustaining ops with a slow release so a released note stays
        // clearly non-idle for the block where we flip the algorithm.
        for op in &mut e.params.patch.voice.ops {
            op.level = 99;
            op.eg.l = [99, 99, 99, 0];
            op.eg.r = [99, 99, 99, 40]; // slow release tail
        }
        e.params.patch.voice.algo = 1;
        e.apply_block_params();

        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];

        // Voice A: play then release → releasing (gate off, not idle).
        e.note_on(60, 100);
        e.process_block(&mut l, &mut r);
        e.note_off(60);
        e.process_block(&mut l, &mut r);
        let a = e
            .alloc
            .stacks
            .iter()
            .position(|s| s.meta.note == 60 && !s.is_idle())
            .expect("voice A should be releasing, not idle");
        assert!(!e.alloc.stacks[a].meta.gate, "voice A is released");
        assert_ne!(
            e.alloc.stacks[a].meta.phase,
            VoicePhase::Declick,
            "voice A not declicking before the algo change"
        );

        // Voice B: held (gated, key down).
        e.note_on(72, 100);
        e.process_block(&mut l, &mut r);
        let b = e
            .alloc
            .stacks
            .iter()
            .position(|s| s.meta.note == 72 && s.meta.gate)
            .expect("voice B should be held");

        // Live algo change (picker move, not a preset load).
        e.params.patch.voice.algo = 2;
        e.apply_block_params();

        assert_eq!(
            e.alloc.stacks[a].meta.phase,
            VoicePhase::Declick,
            "releasing voice must be declick-killed on the algo change"
        );
        assert!(
            e.alloc.stacks[b].meta.gate,
            "held voice keeps gating (re-routes/morphs, not cut)"
        );
        assert_ne!(
            e.alloc.stacks[b].meta.phase,
            VoicePhase::Declick,
            "held voice must NOT be declicked"
        );
    }

    #[test]
    #[ignore = "diagnostic-only: prints per-algo feedback response then panics; run with --ignored"]
    fn diag_engine_feedback_response_per_algo() {
        for algo in [1u8, 3, 4, 5, 6] {
            let mut rms = [0.0f64; 2];
            for (fi, &fb) in [0.0f32, 7.0].iter().enumerate() {
                let mut e = Engine::new(SR, BLK);
                e.params.patch.voice.algo = algo;
                e.params.patch.voice.feedback = fb;
                // Hot, sustaining ops: level 99, EG sustain L3=99, ratio 1:1.
                for op in &mut e.params.patch.voice.ops {
                    op.level = 99;
                    op.num = 1; op.denom = 1; op.detune = 0; op.fine = 0;
                    op.eg.l = [99, 99, 99, 0];
                    op.eg.r = [99, 99, 99, 60];
                }
                e.apply_block_params();
                e.note_on(60, 100);
                let mut l = [0.0f32; BLK];
                let mut r = [0.0f32; BLK];
                for _ in 0..20 { e.process_block(&mut l, &mut r); }
                if let Some(s) = e.alloc.stacks.iter().position(|s| !s.is_idle()) {
                    let spec = vxn2_dsp::algo::spec_of(e.alloc.stacks[s].meta.algo);
                    let fbop = spec.fb_src as usize - 1;
                    println!("  algo {algo} fb={fb} -> fb_src=op{} fb_scale[0]={:.4} op_eg_level[fb_src][0]={:.4}",
                        spec.fb_src, e.alloc.stacks[s].core.ops[fbop].fb_scale[0], e.alloc.stacks[s].core.op_eg_level[fbop][0]);
                }
                let mut acc = 0.0f64;
                for _ in 0..40 {
                    e.process_block(&mut l, &mut r);
                    for &s in l.iter() { acc += (s as f64) * (s as f64); }
                }
                rms[fi] = (acc / (40.0 * BLK as f64)).sqrt();
            }
            let delta = (rms[1] - rms[0]).abs();
            let pct = if rms[0] > 1e-9 { delta / rms[0] * 100.0 } else { f64::INFINITY };
            println!("ENGINE ALGO {algo:2}: fb0={:.5} fb7={:.5} delta={:.5} ({pct:.1}%)", rms[0], rms[1], delta);
        }
        panic!("diag");
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
            scale_src: SourceId::None,
        };
        e.set_mod_wheel(1.0);
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        e.process_block(&mut l, &mut r);

        let s = e.alloc.stacks.iter().position(|s| !s.is_idle()).unwrap();
        let fb_op = spec_of(e.alloc.stacks[s].meta.algo).fb_src as usize;
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
            scale_src: SourceId::None,
        };
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        e.process_block(&mut l, &mut r);

        let s = e.alloc.stacks.iter().position(|s| !s.is_idle()).unwrap();
        let fb_op = spec_of(e.alloc.stacks[s].meta.algo).fb_src as usize;
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
                scale_src: 0,
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
                scale_src: SourceId::None,
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

    /// A mod-wheel scale source gates a live LFO→pitch route through the
    /// full `apply_block_params` → `eval_dests` pipeline. At wheel 0 the
    /// route's per-lane pitch contribution is exactly zero; render stays
    /// finite. (Slot 8 is patch-state depth, so `row.depth` applies directly
    /// rather than the CLAP-automatable slot-1..8 depth.)
    #[test]
    fn scale_source_gates_route_end_to_end() {
        use crate::matrix::{CurveKind, DestId, SourceId};
        use crate::shared::MatrixRowRaw;
        let mut e = Engine::new(SR, BLK);
        e.params.matrix_rows[8] = MatrixRowRaw {
            source: SourceId::Lfo1 as u8,
            dest: DestId::GlobalPitch as u8,
            curve: CurveKind::Lin as u8,
            active: true,
            depth: 1.0,
            scale_src: SourceId::ModWheel as u8,
        };
        e.mod_wheel = 0.0;
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        e.process_block(&mut l, &mut r);
        for i in 0..BLK {
            assert!(l[i].is_finite() && r[i].is_finite());
        }
        let st = active_stack(&e);
        let gi = DestId::GlobalPitch.idx().unwrap();
        for k in 0..STACK_LANES {
            assert_eq!(
                e.dest_vals[st][k][gi], 0.0,
                "wheel 0 must gate the vibrato route to silence (lane {k})"
            );
        }
    }

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
            scale_src: SourceId::None,
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
            scale_src: SourceId::None,
        };
        // Per-op pitch on op4, gain 24, but cubic-tapered depth 1.0 → 1.0.
        e.matrix.slots[1] = MatrixSlot {
            source: SourceId::ModWheel,
            dest: DestId::Op4Pitch,
            depth: 1.0,
            curve: CurveKind::Lin,
            scale_src: SourceId::None,
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
    /// unchanged and the masks are byte-identical.
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

    // Render-level ratio-lock + shared-modulator spread. Wall-split,
    // fixed-target no-op, and recook gating are asserted above (they exercise
    // the same cook + render path). These two add the frequency-domain
    // ratio-lock proof and the shared-modulator case.

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
                scale_src: SourceId::None,
            };
            e.mod_wheel = 0.5;
        }
        e.note_on(60, 100);
        render_blocks(&mut e, 80);
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
