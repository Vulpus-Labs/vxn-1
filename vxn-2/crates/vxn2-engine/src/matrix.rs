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
//! [`eval_sources`] fans these out into a single `[[f32; N_SOURCES];
//! STACK_LANES]` lookup table per stack — the slot eval inner loop reads from
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
//! ## Vectorisation note
//!
//! Per-slot inner loops walk 8 lanes. Curve dispatch happens once per slot
//! (outside the lane loop), so the lane-strided code in each curve arm is
//! straight-line FMA + add — autovectorises to NEON on AArch64.
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

use crate::modulation::ModBlock;

/// Slot count per patch. ADR §6 sets this at 16 for v1.
pub const N_SLOTS: usize = 16;

/// Number of CLAP-automatable depth slots (slots 1..=N). Slots past this
/// count are patch-state only.
pub const N_CLAP_DEPTH_SLOTS: usize = 8;

/// Granularity tier of a source or destination — how many independent values
/// it carries per patch. Coarse → fine, and the discriminant order *is* the
/// coarseness order (used by [`coherence`]).
///
/// - `PatchGlobal` — one value per patch (e.g. `lfo1`, `delay-mix`).
/// - `PerStack` — one value per played voice/stack (e.g. `velocity`,
///   `cutoff`). Broadcast across the stack's 8 unison lanes.
/// - `PerLane` — one value per unison lane (e.g. `lfo2`, `op1-pitch`).
///
/// A routing is **coherent** iff the source tier is coarser-or-equal to the
/// dest tier: a coarser source broadcasts unambiguously to a finer dest; a
/// finer source into a coarser dest is a lossy collapse (which lane wins?).
/// See [`coherence`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Tier {
    PatchGlobal = 0,
    PerStack = 1,
    PerLane = 2,
}

/// Why a routing is degenerate/incoherent, or [`Coherence::Ok`] if it sounds.
/// Single source of truth shared by the wiring (which sources to honour per
/// dest), the table validator, and the docs. Exported into the matrix
/// descriptor so the UI reads the verdict rather than re-deriving the rule.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Coherence {
    /// Coherent — source tier coarser-or-equal to dest tier (or an empty slot).
    Ok = 0,
    /// Finer source into a coarser dest: the per-lane/-stack value collapses
    /// to a single lane (lane 0) — lossy, ambiguous.
    TierCollapse = 1,
    /// An LFO modulating its own rate (`lfo1→lfo1-rate`, `lfo2→lfo2-rate`):
    /// self-referential.
    SelfRate = 2,
    /// `voice-idx` into a lane-0-collapsed dest: `voice_idx[0]` is always 0
    /// ([`vxn2_dsp::stack`]), so the route is a constant zero — no effect.
    Degenerate = 3,
}

impl Coherence {
    /// Machine name for the descriptor export / tooltips. Index-stable.
    #[inline]
    pub const fn name(self) -> &'static str {
        match self {
            Coherence::Ok => "ok",
            Coherence::TierCollapse => "tier-collapse",
            Coherence::SelfRate => "self-rate",
            Coherence::Degenerate => "degenerate",
        }
    }
}

/// Coherence verdict for a `source → dest` routing, per the coherence rule.
/// Empty slots (`None` source or dest) are always [`Coherence::Ok`].
///
/// Precedence: self-rate and degenerate special cases are checked **before**
/// the generic tier-collapse so they get the more specific tooltip even when
/// the tiers would also flag a collapse.
pub fn coherence(src: SourceId, dst: DestId) -> Coherence {
    // Empty slot — nothing to flag.
    if src == SourceId::None || dst == DestId::None {
        return Coherence::Ok;
    }
    // Self-rate: an LFO into its own rate. Tier-legal (both same tier) but
    // self-referential.
    if matches!(
        (src, dst),
        (SourceId::Lfo1, DestId::Lfo1Rate) | (SourceId::Lfo2, DestId::Lfo2Rate)
    ) {
        return Coherence::SelfRate;
    }
    // Degenerate: voice-idx into any lane-0-collapsed dest reads constant 0.
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
        return Coherence::Degenerate;
    }
    // Generic rule: finer source into coarser dest is a lossy collapse.
    if (src.tier() as u8) > (dst.tier() as u8) {
        return Coherence::TierCollapse;
    }
    Coherence::Ok
}

/// Modulation source. `None` is the "empty slot" sentinel — slots whose
/// source is `None` skip evaluation cheaply.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum SourceId {
    #[default]
    None = 0,
    Lfo1 = 1,
    Lfo2 = 2,
    PitchEg = 3,
    ModEnv = 4,
    ModWheel = 5,
    Aftertouch = 6,
    Velocity = 7,
    Key = 8,
    VoiceIdx = 9,
    VoiceSpread = 10,
    VoiceRand = 11,
}

/// Count of non-sentinel sources (i.e. `SourceId::None` excluded).
pub const N_SOURCES: usize = 11;

impl SourceId {
    /// Granularity tier of this source. Exhaustive — a new source
    /// forces a tier decision at compile time. `None` reports the coarsest
    /// tier (it is inert; [`coherence`] short-circuits `None` before reading
    /// tiers, so the value is never consulted for a real verdict).
    #[inline]
    pub const fn tier(self) -> Tier {
        match self {
            SourceId::None => Tier::PatchGlobal,
            SourceId::Lfo1 | SourceId::ModWheel | SourceId::Aftertouch => Tier::PatchGlobal,
            SourceId::PitchEg | SourceId::ModEnv | SourceId::Velocity | SourceId::Key => {
                Tier::PerStack
            }
            SourceId::Lfo2 | SourceId::VoiceIdx | SourceId::VoiceSpread | SourceId::VoiceRand => {
                Tier::PerLane
            }
        }
    }

    /// Whether this source emits a bipolar `[-1, 1]` shape (vs a unipolar
    /// `[0, 1]` one). Used by [`scale_norm`] to fold a bipolar scale source
    /// into the `[0, 1]` VCA range. Exhaustive so a new source forces a
    /// polarity decision at compile time.
    ///
    /// NB: `VoiceRand` is `[0, 1)` (unipolar) — treating it as bipolar would
    /// compress a `[0, 1)` random into `[0.5, 1)` and never gate to zero, so
    /// it stays unipolar (passthrough) here.
    #[inline]
    pub const fn is_bipolar(self) -> bool {
        matches!(
            self,
            SourceId::Lfo1 | SourceId::Lfo2 | SourceId::PitchEg | SourceId::VoiceSpread
        )
    }

    /// Index into the per-lane source lookup, or `None` for the sentinel.
    #[inline]
    pub const fn idx(self) -> Option<usize> {
        match self {
            SourceId::None => None,
            _ => Some(self as usize - 1),
        }
    }

    /// Decode a wire-format `u8`. Out-of-range → [`SourceId::None`] so a
    /// corrupt patch blob degrades to an inert slot rather than panicking.
    #[inline]
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => SourceId::Lfo1,
            2 => SourceId::Lfo2,
            3 => SourceId::PitchEg,
            4 => SourceId::ModEnv,
            5 => SourceId::ModWheel,
            6 => SourceId::Aftertouch,
            7 => SourceId::Velocity,
            8 => SourceId::Key,
            9 => SourceId::VoiceIdx,
            10 => SourceId::VoiceSpread,
            11 => SourceId::VoiceRand,
            _ => SourceId::None,
        }
    }
}

/// Source machine id (kebab-case, stable wire name). Index matches
/// `SourceId as u8` — `None` at index 0, then `Lfo1`..`VoiceRand`.
pub const SOURCE_NAMES: [&str; N_SOURCES + 1] = [
    "none",
    "lfo1",
    "lfo2",
    "pitch-eg",
    "mod-env",
    "mod-wheel",
    "aftertouch",
    "velocity",
    "key",
    "voice-idx",
    "voice-spread",
    "voice-rand",
];

/// Source display label. Same indexing as [`SOURCE_NAMES`].
pub const SOURCE_LABELS: [&str; N_SOURCES + 1] = [
    "—",
    "LFO 1",
    "LFO 2",
    "Pitch EG",
    "Mod Env",
    "Mod Wheel",
    "Aftertouch",
    "Velocity",
    "Key",
    "Voice Idx",
    "Voice Spread",
    "Voice Rand",
];

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
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum DestId {
    #[default]
    None = 0,
    Op1Pitch = 1, Op1Level, Op1Pan,
    Op2Pitch, Op2Level, Op2Pan,
    Op3Pitch, Op3Level, Op3Pan,
    Op4Pitch, Op4Level, Op4Pan,
    Op5Pitch, Op5Level, Op5Pan,
    Op6Pitch, Op6Level, Op6Pan,
    GlobalPitch,
    Lfo1Rate,
    Lfo2Rate,
    Lfo2Phase,
    StackDetune,
    StackSpread,
    DelayMix,
    ReverbMix,
    Feedback,
    Cutoff,
    Resonance,
    // Stack-pitch dests: a pitch route to `OpNStackPitch` bends op N *and its
    // whole ratio-coherent FM stack* by the same semitone delta (cook-time
    // scatter). Same per-lane pitch semantics as `OpNPitch`.
    Op1StackPitch,
    Op2StackPitch,
    Op3StackPitch,
    Op4StackPitch,
    Op5StackPitch,
    Op6StackPitch,
    // Per-op note-on phase offset dests: a continuous, ramped per-lane phase
    // offset added at the sine read, on top of the static note-on
    // `op{n}-phase`. Per-lane, linear (no cubic taper), gain 1.0 = ±1 cycle.
    // Applied via the level/pan-style ramp, not the pitch smoother — it's a
    // phase offset, not a frequency.
    Op1Phase,
    Op2Phase,
    Op3Phase,
    Op4Phase,
    Op5Phase,
    Op6Phase,
    // Filter drive dest: scales the OTA ladder pre-gain. Per-stack scalar like
    // cutoff/resonance (collapses to lane 0). Log/octave domain (gain 4.0 = ±4
    // oct), consumer applies `drive · 2^value` then clamps to the [0.1, 16]
    // param range.
    FilterDrive,
    // Amp-EG rate dests: scale the amplitude envelope's march *rate* per unison
    // lane, so a `voice-spread → eg-rate` route makes the voices in a stack
    // evolve their envelopes at slightly different speeds. **Per-lane** (each
    // lane owns its EG) and **note-on static**: the value is resolved once at
    // note-on and folded into each lane's cooked EG rates
    // (`Stack::rescale_eg_rates`) — it does *not* track live sources during the
    // note. Log/octave domain (gain 4.0 = ±4 oct = ×16 / ÷16 rate, like the
    // LFO-rate / cutoff dests). `GlobalEgRate` scales all the envelopes (the six
    // op amp EGs, the pitch EG, and the mod env); the per-op / per-env dests add
    // on top of it.
    GlobalEgRate,
    Op1EgRate,
    Op2EgRate,
    Op3EgRate,
    Op4EgRate,
    Op5EgRate,
    Op6EgRate,
    // Pitch-EG rate dest: scales the per-lane Pitch EG sweep rate, so a
    // `voice-spread → pitch-eg-rate` route decorrelates the pitch sweep across
    // the unison stack (chorusing). **Per-lane** like the amp eg-rate dests;
    // `GlobalEgRate` also feeds it. Same note-on-static log/octave (±4 oct)
    // treatment.
    PitchEgRate,
    // Mod-Env rate dest: scales the Mod Env's ADSR speed. The Mod Env is
    // one-per-voice (it drives per-stack targets like filter cutoff, where lane
    // decorrelation is meaningless), so this is **per-stack** — a `voice-spread`
    // source correctly reads as tier-collapse; drive it from per-stack sources
    // (velocity, key, LFO). `GlobalEgRate` (lane-0 collapse) also feeds it.
    ModEnvRate,
}

/// Count of non-sentinel destinations.
pub const N_DESTS: usize = 51;

/// Destination machine id (kebab-case wire name). Index matches
/// `DestId as u8` — `None` at index 0, then `Op1Pitch`..`Feedback`.
pub const DEST_NAMES: [&str; N_DESTS + 1] = [
    "none",
    "op1-pitch", "op1-level", "op1-pan",
    "op2-pitch", "op2-level", "op2-pan",
    "op3-pitch", "op3-level", "op3-pan",
    "op4-pitch", "op4-level", "op4-pan",
    "op5-pitch", "op5-level", "op5-pan",
    "op6-pitch", "op6-level", "op6-pan",
    "global-pitch",
    "lfo1-rate",
    "lfo2-rate",
    "lfo2-phase",
    "stack-detune",
    "stack-spread",
    "delay-mix",
    "reverb-mix",
    "feedback",
    "cutoff",
    "resonance",
    "op1-stack-pitch", "op2-stack-pitch", "op3-stack-pitch",
    "op4-stack-pitch", "op5-stack-pitch", "op6-stack-pitch",
    "op1-phase", "op2-phase", "op3-phase",
    "op4-phase", "op5-phase", "op6-phase",
    "filter-drive",
    "global-eg-rate",
    "op1-eg-rate", "op2-eg-rate", "op3-eg-rate",
    "op4-eg-rate", "op5-eg-rate", "op6-eg-rate",
    "pitch-eg-rate", "mod-env-rate",
];

/// Destination display label. Same indexing as [`DEST_NAMES`].
pub const DEST_LABELS: [&str; N_DESTS + 1] = [
    "—",
    "Op 1 Pitch", "Op 1 Level", "Op 1 Pan",
    "Op 2 Pitch", "Op 2 Level", "Op 2 Pan",
    "Op 3 Pitch", "Op 3 Level", "Op 3 Pan",
    "Op 4 Pitch", "Op 4 Level", "Op 4 Pan",
    "Op 5 Pitch", "Op 5 Level", "Op 5 Pan",
    "Op 6 Pitch", "Op 6 Level", "Op 6 Pan",
    "Global Pitch",
    "LFO 1 Rate",
    "LFO 2 Rate",
    "LFO 2 Phase",
    "Stack Detune",
    "Stack Spread",
    "Delay Mix",
    "Reverb Mix",
    "Feedback",
    "Cutoff",
    "Resonance",
    "Op 1 Stack Pitch", "Op 2 Stack Pitch", "Op 3 Stack Pitch",
    "Op 4 Stack Pitch", "Op 5 Stack Pitch", "Op 6 Stack Pitch",
    "Op 1 Phase", "Op 2 Phase", "Op 3 Phase",
    "Op 4 Phase", "Op 5 Phase", "Op 6 Phase",
    "Filter Drive",
    "Global EG Rate",
    "Op 1 EG Rate", "Op 2 EG Rate", "Op 3 EG Rate",
    "Op 4 EG Rate", "Op 5 EG Rate", "Op 6 EG Rate",
    "Pitch EG Rate", "Mod Env Rate",
];

/// Per-destination depth gain applied inside [`eval_dests`]. Depth widgets run
/// a unitless `[-1, 1]`; each source is a normalized shape, and this table
/// converts `depth × shape` to the dest's native unit so a fixed depth is
/// musically comparable across dest kinds.
///
/// **Unit table (`depth = 1` full-scale, per dest):**
///
/// | Dest | Gain | Native unit @ depth 1 |
/// |---|---|---|
/// | `op{N}-pitch`, `global-pitch` | 24.0 | ±24 semitones (±2 oct) |
/// | `op{N}-level` | 1.0 | full multiplicative tremolo on the EG |
/// | `op{N}-pan` | 1.0 | hard L↔R |
/// | `feedback` | 7.0 | the 0..7 feedback clamp range |
/// | `cutoff` | 4.0 | ±4 octaves (log domain, `cutoff · 2^v`) |
/// | `resonance` | 1.0 | additive `[0, 1]` offset |
/// | `lfo1-rate`, `lfo2-rate` | 4.0 | ±4 octaves (log domain, `rate · 2^v`) |
/// | `stack-detune` | 1.0 | scales the note-on detune by `(1 + v)` (0→2×) |
/// | `stack-spread` | 1.0 | scales the VoiceSpread width by `(1 + v)` |
/// | `delay-mix`, `reverb-mix` | 1.0 | additive `[0, 1]` mix offset |
/// | `lfo2-phase` | 1.0 | ±1 full LFO2 cycle of per-lane phase offset |
/// | `op{N}-phase` | 1.0 | ±1 full carrier cycle of per-lane phase offset |
///
/// **Cubic taper:** the 7 semitone pitch dests (`global-pitch`, `op{N}-pitch`)
/// additionally take a `d³` taper on the stored depth before the gain (see
/// [`DestId::cook_depth`]) to widen the musical low end. All other dests —
/// including the log-domain rate/cutoff and the `[-1,1]`-scale stack macros —
/// stay **linear**: their gain is already log/ratio-shaped, so a depth taper
/// would double-bend the response.
pub const DEST_GAIN: [f32; N_DESTS + 1] = {
    let mut g = [1.0_f32; N_DESTS + 1];
    g[DestId::Op1Pitch as usize] = 24.0;
    g[DestId::Op2Pitch as usize] = 24.0;
    g[DestId::Op3Pitch as usize] = 24.0;
    g[DestId::Op4Pitch as usize] = 24.0;
    g[DestId::Op5Pitch as usize] = 24.0;
    g[DestId::Op6Pitch as usize] = 24.0;
    g[DestId::GlobalPitch as usize] = 24.0;
    // Stack-pitch dests carry the same ±24 st semitone span as per-op pitch —
    // the scatter adds this delta into every component op's pitch.
    g[DestId::Op1StackPitch as usize] = 24.0;
    g[DestId::Op2StackPitch as usize] = 24.0;
    g[DestId::Op3StackPitch as usize] = 24.0;
    g[DestId::Op4StackPitch as usize] = 24.0;
    g[DestId::Op5StackPitch as usize] = 24.0;
    g[DestId::Op6StackPitch as usize] = 24.0;
    g[DestId::Feedback as usize] = 7.0;
    // Cutoff modulates in the log/octave domain so a fixed depth is musically
    // uniform across the cutoff range (ADR 0004 §7): the dest value is in
    // *octaves*; the consumer applies `cutoff · 2^value`. Full depth = ±4
    // octaves — so e.g. mod-env [0,1] at full depth sweeps cutoff up four
    // octaves (×16). (Key-tracking is a dedicated engine control, not a matrix
    // route.) Resonance is a plain `[0, 1]` additive offset (1.0).
    g[DestId::Cutoff as usize] = 4.0;
    // LFO-rate dests modulate in the log/octave domain: the dest value is in
    // *octaves*; the consumer applies `rate · 2^value`. Full depth = ±4
    // octaves, matching the cutoff span (a fixed depth is musically uniform
    // across the rate range).
    g[DestId::Lfo1Rate as usize] = 4.0;
    g[DestId::Lfo2Rate as usize] = 4.0;
    // Filter drive modulates in the log/octave domain like cutoff: the dest
    // value is in *octaves*; the consumer applies `drive · 2^value` then clamps
    // to the [0.1, 16] param range. The drive param's own taper is exponential
    // around 1.0, so a log-domain mod is musically uniform. Full depth = ±4
    // octaves (×16 / ÷16), spanning the whole drive range.
    g[DestId::FilterDrive as usize] = 4.0;
    // stack-detune / stack-spread are multiplicative scale factors
    // `(1 + depth·shape)`; gain 1.0 means depth 1 doubles the macro (0→2×).
    // Left at the table default of 1.0 — listed here so the audit is explicit.
    //
    // eg-rate dests modulate in the log/octave domain like the LFO-rate /
    // cutoff / filter-drive dests: the value is in *octaves* and the consumer
    // applies `rate · 2^value` once at note-on. Full depth = ±4 octaves (×16 /
    // ÷16 the EG speed), matching the sibling rate dests — summing many unison
    // lanes averages their envelopes, so a narrow span reads as almost no effect;
    // ±4 oct gives the spread real audible bite (dial back with depth). The
    // consumer clamps the summed octaves to ±4 so a multi-route stack can't run
    // off. Note the `voice-spread` *source* is itself scaled by the Stack-Spread
    // param, so a low spread setting shrinks this route regardless of depth.
    g[DestId::GlobalEgRate as usize] = 4.0;
    g[DestId::Op1EgRate as usize] = 4.0;
    g[DestId::Op2EgRate as usize] = 4.0;
    g[DestId::Op3EgRate as usize] = 4.0;
    g[DestId::Op4EgRate as usize] = 4.0;
    g[DestId::Op5EgRate as usize] = 4.0;
    g[DestId::Op6EgRate as usize] = 4.0;
    g[DestId::PitchEgRate as usize] = 4.0;
    g[DestId::ModEnvRate as usize] = 4.0;
    g
};

impl DestId {
    /// Granularity tier of this dest. Exhaustive — a new dest
    /// forces a tier decision at compile time. `None` reports the finest tier
    /// (inert; [`coherence`] short-circuits `None`).
    ///
    /// Per-op dests, `global-pitch`, `feedback`, `lfo2-phase` are **per-lane**
    /// (applied per unison lane). `lfo2-rate`, `stack-detune`, `stack-spread`,
    /// `cutoff`, `resonance` are **per-stack** (one value per voice; filter +
    /// LFO2 rate are stack-scalar). `lfo1-rate`, `delay-mix`, `reverb-mix` are
    /// **patch-global**.
    #[inline]
    pub const fn tier(self) -> Tier {
        match self {
            DestId::None => Tier::PerLane,
            DestId::Lfo1Rate | DestId::DelayMix | DestId::ReverbMix => Tier::PatchGlobal,
            DestId::Lfo2Rate
            | DestId::StackDetune
            | DestId::StackSpread
            | DestId::Cutoff
            | DestId::Resonance
            | DestId::FilterDrive => Tier::PerStack,
            DestId::Op1Pitch
            | DestId::Op1Level
            | DestId::Op1Pan
            | DestId::Op2Pitch
            | DestId::Op2Level
            | DestId::Op2Pan
            | DestId::Op3Pitch
            | DestId::Op3Level
            | DestId::Op3Pan
            | DestId::Op4Pitch
            | DestId::Op4Level
            | DestId::Op4Pan
            | DestId::Op5Pitch
            | DestId::Op5Level
            | DestId::Op5Pan
            | DestId::Op6Pitch
            | DestId::Op6Level
            | DestId::Op6Pan
            | DestId::GlobalPitch
            | DestId::Feedback
            | DestId::Lfo2Phase
            | DestId::Op1StackPitch
            | DestId::Op2StackPitch
            | DestId::Op3StackPitch
            | DestId::Op4StackPitch
            | DestId::Op5StackPitch
            | DestId::Op6StackPitch
            | DestId::Op1Phase
            | DestId::Op2Phase
            | DestId::Op3Phase
            | DestId::Op4Phase
            | DestId::Op5Phase
            | DestId::Op6Phase
            | DestId::GlobalEgRate
            | DestId::Op1EgRate
            | DestId::Op2EgRate
            | DestId::Op3EgRate
            | DestId::Op4EgRate
            | DestId::Op5EgRate
            | DestId::Op6EgRate
            | DestId::PitchEgRate => Tier::PerLane,
            // Mod Env is one-per-voice → its rate dest collapses to lane 0.
            DestId::ModEnvRate => Tier::PerStack,
        }
    }

    #[inline]
    pub const fn idx(self) -> Option<usize> {
        match self {
            DestId::None => None,
            _ => Some(self as usize - 1),
        }
    }

    /// Decode a wire-format `u8`. Out-of-range → [`DestId::None`] so a corrupt
    /// patch blob degrades to an inert slot rather than panicking.
    #[inline]
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => DestId::Op1Pitch,
            2 => DestId::Op1Level,
            3 => DestId::Op1Pan,
            4 => DestId::Op2Pitch,
            5 => DestId::Op2Level,
            6 => DestId::Op2Pan,
            7 => DestId::Op3Pitch,
            8 => DestId::Op3Level,
            9 => DestId::Op3Pan,
            10 => DestId::Op4Pitch,
            11 => DestId::Op4Level,
            12 => DestId::Op4Pan,
            13 => DestId::Op5Pitch,
            14 => DestId::Op5Level,
            15 => DestId::Op5Pan,
            16 => DestId::Op6Pitch,
            17 => DestId::Op6Level,
            18 => DestId::Op6Pan,
            19 => DestId::GlobalPitch,
            20 => DestId::Lfo1Rate,
            21 => DestId::Lfo2Rate,
            22 => DestId::Lfo2Phase,
            23 => DestId::StackDetune,
            24 => DestId::StackSpread,
            25 => DestId::DelayMix,
            26 => DestId::ReverbMix,
            27 => DestId::Feedback,
            28 => DestId::Cutoff,
            29 => DestId::Resonance,
            30 => DestId::Op1StackPitch,
            31 => DestId::Op2StackPitch,
            32 => DestId::Op3StackPitch,
            33 => DestId::Op4StackPitch,
            34 => DestId::Op5StackPitch,
            35 => DestId::Op6StackPitch,
            36 => DestId::Op1Phase,
            37 => DestId::Op2Phase,
            38 => DestId::Op3Phase,
            39 => DestId::Op4Phase,
            40 => DestId::Op5Phase,
            41 => DestId::Op6Phase,
            42 => DestId::FilterDrive,
            43 => DestId::GlobalEgRate,
            44 => DestId::Op1EgRate,
            45 => DestId::Op2EgRate,
            46 => DestId::Op3EgRate,
            47 => DestId::Op4EgRate,
            48 => DestId::Op5EgRate,
            49 => DestId::Op6EgRate,
            50 => DestId::PitchEgRate,
            51 => DestId::ModEnvRate,
            _ => DestId::None,
        }
    }

    /// Cubic depth taper for the ±24 st semitone dests. Linear depth puts
    /// vibrato-scale amounts (≤ 0.5 st) inside the bottom 2% of widget
    /// travel; `d³` keeps the sign and the full ±2 oct reach while widening
    /// the musical low end (25% travel ≈ ±0.4 st, 50% ≈ ±3 st). Applied at
    /// slot-cook time (block rate), never in the per-sample path. Non-pitch
    /// dests pass through untouched — `Lfo2Phase` (gain 1.0) included.
    #[inline]
    pub fn cook_depth(self, depth: f32) -> f32 {
        match self {
            DestId::GlobalPitch
            | DestId::Op1Pitch
            | DestId::Op2Pitch
            | DestId::Op3Pitch
            | DestId::Op4Pitch
            | DestId::Op5Pitch
            | DestId::Op6Pitch
            | DestId::Op1StackPitch
            | DestId::Op2StackPitch
            | DestId::Op3StackPitch
            | DestId::Op4StackPitch
            | DestId::Op5StackPitch
            | DestId::Op6StackPitch => depth * depth * depth,
            _ => depth,
        }
    }

    /// Pitch-shaped destinations are zipper-sensitive: per-sample smoothing
    /// applies. All others apply at block boundary.
    #[inline]
    pub fn is_pitch_shaped(self) -> bool {
        matches!(
            self,
            DestId::GlobalPitch
                | DestId::Lfo2Phase
                | DestId::Op1Pitch
                | DestId::Op2Pitch
                | DestId::Op3Pitch
                | DestId::Op4Pitch
                | DestId::Op5Pitch
                | DestId::Op6Pitch
        )
    }
}

/// Pitch-shaped destinations in canonical order. [`PitchSmoother`] iterates
/// over this list when copying targets out of [`LaneDestVals`].
pub const PITCH_DESTS: [DestId; N_PITCH_DESTS] = [
    DestId::GlobalPitch,
    DestId::Lfo2Phase,
    DestId::Op1Pitch,
    DestId::Op2Pitch,
    DestId::Op3Pitch,
    DestId::Op4Pitch,
    DestId::Op5Pitch,
    DestId::Op6Pitch,
];

pub const N_PITCH_DESTS: usize = 8;

/// Curve applied to a source value before depth scaling.
///
/// - `Lin` — identity (passthrough).
/// - `Exp` — signed square: `sign(v) · v²`. More extreme excursions.
/// - `Log` — signed square root: `sign(v) · √|v|`. Compresses toward 0.
/// - `Bipolar` — AC-couple a unipolar `[0, 1]` source to `[-1, +1]` via
///   `2v - 1`. Useful when routing mod-wheel / aftertouch into a pitch dest
///   that wants centred swing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum CurveKind {
    #[default]
    Lin = 0,
    Exp,
    Log,
    Bipolar,
}

/// Count of curve variants.
pub const N_CURVES: usize = 4;

/// Curve machine id. Index matches `CurveKind as u8`.
pub const CURVE_NAMES: [&str; N_CURVES] = ["lin", "exp", "log", "bipolar"];

/// Curve display label. Same indexing as [`CURVE_NAMES`].
pub const CURVE_LABELS: [&str; N_CURVES] = ["Lin", "Exp", "Log", "Bipolar"];

impl CurveKind {
    /// Decode a wire-format `u8`. Out-of-range → [`CurveKind::Lin`].
    #[inline]
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => CurveKind::Exp,
            2 => CurveKind::Log,
            3 => CurveKind::Bipolar,
            _ => CurveKind::Lin,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MatrixSlot {
    pub source: SourceId,
    pub dest: DestId,
    pub depth: f32,
    pub curve: CurveKind,
    /// Optional secondary "scale" source. When non-`None`, this slot's
    /// per-lane contribution is multiplied by the scale source's value
    /// normalised to `[0, 1]` (see [`scale_norm`]) — a VCA on the route's
    /// depth, e.g. mod wheel gating an LFO→pitch vibrato. `None` is identity
    /// (multiply by 1.0). The scale source is a *leaf* value read from the same
    /// `[lane][source]` table as the primary source, so it can never form a
    /// cycle (unlike routing a dest output back into depth).
    pub scale_src: SourceId,
}

impl Default for MatrixSlot {
    fn default() -> Self {
        Self {
            source: SourceId::None,
            dest: DestId::None,
            depth: 0.0,
            curve: CurveKind::Lin,
            scale_src: SourceId::None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MatrixTable {
    pub slots: [MatrixSlot; N_SLOTS],
}

impl Default for MatrixTable {
    fn default() -> Self {
        Self {
            slots: [MatrixSlot::default(); N_SLOTS],
        }
    }
}

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
/// [`DEST_GAIN`] converts that shape to the dest's native unit. No source
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

/// Per-lane source lookup populated by [`eval_sources`].
pub type LaneSourceVals = [[f32; N_SOURCES]; STACK_LANES];

/// Per-lane destination accumulator populated by [`eval_dests`].
pub type LaneDestVals = [[f32; N_DESTS]; STACK_LANES];

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
    // is a constant. Each lane assignment is straight stores.
    for k in 0..STACK_LANES {
        let v = &mut out[k];
        v[(SourceId::Lfo1 as usize) - 1] = patch.lfo1;
        v[(SourceId::Lfo2 as usize) - 1] = lanes.lfo2[k];
        v[(SourceId::PitchEg as usize) - 1] = stack.pitch_eg;
        v[(SourceId::ModEnv as usize) - 1] = stack.mod_env;
        v[(SourceId::ModWheel as usize) - 1] = patch.mod_wheel;
        v[(SourceId::Aftertouch as usize) - 1] = patch.aftertouch;
        v[(SourceId::Velocity as usize) - 1] = stack.velocity;
        v[(SourceId::Key as usize) - 1] = stack.key;
        v[(SourceId::VoiceIdx as usize) - 1] = lanes.voice_idx[k];
        v[(SourceId::VoiceSpread as usize) - 1] = lanes.voice_spread[k];
        v[(SourceId::VoiceRand as usize) - 1] = lanes.voice_rand[k];
    }
}

/// Normalise a scale source's value to the `[0, 1]` VCA range.
/// Unipolar sources pass through (already `[0, 1]`); bipolar sources map
/// `(x + 1) × 0.5`. Always clamped, so an out-of-range source can't push the
/// scale factor negative or past full. `0 → route contributes nothing`,
/// `1 → route at its full configured depth`.
#[inline]
pub fn scale_norm(src: SourceId, v: f32) -> f32 {
    let n = if src.is_bipolar() { (v + 1.0) * 0.5 } else { v };
    n.clamp(0.0, 1.0)
}

/// Walk slots, accumulate `source · curve · depth · scale` into `out`. Zeroes
/// `out` before accumulating, so the caller can hand in any buffer. Empty slots
/// (source = `None` or dest = `None` or depth = 0) are skipped.
///
/// `scale` is the secondary-source VCA: each slot's per-lane contribution
/// is multiplied by [`scale_norm`] of its `scale_src` value read from the same
/// `[lane][source]` table as the primary source, at the slot's own lane. A
/// `scale_src` of `None` leaves the per-lane factor at `1.0` (identity, table
/// untouched).
///
/// Curve match happens once per slot — lane loop inside each arm is
/// straight-line, autovectorises to NEON on AArch64. The scale factor is
/// resolved once per slot·lane *before* the curve dispatch, so the polarity
/// branch never lands in the hot inner loop.
#[inline]
pub fn eval_dests(table: &MatrixTable, sources: &LaneSourceVals, out: &mut LaneDestVals) {
    for k in 0..STACK_LANES {
        out[k].fill(0.0);
    }
    for slot in &table.slots {
        let Some(si) = slot.source.idx() else {
            continue;
        };
        let Some(di) = slot.dest.idx() else {
            continue;
        };
        if slot.depth == 0.0 {
            continue;
        }
        // Pre-scale depth by the destination's native-unit gain. Pitch
        // dests sweep ±2 octaves at full depth; feedback covers its 0..7
        // range; everything else uses 1.0 (depth = native units).
        let depth = slot.depth * DEST_GAIN[slot.dest as usize];
        // Secondary scale (VCA on the route). Default 1.0 per lane; only read
        // the source table when a scale source is set.
        let mut scale = [1.0_f32; STACK_LANES];
        if let Some(sc) = slot.scale_src.idx() {
            for k in 0..STACK_LANES {
                scale[k] = scale_norm(slot.scale_src, sources[k][sc]);
            }
        }
        match slot.curve {
            CurveKind::Lin => {
                for k in 0..STACK_LANES {
                    out[k][di] += sources[k][si] * depth * scale[k];
                }
            }
            CurveKind::Exp => {
                for k in 0..STACK_LANES {
                    let v = sources[k][si];
                    out[k][di] += v.abs() * v * depth * scale[k];
                }
            }
            CurveKind::Log => {
                for k in 0..STACK_LANES {
                    let v = sources[k][si];
                    let mag = v.abs().sqrt();
                    let shaped = if v < 0.0 { -mag } else { mag };
                    out[k][di] += shaped * depth * scale[k];
                }
            }
            CurveKind::Bipolar => {
                for k in 0..STACK_LANES {
                    out[k][di] += (2.0 * sources[k][si] - 1.0) * depth * scale[k];
                }
            }
        }
    }
}

/// Per-lane × per-pitch-dest one-pole IIR. Block-rate `set_targets_from`
/// updates targets; per-sample `tick` glides state toward them.
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

    /// Pull the latest block target out of `dest_block` for each pitch dest.
    pub fn targets_from(&self, dest_block: &LaneDestVals) -> [[f32; STACK_LANES]; N_PITCH_DESTS] {
        let mut tgt = [[0.0; STACK_LANES]; N_PITCH_DESTS];
        for (i, d) in PITCH_DESTS.iter().enumerate() {
            let di = d.idx().expect("PITCH_DESTS entries are never None");
            for k in 0..STACK_LANES {
                tgt[i][k] = dest_block[k][di];
            }
        }
        tgt
    }

    /// Advance one sample toward `target`, return current smoothed state.
    /// Two cascaded one-poles: `stage1` chases the target, `state` chases
    /// `stage1`. The second stage is what gives the output a zero starting
    /// slope so sharp LFO-into-pitch steps ramp in without a click.
    #[inline]
    pub fn tick(
        &mut self,
        target: &[[f32; STACK_LANES]; N_PITCH_DESTS],
    ) -> &[[f32; STACK_LANES]; N_PITCH_DESTS] {
        let a = self.coeff;
        for i in 0..N_PITCH_DESTS {
            for k in 0..STACK_LANES {
                self.stage1[i][k] += a * (target[i][k] - self.stage1[i][k]);
                self.state[i][k] += a * (self.stage1[i][k] - self.state[i][k]);
            }
        }
        &self.state
    }

    /// Snap state to `target` without smoothing (preset load, voice steal).
    /// Both cascade stages snap so a re-armed smoother starts settled, not
    /// mid-ramp.
    pub fn snap_to(&mut self, target: &[[f32; STACK_LANES]; N_PITCH_DESTS]) {
        self.stage1 = *target;
        self.state = *target;
    }

    /// True when every lane of *both* cascade stages is within `eps` of its
    /// target — the engine skips the tick + pitch recook entirely once a
    /// smoother has settled (the common case: no active pitch-shaped matrix
    /// route). Both stages must be checked: the output (`state`) can pass
    /// through the target while `stage1` is still mid-ramp, and freezing
    /// there would strand the output short of the real target.
    pub fn converged(&self, target: &[[f32; STACK_LANES]; N_PITCH_DESTS], eps: f32) -> bool {
        for i in 0..N_PITCH_DESTS {
            for k in 0..STACK_LANES {
                if (self.state[i][k] - target[i][k]).abs() > eps
                    || (self.stage1[i][k] - target[i][k]).abs() > eps
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

#[cfg(test)]
mod tests {
    use super::*;

    fn full_slot(source: SourceId, dest: DestId, depth: f32, curve: CurveKind) -> MatrixSlot {
        MatrixSlot { source, dest, depth, curve, scale_src: SourceId::None }
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
        let mut out = [[0.0; N_SOURCES]; STACK_LANES];
        eval_sources(&patch, &stack, &lanes, &mut out);
        out
    }

    /// Build a source table with a chosen patch-global LFO1 + mod-wheel value;
    /// everything else zeroed. Used by the scale tests.
    fn sources_with(lfo1: f32, mod_wheel: f32) -> LaneSourceVals {
        let patch = PatchSources {
            lfo1,
            mod_wheel,
            aftertouch: 0.0,
        };
        let mut out = [[0.0; N_SOURCES]; STACK_LANES];
        eval_sources(
            &patch,
            &StackScalarSources::default(),
            &LaneSources::default(),
            &mut out,
        );
        out
    }

    #[test]
    fn scale_norm_maps_polarity() {
        // Unipolar: passthrough (already [0, 1]).
        assert_eq!(scale_norm(SourceId::ModWheel, 0.3), 0.3);
        assert_eq!(scale_norm(SourceId::Velocity, 1.0), 1.0);
        // Bipolar: (x + 1) / 2.
        assert_eq!(scale_norm(SourceId::Lfo1, 0.0), 0.5);
        assert_eq!(scale_norm(SourceId::Lfo1, 1.0), 1.0);
        assert_eq!(scale_norm(SourceId::Lfo1, -1.0), 0.0);
        // Clamp both ends.
        assert_eq!(scale_norm(SourceId::ModWheel, 1.7), 1.0);
        assert_eq!(scale_norm(SourceId::ModWheel, -0.4), 0.0);
    }

    /// A mod-wheel scale source gates an LFO→pitch route: 0 at wheel 0, full
    /// configured depth at wheel 1 (the mod-wheel-vibrato case).
    #[test]
    fn mod_wheel_scale_gates_route_to_zero_and_full() {
        let mut table = MatrixTable::default();
        table.slots[0] = MatrixSlot {
            source: SourceId::Lfo1,
            dest: DestId::GlobalPitch,
            depth: 1.0,
            curve: CurveKind::Lin,
            scale_src: SourceId::ModWheel,
        };
        let di = DestId::GlobalPitch.idx().unwrap();

        // Wheel at 0 → route contributes nothing regardless of LFO.
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources_with(0.8, 0.0), &mut out);
        for k in 0..STACK_LANES {
            assert_eq!(out[k][di], 0.0, "lane {k} must be silent at wheel 0");
        }

        // Wheel at 1 → identical to the same route with no scale source.
        eval_dests(&table, &sources_with(0.8, 1.0), &mut out);
        let mut unscaled_table = table;
        unscaled_table.slots[0].scale_src = SourceId::None;
        let mut unscaled = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&unscaled_table, &sources_with(0.8, 1.0), &mut unscaled);
        for k in 0..STACK_LANES {
            assert_eq!(out[k][di], unscaled[k][di], "lane {k} full at wheel 1");
        }
    }

    /// A bipolar scale source at its centre (0.0) halves the route, following
    /// `(x + 1) × 0.5 = 0.5`.
    #[test]
    fn bipolar_scale_source_halves_at_centre() {
        let mut table = MatrixTable::default();
        table.slots[0] = MatrixSlot {
            source: SourceId::ModWheel,
            dest: DestId::GlobalPitch,
            depth: 1.0,
            curve: CurveKind::Lin,
            scale_src: SourceId::Lfo1, // bipolar; lfo1 = 0.0 → scale 0.5
        };
        let di = DestId::GlobalPitch.idx().unwrap();
        let mut scaled = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources_with(0.0, 0.6), &mut scaled);

        table.slots[0].scale_src = SourceId::None;
        let mut full = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources_with(0.0, 0.6), &mut full);
        for k in 0..STACK_LANES {
            assert!(
                (scaled[k][di] - 0.5 * full[k][di]).abs() < 1e-6,
                "lane {k}: {} != half of {}",
                scaled[k][di],
                full[k][di]
            );
        }
    }

    /// `scale_src = None` is exact identity: the output equals the unscaled
    /// route bit-for-bit.
    #[test]
    fn scale_src_none_is_bit_identical() {
        let src = default_lane_sources();
        let slot = full_slot(SourceId::Lfo1, DestId::Op2Level, 0.7, CurveKind::Exp);
        let mut a = [[0.0; N_DESTS]; STACK_LANES];
        let mut table = MatrixTable::default();
        table.slots[0] = slot;
        eval_dests(&table, &src, &mut a);
        let di = DestId::Op2Level.idx().unwrap();
        // Recompute the expected accumulation by hand.
        for k in 0..STACK_LANES {
            let v = src[k][SourceId::Lfo1.idx().unwrap()];
            let depth = 0.7 * DEST_GAIN[DestId::Op2Level as usize];
            let expect = v.abs() * v * depth;
            assert_eq!(a[k][di], expect, "lane {k}");
        }
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

    #[test]
    fn pitch_shaped_set_matches_constant() {
        for d in PITCH_DESTS {
            assert!(d.is_pitch_shaped(), "{d:?} missing from is_pitch_shaped");
        }
        // Spot-check non-pitch-shaped dests.
        assert!(!DestId::Op1Level.is_pitch_shaped());
        assert!(!DestId::DelayMix.is_pitch_shaped());
        assert!(!DestId::StackDetune.is_pitch_shaped());
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

    #[test]
    fn eval_sources_broadcasts_scalars_and_keeps_lane_values() {
        let sources = default_lane_sources();
        // Patch + stack scalars: same across lanes.
        for k in 0..STACK_LANES {
            assert_eq!(sources[k][SourceId::Lfo1.idx().unwrap()], 0.5);
            assert_eq!(sources[k][SourceId::ModWheel.idx().unwrap()], 0.3);
            assert_eq!(sources[k][SourceId::PitchEg.idx().unwrap()], 0.75);
            assert_eq!(sources[k][SourceId::Velocity.idx().unwrap()], 0.9);
        }
        // Lane-strided sources differ.
        let mut lfo2_vals = std::collections::HashSet::new();
        for k in 0..STACK_LANES {
            lfo2_vals.insert(sources[k][SourceId::Lfo2.idx().unwrap()].to_bits());
        }
        assert_eq!(lfo2_vals.len(), STACK_LANES);
    }

    #[test]
    fn empty_table_writes_zero_accumulator() {
        let table = MatrixTable::default();
        let sources = default_lane_sources();
        let mut out = [[42.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources, &mut out);
        for k in 0..STACK_LANES {
            for d in 0..N_DESTS {
                assert_eq!(out[k][d], 0.0, "lane {k} dest {d}");
            }
        }
    }

    #[test]
    fn single_lin_slot_writes_only_target_dest() {
        // Use a gain=1 dest (Op1Pan) so the numerical check covers the
        // accumulator + curve math without the per-dest gain table mixing in.
        let mut table = MatrixTable::default();
        table.slots[0] = full_slot(SourceId::Lfo1, DestId::Op1Pan, 0.5, CurveKind::Lin);
        let sources = default_lane_sources();
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources, &mut out);
        let dest_idx = DestId::Op1Pan.idx().unwrap();
        for k in 0..STACK_LANES {
            // Lfo1 = 0.5, depth = 0.5, lin, gain = 1 → 0.25 across every lane.
            assert!(
                (out[k][dest_idx] - 0.25).abs() < 1e-6,
                "lane {k} got {}",
                out[k][dest_idx]
            );
            for d in 0..N_DESTS {
                if d == dest_idx {
                    continue;
                }
                assert_eq!(out[k][d], 0.0, "lane {k} non-target dest {d}");
            }
        }
    }

    #[test]
    fn two_slots_into_same_dest_accumulate() {
        let mut table = MatrixTable::default();
        table.slots[0] = full_slot(SourceId::Lfo1, DestId::Op1Pan, 0.5, CurveKind::Lin);
        table.slots[1] = full_slot(SourceId::ModWheel, DestId::Op1Pan, 1.0, CurveKind::Lin);
        let sources = default_lane_sources();
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources, &mut out);
        let want = 0.5 * 0.5 + 1.0 * 0.3;
        for k in 0..STACK_LANES {
            assert!((out[k][DestId::Op1Pan.idx().unwrap()] - want).abs() < 1e-6);
        }
    }

    #[test]
    fn pitch_dest_gain_scales_depth() {
        // Pitch dests sweep ±2 octaves at full depth: depth × source × 24.
        let mut table = MatrixTable::default();
        table.slots[0] =
            full_slot(SourceId::Lfo1, DestId::GlobalPitch, 1.0, CurveKind::Lin);
        let sources = default_lane_sources();
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources, &mut out);
        let di = DestId::GlobalPitch.idx().unwrap();
        // Lfo1 = 0.5, depth = 1, gain = 24 → 12 semitones.
        for k in 0..STACK_LANES {
            assert!((out[k][di] - 12.0).abs() < 1e-4, "lane {k} got {}", out[k][di]);
        }
    }

    #[test]
    fn feedback_dest_gain_scales_depth() {
        let mut table = MatrixTable::default();
        table.slots[0] =
            full_slot(SourceId::ModWheel, DestId::Feedback, 1.0, CurveKind::Lin);
        let sources = default_lane_sources();
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources, &mut out);
        let di = DestId::Feedback.idx().unwrap();
        // ModWheel = 0.3, depth = 1, gain = 7 → 2.1.
        for k in 0..STACK_LANES {
            assert!((out[k][di] - 2.1).abs() < 1e-4, "lane {k} got {}", out[k][di]);
        }
    }

    #[test]
    fn per_lane_source_writes_distinct_lane_values() {
        let mut table = MatrixTable::default();
        table.slots[0] =
            full_slot(SourceId::VoiceSpread, DestId::Op1Pan, 1.0, CurveKind::Lin);
        let sources = default_lane_sources();
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources, &mut out);
        let pan_idx = DestId::Op1Pan.idx().unwrap();
        let mut distinct = std::collections::HashSet::new();
        for k in 0..STACK_LANES {
            distinct.insert(out[k][pan_idx].to_bits());
        }
        assert_eq!(distinct.len(), STACK_LANES);
    }

    #[test]
    fn empty_slot_skipped_when_source_none() {
        let mut table = MatrixTable::default();
        table.slots[0] = MatrixSlot {
            source: SourceId::None,
            dest: DestId::Op1Pan,
            depth: 99.0,
            curve: CurveKind::Lin,
            scale_src: SourceId::None,
        };
        let sources = default_lane_sources();
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources, &mut out);
        for k in 0..STACK_LANES {
            assert_eq!(out[k][DestId::Op1Pan.idx().unwrap()], 0.0);
        }
    }

    #[test]
    fn empty_slot_skipped_when_dest_none() {
        let mut table = MatrixTable::default();
        table.slots[0] = MatrixSlot {
            source: SourceId::Lfo1,
            dest: DestId::None,
            depth: 99.0,
            curve: CurveKind::Lin,
            scale_src: SourceId::None,
        };
        let sources = default_lane_sources();
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources, &mut out);
        for k in 0..STACK_LANES {
            for d in 0..N_DESTS {
                assert_eq!(out[k][d], 0.0);
            }
        }
    }

    #[test]
    fn zero_depth_short_circuits() {
        let mut table = MatrixTable::default();
        table.slots[0] = full_slot(SourceId::Lfo1, DestId::Op1Pan, 0.0, CurveKind::Lin);
        let sources = default_lane_sources();
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources, &mut out);
        for k in 0..STACK_LANES {
            assert_eq!(out[k][DestId::Op1Pan.idx().unwrap()], 0.0);
        }
    }

    #[test]
    fn curve_exp_more_extreme_than_lin() {
        // Source = 0.5 → lin = 0.5, exp = 0.25 (less extreme magnitude-wise
        // for |v| < 1, but characterised by the signed-square shape, not by
        // gain). Just verify it's different from lin.
        let mut lin_t = MatrixTable::default();
        lin_t.slots[0] = full_slot(SourceId::ModWheel, DestId::Op1Pan, 1.0, CurveKind::Lin);
        let mut exp_t = MatrixTable::default();
        exp_t.slots[0] = full_slot(SourceId::ModWheel, DestId::Op1Pan, 1.0, CurveKind::Exp);
        let sources = default_lane_sources();
        let mut lin_out = [[0.0; N_DESTS]; STACK_LANES];
        let mut exp_out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&lin_t, &sources, &mut lin_out);
        eval_dests(&exp_t, &sources, &mut exp_out);
        let pi = DestId::Op1Pan.idx().unwrap();
        assert!(
            (lin_out[0][pi] - 0.3).abs() < 1e-6,
            "lin {} != 0.3",
            lin_out[0][pi]
        );
        assert!(
            (exp_out[0][pi] - 0.09).abs() < 1e-6,
            "exp {} != 0.09",
            exp_out[0][pi]
        );
    }

    #[test]
    fn curve_log_compresses_toward_zero() {
        let mut log_t = MatrixTable::default();
        log_t.slots[0] = full_slot(SourceId::ModWheel, DestId::Op1Pan, 1.0, CurveKind::Log);
        let sources = default_lane_sources();
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&log_t, &sources, &mut out);
        // ModWheel = 0.3, sqrt(0.3) ≈ 0.5477.
        let want = (0.3_f32).sqrt();
        assert!((out[0][DestId::Op1Pan.idx().unwrap()] - want).abs() < 1e-6);
    }

    #[test]
    fn curve_bipolar_shifts_unipolar_source() {
        let mut bp_t = MatrixTable::default();
        bp_t.slots[0] = full_slot(SourceId::ModWheel, DestId::Op1Pan, 1.0, CurveKind::Bipolar);
        let sources = default_lane_sources();
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&bp_t, &sources, &mut out);
        // ModWheel = 0.3 → 2*0.3 - 1 = -0.4.
        assert!((out[0][DestId::Op1Pan.idx().unwrap()] - (-0.4)).abs() < 1e-6);
    }

    #[test]
    fn curve_preserves_sign_for_lin_exp_log() {
        // Negative source preserves sign through Lin/Exp/Log.
        let patch = PatchSources::default();
        let stack = StackScalarSources::default();
        let mut lanes = LaneSources::default();
        for k in 0..STACK_LANES {
            lanes.voice_spread[k] = -0.5;
        }
        let mut sources = [[0.0; N_SOURCES]; STACK_LANES];
        eval_sources(&patch, &stack, &lanes, &mut sources);
        for curve in [CurveKind::Lin, CurveKind::Exp, CurveKind::Log] {
            let mut table = MatrixTable::default();
            table.slots[0] = full_slot(SourceId::VoiceSpread, DestId::Op1Pan, 1.0, curve);
            let mut out = [[0.0; N_DESTS]; STACK_LANES];
            eval_dests(&table, &sources, &mut out);
            let v = out[0][DestId::Op1Pan.idx().unwrap()];
            assert!(v < 0.0, "{curve:?} dropped sign: {v}");
        }
    }

    #[test]
    fn smoother_targets_from_picks_pitch_dest_columns() {
        let mut dest = [[0.0; N_DESTS]; STACK_LANES];
        let pitch_idx = DestId::GlobalPitch.idx().unwrap();
        let op_pitch_idx = DestId::Op1Pitch.idx().unwrap();
        for k in 0..STACK_LANES {
            dest[k][pitch_idx] = 1.0;
            dest[k][op_pitch_idx] = 0.25;
        }
        let s = PitchSmoother::default();
        let tgt = s.targets_from(&dest);
        let pidx = PITCH_DESTS.iter().position(|&d| d == DestId::GlobalPitch).unwrap();
        let ridx = PITCH_DESTS.iter().position(|&d| d == DestId::Op1Pitch).unwrap();
        for k in 0..STACK_LANES {
            assert_eq!(tgt[pidx][k], 1.0);
            assert_eq!(tgt[ridx][k], 0.25);
        }
    }

    #[test]
    fn smoother_glides_toward_target_over_block_time() {
        let sr = 48_000.0;
        let block_secs = 64.0 / sr;
        let mut s = PitchSmoother::new(block_secs, sr);
        let mut tgt = [[0.0; STACK_LANES]; N_PITCH_DESTS];
        for k in 0..STACK_LANES {
            tgt[0][k] = 1.0;
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
        let mut tgt = [[0.0; STACK_LANES]; N_PITCH_DESTS];
        for k in 0..STACK_LANES {
            tgt[0][k] = 0.75;
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

    #[test]
    fn coherence_grid_matches_tier_rule_with_special_cases() {
        for s in all_sources() {
            for d in all_dests() {
                let got = coherence(s, d);
                let want = if s == SourceId::None || d == DestId::None {
                    Coherence::Ok
                } else if matches!(
                    (s, d),
                    (SourceId::Lfo1, DestId::Lfo1Rate) | (SourceId::Lfo2, DestId::Lfo2Rate)
                ) {
                    Coherence::SelfRate
                } else if s == SourceId::VoiceIdx
                    && matches!(
                        d,
                        DestId::Cutoff
                            | DestId::Resonance
                            | DestId::FilterDrive
                            | DestId::DelayMix
                            | DestId::ReverbMix
                    ) {
                    Coherence::Degenerate
                } else if (s.tier() as u8) > (d.tier() as u8) {
                    Coherence::TierCollapse
                } else {
                    Coherence::Ok
                };
                assert_eq!(got, want, "{s:?}→{d:?}");
            }
        }
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
                DEST_GAIN[stack_pitch as usize],
                DEST_GAIN[op_pitch as usize]
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
            full_slot(SourceId::Lfo1, DestId::Op3StackPitch, 1.0, CurveKind::Lin);
        let sources = default_lane_sources();
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources, &mut out);
        let di = DestId::Op3StackPitch.idx().unwrap();
        // Lfo1 = 0.5, depth 1, gain 24 → 12 st in its own column.
        for k in 0..STACK_LANES {
            assert!((out[k][di] - 12.0).abs() < 1e-4);
            // The per-op pitch column is untouched.
            assert_eq!(out[k][DestId::Op3Pitch.idx().unwrap()], 0.0);
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
        assert_eq!(CURVE_NAMES[CurveKind::Bipolar as usize], "bipolar");
    }

}
