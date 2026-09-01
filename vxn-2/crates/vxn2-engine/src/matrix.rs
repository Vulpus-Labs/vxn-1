//! Mod matrix engine — the central modulation router.
//!
//! Per ADR §6 this is the **only** mechanism for dynamic parameter modulation
//! in VXN2; no hard-wired routes. The patch holds a fixed 16-slot
//! [`MatrixTable`] of `MatrixSlot { source, dest, depth, curve }`.
//!
//! ## Source granularity
//!
//! Sources split into three strides:
//!
//! - **Patch-global** ([`PatchSources`]): `lfo1`, `mod_wheel`, `aftertouch`.
//!   One scalar per patch, broadcast across all stacks and lanes.
//! - **Per-stack** ([`StackScalarSources`]): `pitch_eg`, `mod_env`,
//!   `velocity`, `key`. One scalar per played stack, broadcast across lanes.
//! - **Per-lane** ([`LaneSources`]): `lfo2`, `voice_idx`, `voice_spread`,
//!   `voice_rand`. One value per lane of the 8-lane stack.
//!
//! [`eval_sources`] fans these out into a single `[[f32; STACK_LANES];
//! N_SOURCES]` lookup table per stack — the slot eval inner loop reads from
//! one contiguous matrix regardless of source kind. Broadcast cost is paid
//! once per block at the source-eval site, never inside the per-slot loop.
//!
//! ## Destination application
//!
//! Two tiers per ADR §6 Consequences:
//!
//! - **Per block** ([`eval_dests`] writes into [`LaneDestVals`]): every
//!   non-zipper-sensitive destination is summed into a per-lane accumulator
//!   once per control block. Engine reads the accumulator at block start and
//!   applies it before the per-sample render.
//! - **Sub-block** ([`PitchSmoother`]): pitch-shaped destinations (global
//!   pitch, per-op pitch, lfo2_phase) get one-pole smoothing from the block
//!   accumulator down to a 16-sample quantum (engine's
//!   `PITCH_SMOOTH_QUANTUM`) so the audio loop sees a ramp, not a step.
//!   True per-sample smoothing would re-cook every op's `phase_inc`
//!   (48 `powf` per stack) each sample; at the quantum a 256-sample host
//!   block gets 16 interpolation points, which removes audible stepping.
//!   Time constant matches one control block — same idiom as VXN1's
//!   [`vxn2_dsp::smoother::Smoothed`].
//!
//! ## Granularity tiers & coherence
//!
//! Every source and dest has a [`Tier`] — `PatchGlobal` (1 value/patch),
//! `PerStack` (1/voice), or `PerLane` (1/unison lane):
//!
//! | Tier | Sources | Destinations |
//! |---|---|---|
//! | patch-global | `lfo1`, `mod-wheel`, `aftertouch` | `lfo1-rate`, `delay-mix`, `reverb-mix` |
//! | per-stack | `pitch-eg`, `mod-env`, `velocity`, `key` | `lfo2-rate`, `stack-detune`, `stack-spread`, `cutoff`, `resonance` |
//! | per-lane | `lfo2`, `voice-idx`, `voice-spread`, `voice-rand` | `op{1..6}-{pitch,level,pan}`, `global-pitch`, `feedback`, `lfo2-phase` |
//!
//! A routing is **coherent** iff the source tier is coarser-or-equal to the
//! dest tier — a coarser source broadcasts unambiguously to a finer dest; a
//! finer source into a coarser dest is a lossy collapse to lane 0. Plus two
//! special cases: an LFO into its own rate ([`Coherence::SelfRate`]) and
//! `voice-idx` into a lane-0-collapsed dest ([`Coherence::Degenerate`],
//! constant 0). [`coherence`] is the canonical predicate; it is exported in
//! the matrix descriptor so the UI flags incoherent rows without re-deriving
//! the rule.
//!
//! ## Inner-loop shape
//!
//! Per-slot inner loops walk 8 lanes. Every per-slot decision — curve
//! polarity, curve shape, scale-source polarity, scale bend — is dispatched
//! *outside* the lane loop, so each arm's body is straight-line FMA + add with
//! no branch or match per lane. Letting one of those matches ride inside the
//! loop is expensive: `scale_norm` was originally called per lane, and hoisting
//! its two decisions into the arms below cut a fully-scaled 16-slot eval by
//! ~47% (253 ns → 133 ns, `matrix_eval_scaled`).
//!
//! Both buffers are stored **transposed** — [`LaneSourceVals`] source-major,
//! [`LaneDestVals`] dest-major — so a slot's source row and dest row are each
//! a contiguous `[f32; STACK_LANES]` and each curve arm's accumulate compiles
//! to a `ldr q` / `fmul.4s` / `fadd.4s` / `str q` over two vectors, not a
//! gather-scatter (ticket 0328). Under the old lane-major layout
//! `sources[k][si]` and `out[k][di]` strided a whole row per lane and the
//! accumulate stayed scalar. Measured post-LTO on the linked `matrix` bench
//! binary, lane-major → transposed: 895 → 569 instructions, 383 → 112 scalar
//! FP ops, 23 → 74 `.4s` ops — and where every vector op used to live in the
//! *scale-VCA* loop (the one loop already walking a contiguous local), the
//! curve arms now carry `fabs.4s` / `fneg.4s` / `fcmlt.4s` of their own.
//! Straight-line-per-lane is worth keeping on top of that, for the
//! branch-prediction and code-size win.
//!
//! **Measure this post-LTO or not at all.** `cargo rustc --emit asm` on this
//! crate runs *no* loop vectoriser — with `lto` set, cargo passes
//! `-C linker-plugin-lto` and the pipeline is deferred to link time, so even a
//! trivially vectorisable loop shows up scalar. Use `llvm-objdump` on a linked
//! artifact. An earlier revision of this note asserted "autovectorises to NEON",
//! a later one asserted the exact opposite; both were written from unlinked
//! per-crate asm, and neither was evidence.
//!
//! ## CLAP exposure
//!
//! Slots 1–8 `depth` are CLAP-automatable; slots 9–16 `depth` and all slot
//! `source` / `dest` / `curve` fields are patch state only. Topology
//! (source/dest/curve) isn't a continuous control. See
//! [`N_CLAP_DEPTH_SLOTS`]. Slot depth, even when CLAP-automatable, is treated
//! as a per-block constant by the matrix engine — matrix-routing a slot's
//! depth via the matrix itself isn't supported in v1 (sidesteps cycle
//! detection).

use vxn2_dsp::smoother::one_pole_coeff;
use vxn2_dsp::stack::STACK_LANES;
use vxn_core_matrix::matrix_enum;

use crate::modulation::ModBlock;

/// The curve-shaping vocabulary, re-exported from [`vxn_core_matrix::curve`] so
/// that `crate::matrix::Shape` keeps meaning what it always did.
///
/// Both axes, their name/label tables, the flat wire codec and the scale VCA
/// live in the shared crate as of ticket 0330 (epic E049). VXN1b's matrix is a
/// hand-port of this one and had picked up its own copy of all of it 96 minutes
/// after this one was written; what stays here is the roster — which sources and
/// destinations *this* synth can route, and what they mean.
///
/// The two enums were `ShapeKind` / `PolarityKind` here and `Shape` / `Polarity`
/// in vxn-1b. The shorter names won: there is no other `Shape` in this crate to
/// disambiguate from (the LFO and ADSR shapes are param enums in `params`, not
/// types), and the `Kind` suffix was carrying no information.
pub use vxn_core_matrix::curve::{
    CURVE_LABELS, CURVE_NAMES, N_CURVES, N_POLARITIES, N_SHAPES, POLARITY_LABELS, POLARITY_NAMES,
    Polarity, SHAPE_LABELS, SHAPE_NAMES, Shape, curve_code, curve_split, scale_norm,
};

/// How a destination's summed total is moved from one control block's value to
/// the next — the `smooth =` column of a roster row, re-exported from
/// [`vxn_core_matrix::roster`].
///
/// Only [`Smoothing::QuantumCascade`] is read today, to derive [`PITCH_DESTS`].
/// The rest is declaration ahead of its consumer: the shared smoother bank
/// ([0335](../../../../tickets/open/0335-declared-target-smoothing.md)) is what
/// turns the other classes into behaviour, and declaring them here first means
/// that ticket is a consumer change rather than a data-entry pass over 51 rows.
pub use vxn_core_matrix::roster::Smoothing;

/// Slot count per patch. ADR §6 sets this at 16 for v1.
pub const N_SLOTS: usize = 16;

/// Number of CLAP-automatable depth slots (slots 1..=N). Slots past this
/// count are patch-state only.
pub const N_CLAP_DEPTH_SLOTS: usize = 8;

/// Granularity tier of a source or destination — how many independent values it
/// carries per patch, re-exported from [`vxn_core_matrix::roster`] so that
/// `crate::matrix::Tier` keeps meaning what it always did.
///
/// This crate had its own three-variant copy, identical in variant names and
/// discriminants, written before the shared crate existed. A source's and a
/// destination's tier are now columns on their roster rows (0332 for dests,
/// 0336 for sources) and the generated `tier()` returns the shared type, so
/// keeping a local duplicate would mean a conversion at every use for no gain.
///
/// The coarseness order is the discriminant order and [`coherence`] depends on
/// it: a routing is **coherent** iff the source tier is coarser-or-equal to the
/// dest tier — a coarser source broadcasts unambiguously to a finer dest; a
/// finer source into a coarser dest is a lossy collapse (which lane wins?).
pub use vxn_core_matrix::roster::Tier;

/// The coherence verdict vocabulary, re-exported from
/// [`vxn_core_matrix::coherence`] so that `crate::matrix::Coherence` keeps
/// meaning what it always did — including for the descriptor export, whose
/// `name()` strings are the faceplate's contract.
///
/// This crate wrote the enum, and 0336 moved it: the tier rule behind it is
/// arithmetic on two declared columns and was never FM-specific. What stays
/// here is [`Vxn2Coherence`] — the two special cases, which name *this* synth's
/// variants and are deliberately not shared (vxn-1b runs an
/// `lfo1 → lfo1-rate` route on purpose).
pub use vxn_core_matrix::coherence::Coherence;

/// vxn-2's coherence roster: the declared tiers, plus the two special cases the
/// generic tier rule does not cover.
///
/// A marker type rather than a function, because the shared predicate owns the
/// *shape* — the empty-slot short circuit and the precedence of a special case
/// over the tier rule — and each synth fills in only the holes. See
/// [`vxn_core_matrix::coherence`] for why the hook is per-synth at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Vxn2Coherence;

impl vxn_core_matrix::coherence::CoherenceRoster for Vxn2Coherence {
    type Source = SourceId;
    type Dest = DestId;

    #[inline]
    fn source_tier(src: SourceId) -> Option<Tier> {
        match src {
            SourceId::None => None,
            _ => Some(src.tier()),
        }
    }

    #[inline]
    fn dest_tier(dst: DestId) -> Option<Tier> {
        match dst {
            DestId::None => None,
            _ => Some(dst.tier()),
        }
    }

    /// Checked before the tier rule, so each gets the more specific tooltip
    /// even where the tiers would also flag a collapse.
    #[inline]
    fn special_case(src: SourceId, dst: DestId) -> Option<Coherence> {
        // Self-rate: an LFO into its own rate. Tier-legal (both the same tier)
        // but self-referential — this synth's LFOs are ticked from the rate the
        // route is trying to set, within the same block.
        if matches!(
            (src, dst),
            (SourceId::Lfo1, DestId::Lfo1Rate) | (SourceId::Lfo2, DestId::Lfo2Rate)
        ) {
            return Some(Coherence::SelfRate);
        }
        // Degenerate: `voice_idx[0]` is always 0 ([`vxn2_dsp::stack`]), so
        // routing it into a dest that collapses to lane 0 is a constant zero at
        // any depth — "no effect" is more use to a player than "lossy".
        if src == SourceId::VoiceIdx
            && matches!(
                dst,
                DestId::Cutoff
                    | DestId::Resonance
                    | DestId::FilterDrive
                    | DestId::DelayMix
                    | DestId::ReverbMix
            )
        {
            return Some(Coherence::Degenerate);
        }
        None
    }
}

/// Coherence verdict for a `source → dest` routing — [`Vxn2Coherence`] through
/// the shared predicate, kept as a free function because that is what every
/// call site in this crate and the faceplate descriptor already says.
///
/// Empty slots (`None` source or dest) are always [`Coherence::Ok`].
#[inline]
pub fn coherence(src: SourceId, dst: DestId) -> Coherence {
    vxn_core_matrix::coherence::coherence::<Vxn2Coherence>(src, dst)
}

/// The dense `[srcWireId][dstWireId]` verdict table the faceplate descriptor
/// carries, as [`Coherence::name`] strings — sentinel row and column included,
/// so the page looks a verdict up with the same `u8` its pick-lists carry.
///
/// Built here rather than in the UI crate so the index space is decided once,
/// on the side that owns the enums.
pub fn coherence_name_grid() -> Vec<Vec<&'static str>> {
    vxn_core_matrix::coherence::coherence_name_grid::<Vxn2Coherence>(&SourceId::ALL, &DestId::ALL)
}

matrix_enum! {
    /// Modulation source. `None` is the "empty slot" sentinel — slots whose
    /// source is `None` skip evaluation cheaply.
    ///
    /// The `uni` / `bi` column is this source's own polarity, which
    /// [`scale_norm`] folds a scale source by. It is not optional, so a new
    /// source forces a polarity decision at compile time and cannot drift from
    /// the row it belongs to.
    SourceId, fallback = None, names = SOURCE_NAMES,
    labels = SOURCE_LABELS, roster_names = ROSTER_SOURCE_NAMES,
    roster_labels = ROSTER_SOURCE_LABELS, polarity;
    sentinel None = 0, "none", "—";
    Lfo1 = 1, "lfo1", "LFO 1", bi, tier = patch_global;
    Lfo2 = 2, "lfo2", "LFO 2", bi, tier = per_lane;
    PitchEg = 3, "pitch-eg", "Pitch EG", bi, tier = per_stack;
    ModEnv = 4, "mod-env", "Mod Env", uni, tier = per_stack;
    ModWheel = 5, "mod-wheel", "Mod Wheel", uni, tier = patch_global;
    Aftertouch = 6, "aftertouch", "Aftertouch", uni, tier = patch_global;
    Velocity = 7, "velocity", "Velocity", uni, tier = per_stack;
    Key = 8, "key", "Key", uni, tier = per_stack;
    VoiceIdx = 9, "voice-idx", "Voice Idx", uni, tier = per_lane;
    VoiceSpread = 10, "voice-spread", "Voice Spread", bi, tier = per_lane;
    /// Per-lane note-on random. `[0, 1)` and therefore **unipolar**: treating
    /// it as bipolar would compress the random into `[0.5, 1)` and it could
    /// never gate a route to zero.
    VoiceRand = 11, "voice-rand", "Voice Rand", uni, tier = per_lane;
}

/// Count of non-sentinel sources (i.e. `SourceId::None` excluded). Derived from
/// the generated table, so adding a row cannot leave it stale.
pub const N_SOURCES: usize = SOURCE_NAMES.len() - 1;

impl SourceId {
    /// Index into the per-lane source lookup, or `None` for the sentinel.
    #[inline]
    pub const fn idx(self) -> Option<usize> {
        match self {
            SourceId::None => None,
            _ => Some(self as usize - 1),
        }
    }
}

matrix_enum! {
    /// Modulation destination. `None` is the "empty slot" sentinel.
    ///
    /// Per-op dests are laid out in op-major order (`op1_*` block, then `op2_*`,
    /// …). 6 ops × 3 dests each = 18 op dests. Plus 4 global, 2 stack-macro,
    /// 2 FX, a single `Feedback` dest, plus 2 filter dests (`Cutoff`,
    /// `Resonance`), plus 6 per-op stack-pitch dests (`OpNStackPitch`). Feedback
    /// modulates the algorithm's structural FB op only, but applies per lane —
    /// it's a voice property, unlike the post-mixdown FX dests.
    ///
    /// Appended dests (stack-pitch, phase, filter-drive, eg-rate) sit past
    /// `Resonance` so the blob dest space stays a 1:1 prefix for older patches.
    ///
    /// ## Audio wiring status
    ///
    /// Live (consumed by [`crate::engine::Engine::process_block`]):
    /// - `Op{1..6}Level` — additive per-lane offset on EG level pre-sine.
    /// - `Op{1..6}Pitch` — per-lane semitones added to the op pitch sum before
    ///   `phase_inc` recompute.
    /// - `Op{1..6}Pan` — added to the equal-power pan curve per lane.
    /// - `GlobalPitch` — per-lane semitones added to the stack pitch sum.
    /// - `DelayMix` / `ReverbMix` — averaged at lane 0 across active stacks
    ///   and pushed to the FX param surface each block.
    /// - `Feedback` — per-lane: each lane's accumulated amount is added to the
    ///   patch feedback and cooked via `set_feedback_live_lanes`, so per-lane
    ///   sources (VoiceSpread, LFO2, …) give each unison lane its own growl.
    ///
    /// Live (continued):
    /// - `Lfo2Phase` — per-lane LFO2 phase offset. The smoothed per-lane value is
    ///   applied as a wrapping Q32 phase add to each stack's LFO2 before its
    ///   next-block `eval` (one-block latency). `voice-rand → lfo2-phase` is the
    ///   canonical supersaw-shimmer route.
    /// - `Lfo1Rate` (patch-global) / `Lfo2Rate` (per-stack) — log-domain rate
    ///   offset: the accumulator is in *octaves*, applied as `rate · 2^oct`.
    ///   Computed from the previous block's accumulator (one-block latency) to
    ///   sidestep rate-on-self ordering, and gated so an un-targeted dest leaves
    ///   the LFO tick bit-identical.
    /// - `StackDetune` (per-stack) — scales the per-lane note-on detune by
    ///   `(1 + mod)`, folded into the block-rate `apply_pitch_mult` recompute.
    ///   Fresh notes snap; dynamic motion is one-pole smoothed.
    /// - `StackSpread` (per-stack) — scales the `VoiceSpread` matrix source's
    ///   width by `(1 + mod)` (one-block latency).
    ///
    /// Routable in the matrix UI but NOT yet consumed in audio:
    /// - `Cutoff` / `Resonance` — the optional per-voice filter dests. Both
    ///   collapse to a per-stack scalar (lane-0). `Cutoff` is in octaves (log
    ///   domain); `Resonance` is an additive `[0, 1]` offset.
    /// ## Reading a row
    ///
    /// `gain` converts the normalised `[-1, 1]` route product into the dest's
    /// own unit, so a fixed depth is musically comparable across dest kinds:
    /// 24.0 = ±24 semitones (±2 oct), 4.0 = ±4 octaves in a log domain
    /// (`x · 2^v`), 7.0 = the 0..7 feedback clamp, 1.0 = the dest's own natural
    /// full scale (a pan sweep, a `[0, 1]` mix offset, ±1 cycle of phase).
    ///
    /// `taper` is `cubic` on the 13 semitone dests and `linear` everywhere
    /// else — the log-domain rate/cutoff dests and the `[-1, 1]`-scale stack
    /// macros have a gain that is *already* log/ratio-shaped, so a depth taper
    /// would double-bend the response. Note the taper set and the
    /// `quantum_cascade` set overlap without coinciding: `Lfo2Phase` smooths
    /// but is linear (gain 1.0), and the six stack-pitch dests taper but are
    /// not smoothed.
    ///
    /// `smooth` is the class the *shared* smoother bank applies, not an
    /// inventory of every motion this engine applies to a dest. Several dests
    /// declare `block` and then move engine-side after the matrix — see the
    /// comments on their rows and ADR 0003 §3.
    DestId, fallback = None, names = DEST_NAMES,
    labels = DEST_LABELS, roster_names = ROSTER_DEST_NAMES,
    roster_labels = ROSTER_DEST_LABELS, roster_gains = ROSTER_DEST_GAIN;
    sentinel None = 0, "none", "—";
    // `op{n}-level` / `op{n}-pan` declare `block` and then **ramp per-sample,
    // linearly**, to each block's target in the engine's target application
    // (ADR 0003 §3). That motion is not a smoother in the bank's sense and
    // moving it into the bank would be a behaviour change — do not "fix" the
    // column to match what the render does.
    Op1Pitch = 1, "op1-pitch", "Op 1 Pitch", gain = 24.0, taper = cubic,
        tier = per_lane, smooth = quantum_cascade;
    Op1Level = 2, "op1-level", "Op 1 Level", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    Op1Pan = 3, "op1-pan", "Op 1 Pan", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    Op2Pitch = 4, "op2-pitch", "Op 2 Pitch", gain = 24.0, taper = cubic,
        tier = per_lane, smooth = quantum_cascade;
    Op2Level = 5, "op2-level", "Op 2 Level", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    Op2Pan = 6, "op2-pan", "Op 2 Pan", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    Op3Pitch = 7, "op3-pitch", "Op 3 Pitch", gain = 24.0, taper = cubic,
        tier = per_lane, smooth = quantum_cascade;
    Op3Level = 8, "op3-level", "Op 3 Level", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    Op3Pan = 9, "op3-pan", "Op 3 Pan", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    Op4Pitch = 10, "op4-pitch", "Op 4 Pitch", gain = 24.0, taper = cubic,
        tier = per_lane, smooth = quantum_cascade;
    Op4Level = 11, "op4-level", "Op 4 Level", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    Op4Pan = 12, "op4-pan", "Op 4 Pan", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    Op5Pitch = 13, "op5-pitch", "Op 5 Pitch", gain = 24.0, taper = cubic,
        tier = per_lane, smooth = quantum_cascade;
    Op5Level = 14, "op5-level", "Op 5 Level", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    Op5Pan = 15, "op5-pan", "Op 5 Pan", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    Op6Pitch = 16, "op6-pitch", "Op 6 Pitch", gain = 24.0, taper = cubic,
        tier = per_lane, smooth = quantum_cascade;
    Op6Level = 17, "op6-level", "Op 6 Level", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    Op6Pan = 18, "op6-pan", "Op 6 Pan", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    GlobalPitch = 19, "global-pitch", "Global Pitch", gain = 24.0, taper = cubic,
        tier = per_lane, smooth = quantum_cascade;
    Lfo1Rate = 20, "lfo1-rate", "LFO 1 Rate", gain = 4.0, taper = linear,
        tier = patch_global, smooth = block;
    Lfo2Rate = 21, "lfo2-rate", "LFO 2 Rate", gain = 4.0, taper = linear,
        tier = per_stack, smooth = block;
    // Smoothed by the same cascade as the pitch dests (a phase offset stepping
    // at a block edge clicks the same way), but **linear** and gain 1.0: it is
    // a fraction of an LFO cycle, not semitones, so it takes no cubic taper.
    Lfo2Phase = 22, "lfo2-phase", "LFO 2 Phase", gain = 1.0, taper = linear,
        tier = per_lane, smooth = quantum_cascade;
    // `stack-detune` / `stack-spread` declare `block` and then take a
    // **block-rate one-pole** engine-side (`STACK_MACRO_SMOOTH`, snap-on-fresh)
    // folded into the pitch-mult recompute — target application, not routing
    // (ADR 0003 §3). Their gain is 1.0 because both are multiplicative scale
    // factors `(1 + depth·shape)`: depth 1 doubles the macro (0→2×).
    StackDetune = 23, "stack-detune", "Stack Detune", gain = 1.0, taper = linear,
        tier = per_stack, smooth = block;
    StackSpread = 24, "stack-spread", "Stack Spread", gain = 1.0, taper = linear,
        tier = per_stack, smooth = block;
    DelayMix = 25, "delay-mix", "Delay Mix", gain = 1.0, taper = linear,
        tier = patch_global, smooth = block;
    ReverbMix = 26, "reverb-mix", "Reverb Mix", gain = 1.0, taper = linear,
        tier = patch_global, smooth = block;
    Feedback = 27, "feedback", "Feedback", gain = 7.0, taper = linear,
        tier = per_lane, smooth = block;
    // Cutoff modulates in the log/octave domain so a fixed depth is musically
    // uniform across the cutoff range (ADR 0004 §7): the dest value is in
    // *octaves* and the consumer applies `cutoff · 2^value`, so full depth is
    // ±4 octaves (×16). Resonance is a plain `[0, 1]` additive offset. Neither
    // is smoothed here: the OTA ladder ramps its own coefficients per frame,
    // which already absorbs the block-edge step.
    Cutoff = 28, "cutoff", "Cutoff", gain = 4.0, taper = linear,
        tier = per_stack, smooth = block;
    Resonance = 29, "resonance", "Resonance", gain = 1.0, taper = linear,
        tier = per_stack, smooth = block;
    // Stack-pitch dests: a pitch route to `OpNStackPitch` bends op N *and its
    // whole ratio-coherent FM stack* by the same semitone delta (cook-time
    // scatter). Same per-lane ±24 st semantics as `OpNPitch` — hence the same
    // gain and cubic taper — but the delta is scattered into every component
    // op at cook time rather than smoothed, so these are **not** in the
    // cascade. The taper column and the smoothing column part company here.
    Op1StackPitch = 30, "op1-stack-pitch", "Op 1 Stack Pitch", gain = 24.0,
        taper = cubic, tier = per_lane, smooth = block;
    Op2StackPitch = 31, "op2-stack-pitch", "Op 2 Stack Pitch", gain = 24.0,
        taper = cubic, tier = per_lane, smooth = block;
    Op3StackPitch = 32, "op3-stack-pitch", "Op 3 Stack Pitch", gain = 24.0,
        taper = cubic, tier = per_lane, smooth = block;
    Op4StackPitch = 33, "op4-stack-pitch", "Op 4 Stack Pitch", gain = 24.0,
        taper = cubic, tier = per_lane, smooth = block;
    Op5StackPitch = 34, "op5-stack-pitch", "Op 5 Stack Pitch", gain = 24.0,
        taper = cubic, tier = per_lane, smooth = block;
    Op6StackPitch = 35, "op6-stack-pitch", "Op 6 Stack Pitch", gain = 24.0,
        taper = cubic, tier = per_lane, smooth = block;
    // Per-op note-on phase offset dests: a continuous per-lane phase offset
    // added at the sine read, on top of the static note-on `op{n}-phase`.
    // Per-lane, linear (no cubic taper), gain 1.0 = ±1 cycle. Like the
    // level/pan dests these declare `block` and then **ramp per-sample,
    // linearly**, engine-side — it's a phase offset, not a frequency, so it
    // does not belong on the pitch cascade (ADR 0003 §3).
    Op1Phase = 36, "op1-phase", "Op 1 Phase", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    Op2Phase = 37, "op2-phase", "Op 2 Phase", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    Op3Phase = 38, "op3-phase", "Op 3 Phase", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    Op4Phase = 39, "op4-phase", "Op 4 Phase", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    Op5Phase = 40, "op5-phase", "Op 5 Phase", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    Op6Phase = 41, "op6-phase", "Op 6 Phase", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    // Filter drive dest: scales the OTA ladder pre-gain. Per-stack scalar like
    // cutoff/resonance (collapses to lane 0). Log/octave domain (gain 4.0 = ±4
    // oct), consumer applies `drive · 2^value` then clamps to the [0.1, 16]
    // param range.
    FilterDrive = 42, "filter-drive", "Filter Drive", gain = 4.0, taper = linear,
        tier = per_stack, smooth = block;
    // Amp-EG rate dests: scale the amplitude envelope's march *rate* per unison
    // lane, so a `voice-spread → eg-rate` route makes the voices in a stack
    // evolve their envelopes at slightly different speeds. **Per-lane** (each
    // lane owns its EG) and **note-on static**: the value is resolved once at
    // note-on and folded into each lane's cooked EG rates
    // (`Stack::rescale_eg_rates`) — it does *not* track live sources during the
    // note. That is why the nine eg-rate dests declare `smooth = block`:
    // consumption-time semantics, not smoothing at all (ADR 0003 §3) — putting
    // them on a smoother would give the bank state nothing ever reads.
    // Log/octave domain (gain 4.0 = ±4 oct = ×16 / ÷16 rate, like the LFO-rate /
    // cutoff dests): summing many unison lanes averages their envelopes, so a
    // narrow span reads as almost no effect. The consumer clamps the summed
    // octaves to ±4 so a multi-route stack can't run off. `GlobalEgRate` scales
    // all the envelopes (the six op amp EGs, the pitch EG, and the mod env); the
    // per-op / per-env dests add on top of it.
    GlobalEgRate = 43, "global-eg-rate", "Global EG Rate", gain = 4.0,
        taper = linear, tier = per_lane, smooth = block;
    Op1EgRate = 44, "op1-eg-rate", "Op 1 EG Rate", gain = 4.0, taper = linear,
        tier = per_lane, smooth = block;
    Op2EgRate = 45, "op2-eg-rate", "Op 2 EG Rate", gain = 4.0, taper = linear,
        tier = per_lane, smooth = block;
    Op3EgRate = 46, "op3-eg-rate", "Op 3 EG Rate", gain = 4.0, taper = linear,
        tier = per_lane, smooth = block;
    Op4EgRate = 47, "op4-eg-rate", "Op 4 EG Rate", gain = 4.0, taper = linear,
        tier = per_lane, smooth = block;
    Op5EgRate = 48, "op5-eg-rate", "Op 5 EG Rate", gain = 4.0, taper = linear,
        tier = per_lane, smooth = block;
    Op6EgRate = 49, "op6-eg-rate", "Op 6 EG Rate", gain = 4.0, taper = linear,
        tier = per_lane, smooth = block;
    // Pitch-EG rate dest: scales the per-lane Pitch EG sweep rate, so a
    // `voice-spread → pitch-eg-rate` route decorrelates the pitch sweep across
    // the unison stack (chorusing). **Per-lane** like the amp eg-rate dests;
    // `GlobalEgRate` also feeds it. Same note-on-static log/octave (±4 oct)
    // treatment, and `block` for the same reason.
    PitchEgRate = 50, "pitch-eg-rate", "Pitch EG Rate", gain = 4.0,
        taper = linear, tier = per_lane, smooth = block;
    // Mod-Env rate dest: scales the Mod Env's ADSR speed. The Mod Env is
    // one-per-voice (it drives per-stack targets like filter cutoff, where lane
    // decorrelation is meaningless), so this is **per-stack** — a `voice-spread`
    // source correctly reads as tier-collapse; drive it from per-stack sources
    // (velocity, key, LFO). `GlobalEgRate` (lane-0 collapse) also feeds it.
    ModEnvRate = 51, "mod-env-rate", "Mod Env Rate", gain = 4.0, taper = linear,
        tier = per_stack, smooth = block;
}

/// Count of non-sentinel destinations. Derived from the generated table, like
/// [`N_SOURCES`].
pub const N_DESTS: usize = DEST_NAMES.len() - 1;

// The discriminant-indexed `DEST_GAIN` table retired in 0333, as its own
// doc-comment said it would. It existed because `eval_dests` looked a gain up
// per slot per stack, by wire discriminant; the lookup now happens once per
// block inside `RouteList::compile`, through the roster seam's storage index.
// [`ROSTER_DEST_GAIN`] and [`DestId::gain`] are what remain, and they are the
// same `gain =` column of the same row list (0332).

impl DestId {
    #[inline]
    pub const fn idx(self) -> Option<usize> {
        match self {
            DestId::None => None,
            _ => Some(self as usize - 1),
        }
    }
}

/// Destinations the [`PitchSmoother`] cascade smooths, **derived** from the
/// `smooth = quantum_cascade` column rather than listed a second time, in
/// discriminant order. Smoother rows are indexed by position in this list; use
/// [`pitch_smoother_row`] to name one rather than writing the position down.
///
/// Before 0332 this was a hand-kept constant with a hand-kept `is_pitch_shaped`
/// predicate beside it, the two held together only by a test. Both are now the
/// same column of the same row.
pub const PITCH_DESTS: [DestId; N_PITCH_DESTS] = {
    let mut out = [DestId::None; N_PITCH_DESTS];
    let mut i = 0;
    let mut n = 0;
    while i < DestId::ALL.len() {
        if matches!(DestId::ALL[i].smoothing(), Smoothing::QuantumCascade) {
            out[n] = DestId::ALL[i];
            n += 1;
        }
        i += 1;
    }
    out
};

/// Count of [`PITCH_DESTS`] — the [`PitchSmoother`]'s row count.
pub const N_PITCH_DESTS: usize = {
    let mut n = 0;
    let mut i = 0;
    while i < DestId::ALL.len() {
        if matches!(DestId::ALL[i].smoothing(), Smoothing::QuantumCascade) {
            n += 1;
        }
        i += 1;
    }
    n
};

/// [`LaneDestVals`] row index for each [`PITCH_DESTS`] entry, same order.
/// Since the accumulator is dest-major, `dest_vals[PITCH_DEST_ROWS[i]]` is
/// smoother row `i`'s per-lane target directly — no gather, no transpose.
pub const PITCH_DEST_ROWS: [usize; N_PITCH_DESTS] = {
    let mut rows = [0_usize; N_PITCH_DESTS];
    let mut i = 0;
    while i < N_PITCH_DESTS {
        rows[i] = match PITCH_DESTS[i].idx() {
            Some(d) => d,
            None => panic!("PITCH_DESTS entries are never None"),
        };
        i += 1;
    }
    rows
};

/// Which [`PitchSmoother`] row carries `dest`, or `None` for a destination the
/// cascade does not smooth.
///
/// [`PITCH_DESTS`] is derived from a column now, so its *order* is the dest
/// enum's discriminant order and moves whenever a cascade-smoothed dest is
/// added. Every consumer therefore asks for its row by name; a written-down
/// literal would be right until the next roster row and then silently address
/// someone else's pitch.
///
/// `const`, so a caller naming a dest it knows is smoothed resolves the row at
/// compile time. It returns an `Option` rather than panicking on a miss because
/// it is `pub`: a runtime caller (0335's bank walks classes it did not choose)
/// gets a value to branch on instead of an audio-thread panic.
pub const fn pitch_smoother_row(dest: DestId) -> Option<usize> {
    let mut i = 0;
    while i < N_PITCH_DESTS {
        if PITCH_DESTS[i] as u8 == dest as u8 {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// The endpoint seam: what the shared routing mechanism needs to know about a
/// [`SourceId`] — which row it names, and which way it swings.
///
/// Both methods forward to the inherent ones, which keep name resolution:
/// `source.idx()` at a VXN2 call site still reaches the inherent `idx`, trait
/// in scope or not.
impl vxn_core_matrix::slot::SourceEndpoint for SourceId {
    #[inline]
    fn idx(self) -> Option<usize> {
        SourceId::idx(self)
    }

    #[inline]
    fn is_bipolar(self) -> bool {
        SourceId::is_bipolar(self)
    }
}

/// The endpoint seam for a [`DestId`]: its row, its native-unit gain and its
/// depth taper — the two numeric columns [`RouteList::compile`] folds into a
/// route's single gain factor, **once per block**, from the raw depth.
impl vxn_core_matrix::slot::DestEndpoint for DestId {
    #[inline]
    fn idx(self) -> Option<usize> {
        DestId::idx(self)
    }

    #[inline]
    fn gain(self) -> f32 {
        DestId::gain(self)
    }

    #[inline]
    fn cook_depth(self, depth: f32) -> f32 {
        DestId::cook_depth(self, depth)
    }
}

vxn_core_matrix::matrix_roster! {
    /// VXN2's roster: the eleven sources, the fifty-one destinations and their
    /// declared columns, as the shared mechanism reads them (0334).
    ///
    /// Pure forwarding to the enums' generated inherent methods. The shared
    /// evaluator is generic over it, but not because the lane loop reads a
    /// column — a compiled [`Route`] already carries the folded gain and the
    /// scale source's polarity. It reads the roster to *size* the accumulators:
    /// the `const {}` guards in [`vxn_core_matrix::storage`] turn a 51-dest
    /// roster handed VXN1b's 16-wide buffer into a compile error rather than a
    /// silently half-used one.
    ///
    /// Indices here are **storage** indices, `0..N`, one less than the wire
    /// discriminant. Anything past the roster panics, which is the trait's
    /// contract.
    Vxn2Roster, source = SourceId, dest = DestId, slots = N_SLOTS,
    source_names = ROSTER_SOURCE_NAMES, source_labels = ROSTER_SOURCE_LABELS,
    dest_names = ROSTER_DEST_NAMES, dest_labels = ROSTER_DEST_LABELS,
}

/// One matrix route: two endpoints, a **raw** depth, the two shaping axes, the
/// player's on/off switch and an optional scale VCA.
///
/// Shared with VXN1b as of 0333 ([`vxn_core_matrix::slot::MatrixSlot`]); what
/// stays here is the roster the two type parameters name. Two things changed for
/// VXN2 in that move, and they changed together on purpose:
///
/// - `depth` is now the raw, untapered value. It used to arrive already cooked
///   from `apply_block_params`, and [`RouteList::compile`] cooks — a slot that
///   still pre-cooked would cube an already-cubed depth and quietly lose ~64× of
///   a pitch route.
/// - `enabled` is a real field rather than the `source = None` this engine used
///   to fold "inactive" into at rebuild time. That fold is why VXN2 had no
///   `is_wired` to ask and no `active` column in its preset format; the
///   distinction is now the shared slot's, and the wire encodings
///   ([`crate::shared`]'s packed `u32`) are unchanged — `active` was already a
///   bit there.
pub type MatrixSlot = vxn_core_matrix::slot::MatrixSlot<SourceId, DestId>;

/// The patch's 16-slot routing topology, shared with VXN1b (0333). Slot order is
/// load-bearing: dests accumulate additively and float addition is not
/// associative.
pub type MatrixTable = vxn_core_matrix::slot::MatrixTable<SourceId, DestId, N_SLOTS>;

/// One active slot with its lane-invariant half resolved, and the block's list
/// of them. Re-exported under this module's names so `matrix::RouteList` reads
/// like the rest of the routing vocabulary.
///
/// [`RouteList::compile`] is what this engine gained in 0333: the sentinel
/// checks, the on/off switch, the zero-depth skip, the depth taper and the dest
/// gain used to be re-derived inside [`eval_dests`], which runs **once per
/// active stack**. They are pure functions of the patch, so they now happen once
/// per block instead of up to sixteen times.
pub use vxn_core_matrix::slot::Route;

/// The block's active routes — see [`Route`].
pub type RouteList = vxn_core_matrix::slot::RouteList<N_SLOTS>;

/// Patch-global scalar sources. Broadcast across every stack and every lane
/// inside [`eval_sources`].
#[derive(Clone, Copy, Debug, Default)]
pub struct PatchSources {
    pub lfo1: f32,
    pub mod_wheel: f32,
    pub aftertouch: f32,
}

impl PatchSources {
    /// Pull LFO1 from a [`ModBlock`]; mod wheel + aftertouch supplied by the
    /// host MIDI layer.
    #[inline]
    pub fn from_modblock(mb: &ModBlock, mod_wheel: f32, aftertouch: f32) -> Self {
        Self {
            lfo1: mb.lfo1,
            mod_wheel,
            aftertouch,
        }
    }
}

/// Per-stack scalar sources. Broadcast across the stack's 8 lanes.
///
/// All fields are **normalized shapes**: every source emits a
/// documented `[-1, 1]` (bipolar) or `[0, 1]` (unipolar) range, and the dest's
/// [`DestId::gain`] converts that shape to the dest's native unit. No source
/// carries hidden units a dest then re-scales.
#[derive(Clone, Copy, Debug, Default)]
pub struct StackScalarSources {
    /// Pitch EG output normalized to `[-1, 1]` — the EG *shape*, not absolute
    /// semitones. The engine divides the raw `level_st` by the configured
    /// `peg_depth` (its full-scale swing) so a pitch dest's gain (±24 st) sets
    /// the actual excursion.
    pub pitch_eg: f32,
    /// Mod env output in `[0, 1]`.
    pub mod_env: f32,
    /// Velocity normalised to `[0, 1]`.
    pub velocity: f32,
    /// Key (MIDI note) normalised to `[0, 1]`.
    pub key: f32,
}

/// Per-lane sources. One value per lane in the 8-lane stack.
#[derive(Clone, Copy, Debug, Default)]
pub struct LaneSources {
    pub lfo2: [f32; STACK_LANES],
    /// Lane index normalised to `[0, 1]`. Matrix consumers expect normalised
    /// shapes; the raw `u8` index lives on the stack for other consumers.
    pub voice_idx: [f32; STACK_LANES],
    /// Lane-symmetric position pre-scaled by the stack-spread macro: the raw
    /// `[-1, +1]` lane position is multiplied by `Stack::cached_spread` so
    /// matrix slots see a wider source as the spread fader opens. At
    /// `spread = 0` every lane reads zero — the spread macro is the matrix
    /// source's gain.
    pub voice_spread: [f32; STACK_LANES],
    /// Per-lane note-on random in `[0, 1)`.
    pub voice_rand: [f32; STACK_LANES],
}

/// Per-lane source lookup populated by [`eval_sources`], **source-major**:
/// `[source][lane]`, so one source's lanes are contiguous. This is the layout
/// `vxn_core_matrix::storage::SourceLanes` defines and the one vxn-1b's
/// `SourceLanesSoa` already uses (0328).
pub type LaneSourceVals = [[f32; STACK_LANES]; N_SOURCES];

/// Per-lane destination accumulator populated by [`eval_dests`],
/// **dest-major**: `[dest][lane]`, the mirror of [`LaneSourceVals`]. One
/// dest's lanes are contiguous, so the slot accumulate is a contiguous 8-lane
/// read-modify-write (two 4-wide vectors) instead of a scatter (0328). Matches
/// `vxn_core_matrix::storage::DestLanes` and [`PitchSmoother`]'s own state
/// layout.
pub type LaneDestVals = [[f32; STACK_LANES]; N_DESTS];

/// Fan patch + stack + lane sources into a per-lane lookup the slot eval
/// loop can read with one index per source.
#[inline]
pub fn eval_sources(
    patch: &PatchSources,
    stack: &StackScalarSources,
    lanes: &LaneSources,
    out: &mut LaneSourceVals,
) {
    // Index expressions evaluate at compile time — `SourceId::Lfo1 as usize`
    // is a constant. Source-major storage makes each row one whole store: a
    // patch/stack scalar is a lane splat, a per-lane array is a row copy.
    out[(SourceId::Lfo1 as usize) - 1] = [patch.lfo1; STACK_LANES];
    out[(SourceId::Lfo2 as usize) - 1] = lanes.lfo2;
    out[(SourceId::PitchEg as usize) - 1] = [stack.pitch_eg; STACK_LANES];
    out[(SourceId::ModEnv as usize) - 1] = [stack.mod_env; STACK_LANES];
    out[(SourceId::ModWheel as usize) - 1] = [patch.mod_wheel; STACK_LANES];
    out[(SourceId::Aftertouch as usize) - 1] = [patch.aftertouch; STACK_LANES];
    out[(SourceId::Velocity as usize) - 1] = [stack.velocity; STACK_LANES];
    out[(SourceId::Key as usize) - 1] = [stack.key; STACK_LANES];
    out[(SourceId::VoiceIdx as usize) - 1] = lanes.voice_idx;
    out[(SourceId::VoiceSpread as usize) - 1] = lanes.voice_spread;
    out[(SourceId::VoiceRand as usize) - 1] = lanes.voice_rand;
}

/// Walk the block's compiled routes, accumulating `shaped(source) · (gain ·
/// scale)` into `out`. Zeroes `out` before accumulating, so the caller can hand
/// in any buffer.
///
/// **The lane loop itself moved to [`vxn_core_matrix::eval::eval_dests_bank`]
/// in 0334** and is no longer written here; this is the roster-and-widths
/// binding, and the two synths now run the same arithmetic in the same order by
/// construction rather than by two copies agreeing. Everything the shape of that
/// loop rests on — the association, the fifteen hoisted dispatch arms,
/// `clamp_unit` over `f32::clamp` — is documented where the loop is, and the
/// warnings there apply to anyone editing it on VXN2's behalf.
///
/// What stays VXN2's, and is unchanged: this runs **once per active stack**
/// while its input is a pure function of the patch, which is why 0333 hoisted
/// [`RouteList::compile`] to once per block; the four deliberate one-block
/// dest→source feedback paths around the call; `scatter_stack_pitch` mutating
/// `out` in place between here and the smoother's target capture; the
/// cross-stack lane-0 reduction that produces patch-global dests; and the
/// `TargetFlags` gating that keeps un-targeted paths bit-identical. None of
/// those crossed the seam.
///
/// Takes a [`RouteList`], not a [`MatrixTable`] (0333). Switched-off, unwired
/// and zero-depth slots never arrive; nor does the depth taper or the dest-gain
/// lookup, both already folded into [`Route::gain`].
///
/// `scale` is the secondary-source VCA: each route's per-lane contribution is
/// multiplied by [`scale_norm`] of its scale source's value, read from the same
/// `[source][lane]` table as the primary source, bent by the route's
/// `scale_shape`. An unscaled route leaves the per-lane factor at `1.0`.
#[inline]
pub fn eval_dests(routes: &RouteList, sources: &LaneSourceVals, out: &mut LaneDestVals) {
    vxn_core_matrix::eval::eval_dests_bank::<Vxn2Roster, N_SOURCES, N_DESTS, STACK_LANES>(
        routes.active(), sources, out,
    )
}

/// Per-lane × per-pitch-dest one-pole IIR. Reads its targets straight out of
/// the block's dest-major [`LaneDestVals`] (rows [`PITCH_DEST_ROWS`]);
/// per-sample `tick` glides state toward them.
#[derive(Clone, Copy, Debug)]
pub struct PitchSmoother {
    /// First cascade stage (intermediate). Not the output — see `state`.
    stage1: [[f32; STACK_LANES]; N_PITCH_DESTS],
    /// Second cascade stage and the smoothed output (`current()` returns this).
    state: [[f32; STACK_LANES]; N_PITCH_DESTS],
    coeff: f32,
}

impl Default for PitchSmoother {
    fn default() -> Self {
        Self {
            stage1: [[0.0; STACK_LANES]; N_PITCH_DESTS],
            state: [[0.0; STACK_LANES]; N_PITCH_DESTS],
            coeff: 1.0,
        }
    }
}

impl PitchSmoother {
    /// Time constant matches the control block: each stage smooths over ~1
    /// block (one tau ≈ block duration). At 64 samples / 48 kHz that's ~1.33 ms
    /// — fast enough that block edges read smooth, slow enough that an LFO at
    /// S&H reads as steps with sloped edges rather than instant jumps.
    ///
    /// Two cascaded one-poles (not one): a single pole is C0 but C1-broken —
    /// at a saw/pulse LFO step the output value is continuous but pitch
    /// *velocity* jumps 0 → max instantly, and that velocity step is the click.
    /// Cascading a second pole makes the output slope start at 0, so sharp
    /// LFO shapes routed to pitch ramp in without a click.
    pub fn new(block_secs: f32, sample_rate: f32) -> Self {
        Self {
            stage1: [[0.0; STACK_LANES]; N_PITCH_DESTS],
            state: [[0.0; STACK_LANES]; N_PITCH_DESTS],
            coeff: one_pole_coeff(block_secs * 1000.0, sample_rate),
        }
    }

    /// Zero both cascade stages (engine reset). Same effect as snapping to an
    /// all-zero target, without materialising a whole [`LaneDestVals`] to snap
    /// against.
    pub fn clear(&mut self) {
        self.stage1 = [[0.0; STACK_LANES]; N_PITCH_DESTS];
        self.state = [[0.0; STACK_LANES]; N_PITCH_DESTS];
    }

    /// Advance one sample toward the pitch rows of `dests`, return current
    /// smoothed state. Two cascaded one-poles: `stage1` chases the target,
    /// `state` chases `stage1`. The second stage is what gives the output a
    /// zero starting slope so sharp LFO-into-pitch steps ramp in without a
    /// click.
    ///
    /// The target is the block accumulator itself: since [`LaneDestVals`] is
    /// dest-major, row `PITCH_DEST_ROWS[i]` *is* this smoother row's per-lane
    /// target and needs no copy (0328 — the old `targets_from` transpose).
    #[inline]
    pub fn tick(&mut self, dests: &LaneDestVals) -> &[[f32; STACK_LANES]; N_PITCH_DESTS] {
        let a = self.coeff;
        for i in 0..N_PITCH_DESTS {
            let target = &dests[PITCH_DEST_ROWS[i]];
            for k in 0..STACK_LANES {
                self.stage1[i][k] += a * (target[k] - self.stage1[i][k]);
                self.state[i][k] += a * (self.stage1[i][k] - self.state[i][k]);
            }
        }
        &self.state
    }

    /// Snap state to the pitch rows of `dests` without smoothing (preset load,
    /// voice steal). Both cascade stages snap so a re-armed smoother starts
    /// settled, not mid-ramp.
    pub fn snap_to(&mut self, dests: &LaneDestVals) {
        for i in 0..N_PITCH_DESTS {
            let target = dests[PITCH_DEST_ROWS[i]];
            self.stage1[i] = target;
            self.state[i] = target;
        }
    }

    /// True when every lane of *both* cascade stages is within `eps` of its
    /// target — the engine skips the tick + pitch recook entirely once a
    /// smoother has settled (the common case: no active pitch-shaped matrix
    /// route). Both stages must be checked: the output (`state`) can pass
    /// through the target while `stage1` is still mid-ramp, and freezing
    /// there would strand the output short of the real target.
    pub fn converged(&self, dests: &LaneDestVals, eps: f32) -> bool {
        for i in 0..N_PITCH_DESTS {
            let target = &dests[PITCH_DEST_ROWS[i]];
            for k in 0..STACK_LANES {
                if (self.state[i][k] - target[k]).abs() > eps
                    || (self.stage1[i][k] - target[k]).abs() > eps
                {
                    return false;
                }
            }
        }
        true
    }

    pub fn current(&self) -> &[[f32; STACK_LANES]; N_PITCH_DESTS] {
        &self.state
    }
}

/// # What is tested here, and what is not
///
/// The **mechanism** — that a route multiplies, sums, shapes, gates and
/// short-circuits correctly, and that an out-of-range curve code degrades
/// rather than aliasing — is tested once for both synths in
/// `vxn_core_matrix::golden`, against a synthetic roster whose gains are all
/// 1.0 and whose taper is the identity
/// ([ADR 0003](../../../../adrs/0003-vxn-core-matrix.md) §5, ticket 0331).
/// Asserting it here meant baking roster constants into an expectation —
/// `out[GlobalPitch] == 12.0` claimed the evaluator multiplies *and* that
/// `DestId::GlobalPitch.gain()` is 24 — so changing a gain failed a test of the
/// evaluator.
///
/// What stays below is **roster tests**: facts about this synth's own tables —
/// which gain, which taper, which tier, which coherence verdict, which name for
/// which variant — reading the evaluator only where that is the most direct way
/// to observe one. Plus the one thing that is neither: the bit-exactness
/// contract between `eval_dests`' hoisted scale arms and `scale_norm`, which is
/// about *this* loop's duplicate spelling of shared arithmetic and so cannot
/// move to the shared crate.
#[cfg(test)]
mod tests {
    use super::*;

    /// Slot with the default `direct` polarity — the common case.
    fn full_slot(source: SourceId, dest: DestId, depth: f32, shape: Shape) -> MatrixSlot {
        full_slot_pol(source, dest, depth, Polarity::Direct, shape)
    }

    fn full_slot_pol(
        source: SourceId,
        dest: DestId,
        depth: f32,
        polarity: Polarity,
        shape: Shape,
    ) -> MatrixSlot {
        MatrixSlot {
            source,
            dest,
            depth,
            polarity,
            shape,
            scale_src: SourceId::None,
            scale_shape: Shape::Lin,
            enabled: true,
        }
    }

    fn default_lane_sources() -> LaneSourceVals {
        let patch = PatchSources {
            lfo1: 0.5,
            mod_wheel: 0.3,
            aftertouch: 0.1,
        };
        let stack = StackScalarSources {
            pitch_eg: 0.75,
            mod_env: 0.7,
            velocity: 0.9,
            key: 0.45,
        };
        let mut lanes = LaneSources::default();
        for k in 0..STACK_LANES {
            lanes.lfo2[k] = -1.0 + (k as f32) * 0.25;
            lanes.voice_idx[k] = k as f32 / 7.0;
            lanes.voice_spread[k] = -1.0 + (k as f32) * 0.286;
            lanes.voice_rand[k] = (k as f32) * 0.127;
        }
        let mut out = [[0.0; STACK_LANES]; N_SOURCES];
        eval_sources(&patch, &stack, &lanes, &mut out);
        out
    }

    #[test]
    fn source_idx_skips_none_and_packs_others() {
        assert_eq!(SourceId::None.idx(), None);
        assert_eq!(SourceId::Lfo1.idx(), Some(0));
        assert_eq!(SourceId::VoiceRand.idx(), Some(N_SOURCES - 1));
    }

    #[test]
    fn dest_idx_skips_none_and_packs_others() {
        assert_eq!(DestId::None.idx(), None);
        assert_eq!(DestId::Op1Pitch.idx(), Some(0));
        // Filter dests sit after Feedback; the 6 stack-pitch dests are
        // appended after Resonance.
        assert_eq!(DestId::Feedback.idx(), Some(26));
        assert_eq!(DestId::Cutoff.idx(), Some(27));
        assert_eq!(DestId::Resonance.idx(), Some(28));
        assert_eq!(DestId::Op1StackPitch.idx(), Some(29));
        assert_eq!(DestId::Op6StackPitch.idx(), Some(34));
        // The 6 per-op phase dests are appended after the stack-pitch block.
        assert_eq!(DestId::Op1Phase.idx(), Some(35));
        assert_eq!(DestId::Op6Phase.idx(), Some(40));
        // FilterDrive is appended after the phase block.
        assert_eq!(DestId::FilterDrive.idx(), Some(41));
        // The eg-rate dests are appended after FilterDrive: global scale then
        // the 6 per-op scales.
        assert_eq!(DestId::GlobalEgRate.idx(), Some(42));
        assert_eq!(DestId::Op1EgRate.idx(), Some(43));
        assert_eq!(DestId::Op6EgRate.idx(), Some(48));
        // pitch-eg-rate + mod-env-rate hold the tail.
        assert_eq!(DestId::PitchEgRate.idx(), Some(49));
        assert_eq!(DestId::ModEnvRate.idx(), Some(N_DESTS - 1));
        // Wire-discriminant round-trip for the new dests.
        assert_eq!(DestId::from_u8(28), DestId::Cutoff);
        assert_eq!(DestId::from_u8(29), DestId::Resonance);
        assert_eq!(DestId::from_u8(30), DestId::Op1StackPitch);
        assert_eq!(DestId::from_u8(35), DestId::Op6StackPitch);
        assert_eq!(DestId::from_u8(36), DestId::Op1Phase);
        assert_eq!(DestId::from_u8(41), DestId::Op6Phase);
        assert_eq!(DestId::from_u8(42), DestId::FilterDrive);
        assert_eq!(DestId::from_u8(43), DestId::GlobalEgRate);
        assert_eq!(DestId::from_u8(44), DestId::Op1EgRate);
        assert_eq!(DestId::from_u8(49), DestId::Op6EgRate);
        assert_eq!(DestId::from_u8(50), DestId::PitchEgRate);
        assert_eq!(DestId::from_u8(51), DestId::ModEnvRate);
    }

    /// `PITCH_DESTS` is now the `smooth = quantum_cascade` column, so the set it
    /// names cannot disagree with itself and the old
    /// `pitch_shaped_set_matches_constant` test has nothing left to check. What
    /// is still worth pinning is the roster fact: *these* eight dests are the
    /// ones the cascade smooths, and the taper column is a different set.
    #[test]
    fn cascade_smoothed_set_is_the_pitch_family() {
        assert_eq!(
            PITCH_DESTS,
            [
                DestId::Op1Pitch,
                DestId::Op2Pitch,
                DestId::Op3Pitch,
                DestId::Op4Pitch,
                DestId::Op5Pitch,
                DestId::Op6Pitch,
                DestId::GlobalPitch,
                DestId::Lfo2Phase,
            ]
        );
        for d in [DestId::Op1Level, DestId::DelayMix, DestId::StackDetune] {
            assert_eq!(d.smoothing(), Smoothing::Block, "{d:?}");
        }
        // The six stack-pitch dests take the cubic taper without riding the
        // cascade — the two columns overlap, they are not the same set.
        assert_eq!(DestId::Op1StackPitch.smoothing(), Smoothing::Block);
        assert_eq!(DestId::Op1StackPitch.cook_depth(0.5), 0.125);
        // …and `Lfo2Phase` rides the cascade without the taper.
        assert_eq!(DestId::Lfo2Phase.cook_depth(0.5), 0.5);
    }

    #[test]
    fn cook_depth_tapers_semitone_dests_only() {
        // Cubic taper: sign and endpoints kept, low end widened.
        assert_eq!(DestId::GlobalPitch.cook_depth(1.0), 1.0);
        assert_eq!(DestId::GlobalPitch.cook_depth(-1.0), -1.0);
        assert_eq!(DestId::GlobalPitch.cook_depth(0.0), 0.0);
        assert!((DestId::GlobalPitch.cook_depth(0.25) - 0.015625).abs() < 1e-7);
        assert_eq!(DestId::Op3Pitch.cook_depth(-0.5), -0.125);
        // Pitch-shaped but gain 1.0: passthrough.
        assert_eq!(DestId::Lfo2Phase.cook_depth(0.5), 0.5);
        // Non-pitch dests: passthrough.
        assert_eq!(DestId::Op1Level.cook_depth(0.5), 0.5);
        assert_eq!(DestId::Feedback.cook_depth(0.5), 0.5);
    }

    /// Each destination's native unit, stated as a number rather than inferred
    /// from an evaluator result.
    ///
    /// This is what `pitch_dest_gain_scales_depth` and
    /// `feedback_dest_gain_scales_depth` were really pinning — that a pitch
    /// dest spans ±24 st and feedback its 0..7 range — with a whole matrix eval
    /// wrapped round the claim. The eval belonged to the mechanism and moved to
    /// `vxn_core_matrix::golden`; the constants are roster facts and stay here.
    ///
    /// Spot-checks one dest per *kind* of unit rather than restating the table,
    /// which would just be the `gain =` column written twice.
    #[test]
    fn dest_gains_are_the_native_unit_scalings() {
        let gain = |d: DestId| d.gain();
        // Semitone dests: ±24 st at full depth, per-op and global alike.
        assert_eq!(gain(DestId::GlobalPitch), 24.0);
        assert_eq!(gain(DestId::Op1Pitch), 24.0);
        assert_eq!(gain(DestId::Op6StackPitch), 24.0);
        // Feedback covers its own 0..7 param range.
        assert_eq!(gain(DestId::Feedback), 7.0);
        // Log-domain dests are in octaves, ±4 of them.
        assert_eq!(gain(DestId::Cutoff), 4.0);
        assert_eq!(gain(DestId::Lfo1Rate), 4.0);
        assert_eq!(gain(DestId::FilterDrive), 4.0);
        assert_eq!(gain(DestId::Op1EgRate), 4.0);
        // Everything else is already normalised: depth *is* the native unit.
        assert_eq!(gain(DestId::Op1Level), 1.0);
        assert_eq!(gain(DestId::Op1Pan), 1.0);
        assert_eq!(gain(DestId::StackDetune), 1.0);
        assert_eq!(gain(DestId::Lfo2Phase), 1.0);
        // The sentinel is inert — a slot with no dest is dropped before any
        // gain is read — and reports the identity, which cannot mislead.
        assert_eq!(gain(DestId::None), 1.0);
    }

    #[test]
    fn eval_sources_broadcasts_scalars_and_keeps_lane_values() {
        let sources = default_lane_sources();
        // Patch + stack scalars: same across lanes.
        for k in 0..STACK_LANES {
            assert_eq!(sources[SourceId::Lfo1.idx().unwrap()][k], 0.5);
            assert_eq!(sources[SourceId::ModWheel.idx().unwrap()][k], 0.3);
            assert_eq!(sources[SourceId::PitchEg.idx().unwrap()][k], 0.75);
            assert_eq!(sources[SourceId::Velocity.idx().unwrap()][k], 0.9);
        }
        // Lane-strided sources differ.
        let mut lfo2_vals = std::collections::HashSet::new();
        for k in 0..STACK_LANES {
            lfo2_vals.insert(sources[SourceId::Lfo2.idx().unwrap()][k].to_bits());
        }
        assert_eq!(lfo2_vals.len(), STACK_LANES);
    }

    // ── the shared golden-vector table, run through VXN2's own evaluator ────

    /// VXN2 endpoints standing in for the synthetic roster's four sources, in
    /// its storage-index order: two bipolar, then two unipolar.
    const GOLDEN_SOURCES: [SourceId; 4] = [
        SourceId::Lfo1,
        SourceId::Lfo2,
        SourceId::ModWheel,
        SourceId::Velocity,
    ];

    /// …and for its four destinations. Every one of these has gain 1.0
    /// and the identity taper, which is what lets a case's expectation —
    /// written against a roster with no gain and no taper — carry over
    /// unchanged. The assertion below holds them to it, so swapping in a scaled
    /// dest fails here rather than producing a plausible-looking wrong number.
    const GOLDEN_DESTS: [DestId; 4] = [
        DestId::Op1Pan,
        DestId::Op2Pan,
        DestId::Op3Pan,
        DestId::Op4Pan,
    ];

    /// The mechanism table from `vxn_core_matrix::golden`, evaluated by
    /// **VXN2's** [`eval_dests`] rather than by the harness's reference pair.
    ///
    /// This is what makes the deleted mechanism tests a move rather than a
    /// loss. The shared table's own paths prove the shared arithmetic
    /// self-consistent; nothing there touches this function, so without this
    /// bridge a transposed arm in the nine-way curve dispatch or the six-way
    /// scale dispatch — `Abs` wired to `pol_direct`, say — would be invisible.
    /// VXN2 has a single evaluator and no scalar twin to compare it against,
    /// which is exactly why it needs the shared table pointed at it.
    ///
    /// The translation is now literal in both places it used to fudge: a
    /// switched-off case route maps to a switched-off slot (rather than an
    /// unwired one standing in for the effect), and a case's depth goes in raw,
    /// because [`RouteList::compile`] is what cooks (0333).
    #[test]
    fn the_shared_golden_vectors_hold_for_vxn2() {
        use vxn_core_matrix::golden::{CASES, NONE, expected_totals};
        use vxn_core_matrix::roster::MatrixRoster;
        use vxn_core_matrix::test_roster::TestRoster;

        for (i, d) in GOLDEN_DESTS.iter().enumerate() {
            assert_eq!(d.gain(), 1.0, "{d:?} is not a unit-gain dest");
            assert_eq!(d.cook_depth(0.5), 0.5, "{d:?} does not take the identity taper");
            assert_eq!(
                GOLDEN_SOURCES[i].is_bipolar(),
                TestRoster::source_is_bipolar(i as u8),
                "source {i} stands in for the wrong polarity"
            );
        }

        let endpoint = |i: u8| {
            if i == NONE {
                SourceId::None
            } else {
                GOLDEN_SOURCES[i as usize]
            }
        };
        for case in CASES {
            let mut table = MatrixTable::default();
            for (i, r) in case.routes.iter().enumerate() {
                let (polarity, shape) = curve_split(r.curve);
                table.slots[i] = MatrixSlot {
                    source: endpoint(r.source),
                    dest: if r.dest == NONE {
                        DestId::None
                    } else {
                        GOLDEN_DESTS[r.dest as usize]
                    },
                    depth: r.depth,
                    polarity,
                    shape,
                    scale_src: endpoint(r.scale_src),
                    scale_shape: Shape::from_u8(r.scale_bend),
                    enabled: r.enabled,
                };
            }
            let mut sources = [[0.0f32; STACK_LANES]; N_SOURCES];
            for &(si, v) in case.sources {
                let row = GOLDEN_SOURCES[si as usize].idx().unwrap();
                sources[row] = [v; STACK_LANES];
            }

            let want: [f32; 4] = expected_totals::<TestRoster, 4>(case);
            let mut out = [[0.0f32; STACK_LANES]; N_DESTS];
            eval_dests(&RouteList::compile(&table), &sources, &mut out);

            for d in 0..N_DESTS {
                // A dest the case does not name must come out exactly zero, and
                // so must every VXN2 dest the mapping never touches.
                let expect = GOLDEN_DESTS
                    .iter()
                    .position(|g| g.idx() == Some(d))
                    .map_or(0.0, |g| want[g]);
                for k in 0..STACK_LANES {
                    assert_eq!(
                        out[d][k].to_bits(),
                        expect.to_bits(),
                        "'{}': dest {d} lane {k}",
                        case.name
                    );
                }
            }
        }
    }

    /// `PITCH_DEST_ROWS` maps each smoother row onto the right dest-major
    /// accumulator row — the mapping that replaced the old `targets_from`
    /// transpose (0328). `snap_to`, `tick` and `converged` all index through
    /// it; `snap_to` is the one that shows the mapping without the IIR in the
    /// way, so it is what this pins.
    #[test]
    fn smoother_reads_the_pitch_dest_rows() {
        let mut dest = [[0.0; STACK_LANES]; N_DESTS];
        let pitch_idx = DestId::GlobalPitch.idx().unwrap();
        let op_pitch_idx = DestId::Op1Pitch.idx().unwrap();
        for k in 0..STACK_LANES {
            dest[pitch_idx][k] = 1.0;
            dest[op_pitch_idx][k] = 0.25;
        }
        let mut s = PitchSmoother::default();
        s.snap_to(&dest);
        let pidx = PITCH_DESTS.iter().position(|&d| d == DestId::GlobalPitch).unwrap();
        let ridx = PITCH_DESTS.iter().position(|&d| d == DestId::Op1Pitch).unwrap();
        assert_eq!(PITCH_DEST_ROWS[pidx], pitch_idx);
        assert_eq!(PITCH_DEST_ROWS[ridx], op_pitch_idx);
        for k in 0..STACK_LANES {
            assert_eq!(s.current()[pidx][k], 1.0);
            assert_eq!(s.current()[ridx][k], 0.25);
            // Every other smoother row stayed at the accumulator's zero.
            for i in 0..N_PITCH_DESTS {
                if i != pidx && i != ridx {
                    assert_eq!(s.current()[i][k], 0.0, "row {i} lane {k}");
                }
            }
        }
    }

    #[test]
    fn smoother_glides_toward_target_over_block_time() {
        let sr = 48_000.0;
        let block_secs = 64.0 / sr;
        let mut s = PitchSmoother::new(block_secs, sr);
        // Any cascade row will do — this asserts the filter's glide, not which
        // dest sits where. Row order follows the `smooth = quantum_cascade`
        // column (0332), so naming a specific dest here would go stale.
        let mut tgt = [[0.0; STACK_LANES]; N_DESTS];
        for k in 0..STACK_LANES {
            tgt[PITCH_DEST_ROWS[0]][k] = 1.0;
        }
        // Run ~10 blocks worth of samples; should converge well past 99%.
        for _ in 0..(10 * 64) {
            s.tick(&tgt);
        }
        for k in 0..STACK_LANES {
            assert!(
                (s.current()[0][k] - 1.0).abs() < 1e-2,
                "lane {k} got {}",
                s.current()[0][k]
            );
        }
    }

    #[test]
    fn smoother_snap_jumps_immediately() {
        let mut s = PitchSmoother::default();
        let mut tgt = [[0.0; STACK_LANES]; N_DESTS];
        for k in 0..STACK_LANES {
            tgt[PITCH_DEST_ROWS[0]][k] = 0.75;
        }
        s.snap_to(&tgt);
        assert_eq!(s.current()[0][0], 0.75);
    }

    /// Every non-None source/dest, by wire discriminant, for grid walks.
    fn all_sources() -> Vec<SourceId> {
        (0..=N_SOURCES as u8).map(SourceId::from_u8).collect()
    }
    fn all_dests() -> Vec<DestId> {
        (0..=N_DESTS as u8).map(DestId::from_u8).collect()
    }

    #[test]
    fn source_tiers_cover_all_and_match_table() {
        use SourceId::*;
        for (s, want) in [
            (Lfo1, Tier::PatchGlobal),
            (ModWheel, Tier::PatchGlobal),
            (Aftertouch, Tier::PatchGlobal),
            (PitchEg, Tier::PerStack),
            (ModEnv, Tier::PerStack),
            (Velocity, Tier::PerStack),
            (Key, Tier::PerStack),
            (Lfo2, Tier::PerLane),
            (VoiceIdx, Tier::PerLane),
            (VoiceSpread, Tier::PerLane),
            (VoiceRand, Tier::PerLane),
        ] {
            assert_eq!(s.tier(), want, "{s:?}");
        }
    }

    #[test]
    fn dest_tiers_cover_all_and_match_table() {
        use DestId::*;
        for (d, want) in [
            (Lfo1Rate, Tier::PatchGlobal),
            (DelayMix, Tier::PatchGlobal),
            (ReverbMix, Tier::PatchGlobal),
            (Lfo2Rate, Tier::PerStack),
            (StackDetune, Tier::PerStack),
            (StackSpread, Tier::PerStack),
            (Cutoff, Tier::PerStack),
            (Resonance, Tier::PerStack),
            (Op1Pitch, Tier::PerLane),
            (Op6Pan, Tier::PerLane),
            (GlobalPitch, Tier::PerLane),
            (Feedback, Tier::PerLane),
            (Lfo2Phase, Tier::PerLane),
            (FilterDrive, Tier::PerStack),
            (GlobalEgRate, Tier::PerLane),
            (Op1EgRate, Tier::PerLane),
            (Op6EgRate, Tier::PerLane),
            (PitchEgRate, Tier::PerLane),
            (ModEnvRate, Tier::PerStack),
        ] {
            assert_eq!(d.tier(), want, "{d:?}");
        }
    }

    #[test]
    fn coherence_none_slots_always_ok() {
        for d in all_dests() {
            assert_eq!(coherence(SourceId::None, d), Coherence::Ok, "none→{d:?}");
        }
        for s in all_sources() {
            assert_eq!(coherence(s, DestId::None), Coherence::Ok, "{s:?}→none");
        }
    }

    #[test]
    fn coherence_self_rate() {
        assert_eq!(coherence(SourceId::Lfo1, DestId::Lfo1Rate), Coherence::SelfRate);
        assert_eq!(coherence(SourceId::Lfo2, DestId::Lfo2Rate), Coherence::SelfRate);
        // Cross-LFO rate is fine (lfo1 patch-global into lfo2-rate per-stack).
        assert_eq!(coherence(SourceId::Lfo1, DestId::Lfo2Rate), Coherence::Ok);
    }

    #[test]
    fn coherence_degenerate_voice_idx_into_lane0_dests() {
        for d in [
            DestId::Cutoff,
            DestId::Resonance,
            DestId::FilterDrive,
            DestId::DelayMix,
            DestId::ReverbMix,
        ] {
            assert_eq!(coherence(SourceId::VoiceIdx, d), Coherence::Degenerate, "{d:?}");
        }
        // voice-idx into a per-lane dest is a clean per-lane write, not degenerate.
        assert_eq!(coherence(SourceId::VoiceIdx, DestId::Op1Pan), Coherence::Ok);
    }

    /// Every verdict this synth produced **immediately before 0336** moved the
    /// predicate into `vxn-core-matrix`, dumped from that build and pasted
    /// here. One row per `SourceId` discriminant, one character per `DestId`
    /// discriminant, sentinels included: `o` = ok, `t` = tier-collapse,
    /// `s` = self-rate, `d` = degenerate.
    ///
    /// A recorded table rather than a re-derivation. The test this replaced
    /// walked the same grid but recomputed `want` from a copy of the rule, so
    /// it could only ever catch the predicate disagreeing with a transcription
    /// of itself — a tier column mistyped on a roster row would move both sides
    /// together and the assertion would still pass. These characters were
    /// produced by code that no longer exists, which is the only way the
    /// assertion is about *behaviour being unchanged* rather than about the
    /// current rule being self-consistent.
    ///
    /// Editing a character here is therefore a deliberate act: it says a
    /// routing that used to be flagged one way is now flagged another, and the
    /// commit that does it owes an explanation.
    const COHERENCE_BEFORE_0336: [&str; 12] = [
        // none            0
        "oooooooooooooooooooooooooooooooooooooooooooooooooooo",
        // lfo1            1
        "oooooooooooooooooooosooooooooooooooooooooooooooooooo",
        // lfo2            2
        "ooooooooooooooooooootsottttottooooooooooootoooooooot",
        // pitch-eg        3
        "ooooooooooooooooooootoooottooooooooooooooooooooooooo",
        // mod-env         4
        "ooooooooooooooooooootoooottooooooooooooooooooooooooo",
        // mod-wheel       5
        "oooooooooooooooooooooooooooooooooooooooooooooooooooo",
        // aftertouch      6
        "oooooooooooooooooooooooooooooooooooooooooooooooooooo",
        // velocity        7
        "ooooooooooooooooooootoooottooooooooooooooooooooooooo",
        // key             8
        "ooooooooooooooooooootoooottooooooooooooooooooooooooo",
        // voice-idx       9
        "oooooooooooooooooooottottddoddoooooooooooodoooooooot",
        // voice-spread   10
        "oooooooooooooooooooottottttottooooooooooootoooooooot",
        // voice-rand     11
        "oooooooooooooooooooottottttottooooooooooootoooooooot",
    ];

    #[test]
    fn coherence_grid_matches_the_pre_0336_table() {
        assert_eq!(
            COHERENCE_BEFORE_0336.len(),
            SOURCE_NAMES.len(),
            "a source was added or removed without re-recording the baseline"
        );
        for (si, row) in COHERENCE_BEFORE_0336.iter().enumerate() {
            assert_eq!(
                row.len(),
                DEST_NAMES.len(),
                "row {si} ({}): a dest was added or removed without re-recording \
                 the baseline",
                SOURCE_NAMES[si]
            );
            let s = SourceId::from_u8(si as u8);
            for (di, code) in row.chars().enumerate() {
                let d = DestId::from_u8(di as u8);
                let want = match code {
                    'o' => Coherence::Ok,
                    't' => Coherence::TierCollapse,
                    's' => Coherence::SelfRate,
                    'd' => Coherence::Degenerate,
                    c => panic!("unknown verdict code {c:?} at [{si}][{di}]"),
                };
                assert_eq!(
                    coherence(s, d),
                    want,
                    "{}→{} ({s:?}→{d:?}) moved",
                    SOURCE_NAMES[si],
                    DEST_NAMES[di]
                );
            }
        }
    }

    /// The baseline above is exhaustive only while it covers every pairing, so
    /// pin that it does — otherwise a dest added at a new discriminant would
    /// widen the grid and the length check above is the only thing standing
    /// between that and an unasserted column.
    #[test]
    fn the_baseline_covers_every_source_dest_pairing() {
        assert_eq!(all_sources().len(), COHERENCE_BEFORE_0336.len());
        assert_eq!(all_dests().len(), COHERENCE_BEFORE_0336[0].len());
    }

    #[test]
    fn coherence_representative_pairs() {
        // The pairs the UI test pins.
        assert_eq!(coherence(SourceId::VoiceRand, DestId::Lfo2Rate), Coherence::TierCollapse);
        assert_eq!(coherence(SourceId::VoiceRand, DestId::Lfo2Phase), Coherence::Ok);
        assert_eq!(coherence(SourceId::Velocity, DestId::Cutoff), Coherence::Ok);
        assert_eq!(coherence(SourceId::VoiceIdx, DestId::Cutoff), Coherence::Degenerate);
    }

    #[test]
    fn stack_pitch_dests_cohere_like_per_op_pitch() {
        // Same per-lane tier as OpNPitch → identical coherence verdicts.
        for (op_pitch, stack_pitch) in [
            (DestId::Op1Pitch, DestId::Op1StackPitch),
            (DestId::Op6Pitch, DestId::Op6StackPitch),
        ] {
            assert_eq!(stack_pitch.tier(), Tier::PerLane);
            for s in all_sources() {
                assert_eq!(
                    coherence(s, stack_pitch),
                    coherence(s, op_pitch),
                    "{s:?}: stack-pitch coherence diverged from per-op pitch"
                );
            }
            // Cubic taper + ±24 st gain match per-op pitch exactly.
            assert_eq!(stack_pitch.cook_depth(0.5), op_pitch.cook_depth(0.5));
            assert_eq!(
                stack_pitch.gain(),
                op_pitch.gain()
            );
        }
    }

    #[test]
    fn stack_pitch_dest_writes_own_column_only() {
        // `eval_dests` routes a stack-pitch dest to its own accumulator column
        // and does NOT spill into the per-op pitch column. Pins the single-column
        // write invariant: stack-pitch modulation is a separate accumulator from
        // per-op pitch and the two must not alias.
        let mut table = MatrixTable::default();
        table.slots[0] =
            full_slot(SourceId::Lfo1, DestId::Op3StackPitch, 1.0, Shape::Lin);
        let sources = default_lane_sources();
        let mut out = [[0.0; STACK_LANES]; N_DESTS];
        eval_dests(&RouteList::compile(&table), &sources, &mut out);
        let di = DestId::Op3StackPitch.idx().unwrap();
        // Lfo1 = 0.5, depth 1, gain 24 → 12 st in its own column.
        for k in 0..STACK_LANES {
            assert!((out[di][k] - 12.0).abs() < 1e-4);
            // The per-op pitch column is untouched.
            assert_eq!(out[DestId::Op3Pitch.idx().unwrap()][k], 0.0);
        }
    }

    #[test]
    fn coherence_name_strings_stable() {
        assert_eq!(Coherence::Ok.name(), "ok");
        assert_eq!(Coherence::TierCollapse.name(), "tier-collapse");
        assert_eq!(Coherence::SelfRate.name(), "self-rate");
        assert_eq!(Coherence::Degenerate.name(), "degenerate");
    }

    #[test]
    fn source_dest_curve_label_tables_match_enum_widths() {
        // The four `N_* + 1` widths are tautologies now that the counts are
        // *derived* from the generated tables (0330) rather than written out —
        // kept as documentation of the relationship, with
        // `variant_order_matches_the_tables` carrying the load they used to.
        assert_eq!(SOURCE_NAMES.len(), N_SOURCES + 1);
        assert_eq!(SOURCE_LABELS.len(), N_SOURCES + 1);
        assert_eq!(DEST_NAMES.len(), N_DESTS + 1);
        assert_eq!(DEST_LABELS.len(), N_DESTS + 1);
        assert_eq!(CURVE_NAMES.len(), N_CURVES);
        assert_eq!(CURVE_LABELS.len(), N_CURVES);
        // Sentinel entries first.
        assert_eq!(SOURCE_NAMES[0], "none");
        assert_eq!(DEST_NAMES[0], "none");
        // Spot-check that machine names track the enum order.
        assert_eq!(SOURCE_NAMES[SourceId::Lfo1 as usize], "lfo1");
        assert_eq!(DEST_NAMES[DestId::ReverbMix as usize], "reverb-mix");
        assert_eq!(SHAPE_NAMES.len(), N_SHAPES);
        assert_eq!(SHAPE_LABELS.len(), N_SHAPES);
        assert_eq!(POLARITY_NAMES.len(), N_POLARITIES);
        assert_eq!(POLARITY_LABELS.len(), N_POLARITIES);
    }

    /// `ALL` is the bridge between a variant and its row in the two string
    /// tables, so it has to be dense and in discriminant order: `ALL[i] as u8`
    /// must be `i`, or every name and label after a gap is off by one.
    ///
    /// This matters more here than it reads. `DestId`'s discriminants were
    /// implicit until 0330 — the language guaranteed contiguity — and are now
    /// 52 numbers written out to feed `matrix_enum!`, which builds the tables
    /// in *declaration* order while every consumer indexes them by
    /// `variant as usize`. A row typed one off, or inserted out of order, would
    /// compile, collide with nothing, and silently shift every name, label and
    /// `from_u8` decode after it — the wrong destination in the matrix combo,
    /// and saved blobs and preset TOML landing on the wrong dest. Ported from
    /// vxn-1b, which has had this test since its tables were generated.
    #[test]
    fn variant_order_matches_the_tables() {
        macro_rules! check {
            ($ty:ident, $names:ident, $labels:ident) => {
                for (i, v) in $ty::ALL.iter().enumerate() {
                    assert_eq!(*v as usize, i, "{} is not at index {i}", stringify!($ty));
                    assert_eq!($ty::from_u8(i as u8), *v, "{}::from_u8({i})", stringify!($ty));
                }
                assert_eq!($ty::ALL.len(), $names.len());
                assert_eq!($ty::ALL.len(), $labels.len());
            };
        }
        check!(SourceId, SOURCE_NAMES, SOURCE_LABELS);
        check!(DestId, DEST_NAMES, DEST_LABELS);
        check!(Polarity, POLARITY_NAMES, POLARITY_LABELS);
        check!(Shape, SHAPE_NAMES, SHAPE_LABELS);
    }

    /// Name N must *describe* variant N — the property the length-only checks
    /// cannot see, and the one a user notices first because the wrong word is
    /// sitting in the mod-matrix combo. Generation makes a transposed
    /// name/label pair unrepresentable; this catches the other half, a
    /// mis-transcribed or reordered row. Spot-checks the rows most likely to be
    /// swapped — the block boundaries and the near-identical `op{N}` families —
    /// rather than restating the whole table, which would just be the parallel
    /// list again.
    #[test]
    fn names_and_labels_describe_their_own_variant() {
        let src = |s: SourceId| (SOURCE_NAMES[s as usize], SOURCE_LABELS[s as usize]);
        assert_eq!(src(SourceId::None), ("none", "—"));
        assert_eq!(src(SourceId::Lfo1), ("lfo1", "LFO 1"));
        assert_eq!(src(SourceId::Lfo2), ("lfo2", "LFO 2"));
        assert_eq!(src(SourceId::PitchEg), ("pitch-eg", "Pitch EG"));
        assert_eq!(src(SourceId::ModEnv), ("mod-env", "Mod Env"));
        assert_eq!(src(SourceId::VoiceSpread), ("voice-spread", "Voice Spread"));
        assert_eq!(src(SourceId::VoiceRand), ("voice-rand", "Voice Rand"));

        let dst = |d: DestId| (DEST_NAMES[d as usize], DEST_LABELS[d as usize]);
        assert_eq!(dst(DestId::None), ("none", "—"));
        // Ends of the op-major block, where an off-by-one first shows.
        assert_eq!(dst(DestId::Op1Pitch), ("op1-pitch", "Op 1 Pitch"));
        assert_eq!(dst(DestId::Op6Pan), ("op6-pan", "Op 6 Pan"));
        assert_eq!(dst(DestId::GlobalPitch), ("global-pitch", "Global Pitch"));
        // The three appended families, each of which repeats `op{N}`.
        assert_eq!(dst(DestId::Op1StackPitch), ("op1-stack-pitch", "Op 1 Stack Pitch"));
        assert_eq!(dst(DestId::Op6StackPitch), ("op6-stack-pitch", "Op 6 Stack Pitch"));
        assert_eq!(dst(DestId::Op1Phase), ("op1-phase", "Op 1 Phase"));
        assert_eq!(dst(DestId::Op6Phase), ("op6-phase", "Op 6 Phase"));
        assert_eq!(dst(DestId::Op1EgRate), ("op1-eg-rate", "Op 1 EG Rate"));
        assert_eq!(dst(DestId::Op6EgRate), ("op6-eg-rate", "Op 6 EG Rate"));
        // The singletons wedged between those families, and the last row.
        assert_eq!(dst(DestId::FilterDrive), ("filter-drive", "Filter Drive"));
        assert_eq!(dst(DestId::GlobalEgRate), ("global-eg-rate", "Global EG Rate"));
        assert_eq!(dst(DestId::PitchEgRate), ("pitch-eg-rate", "Pitch EG Rate"));
        assert_eq!(dst(DestId::ModEnvRate), ("mod-env-rate", "Mod Env Rate"));
    }

    /// The hot loop dispatches the polarity fold and the bend into six macro
    /// arms rather than calling [`scale_norm`] per lane. That is a duplicate
    /// definition of the same math, so pin them together: every source
    /// polarity × bend × input must agree to the bit, or the optimisation has
    /// silently changed what a patch sounds like.
    #[test]
    fn hoisted_scale_arms_match_scale_norm_exactly() {
        let di = DestId::Op1Level.idx().unwrap();
        for scale_src in [
            SourceId::Velocity,
            SourceId::ModWheel,
            SourceId::Lfo1,
            SourceId::PitchEg,
            SourceId::VoiceRand,
        ] {
            let sc = scale_src.idx().unwrap();
            for shape in [Shape::Lin, Shape::Exp, Shape::Log] {
                for v in [-2.0_f32, -1.0, -0.5, 0.0, 0.25, 0.5, 0.75, 1.0, 3.0] {
                    let mut sources = [[0.0_f32; STACK_LANES]; N_SOURCES];
                    let si = SourceId::ModEnv.idx().unwrap();
                    for k in 0..STACK_LANES {
                        sources[si][k] = 1.0;
                        sources[sc][k] = v;
                    }
                    let mut slot =
                        full_slot(SourceId::ModEnv, DestId::Op1Level, 1.0, Shape::Lin);
                    slot.scale_src = scale_src;
                    slot.scale_shape = shape;
                    let mut table = MatrixTable::default();
                    table.slots[0] = slot;
                    let mut out = [[0.0_f32; STACK_LANES]; N_DESTS];
                    eval_dests(&RouteList::compile(&table), &sources, &mut out);
                    // Route is source 1.0 × depth 1.0 × gain, so the dest value
                    // is the scale factor times that constant.
                    let expect = scale_norm(scale_src.is_bipolar(), v, shape)
                        * DestId::Op1Level.gain();
                    assert_eq!(
                        out[di][0], expect,
                        "{scale_src:?}/{shape:?}/{v}: loop {} vs scale_norm {expect}",
                        out[di][0]
                    );
                }
            }
        }
    }

}
