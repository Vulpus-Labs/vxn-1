//! VXN1b flat parameter table (ticket 0200).
//!
//! Forked from VXN1's model ([`vxn-1/crates/vxn-app/src/params.rs`] +
//! `vxn-engine/src/params.rs`) and reshaped for matrix modulation
//! (ADR 0001 §5):
//!
//! - **Single flat table.** VXN1 duplicates a per-patch block across two layers
//!   (Upper/Lower) plus a global block. VXN1b is a **single-patch** instrument
//!   (ADR 0001 §7's compact one-voice faceplate), so patch and global collapse
//!   into one flat, index-addressed table: `ParamId` = CLAP id = array index,
//!   plain-unit `f32`, identity CLAP-id map (no layer interleave).
//! - **Fixed mod-panel params removed.** Every fixed-route source selector and
//!   depth of VXN1 ADR 0004 (`PitchLfoSrc/Depth`, `CutoffLfo1Depth`,
//!   `VelCutoffDepth`, `ModWheel*`, the `LfoSel`/`EnvSel` selectors, …) is gone
//!   — the matrix (0201/0202) replaces them.
//! - **16 automatable slot depths added.** `MatrixSlotNDepth`, bipolar
//!   `[-1, 1]`. Slot *topology* (source/dest/curve/scale) is patch state, **not**
//!   here (0201 model / 0203 persistence) — only depths are CLAP params.
//! - **Pitch-bend range stays hardwired** (ADR 0001 §3): `PitchBendRange` is a
//!   dedicated always-on term, not a matrix route. (Pitch Wheel is *additionally*
//!   a matrix source, wired in 0201/0202.)
//! - **FX chain params** (0207, epic E037): a serial chorus → phaser → delay →
//!   reverb → dynamics section, each with an on/off bool + wet/mix and a few
//!   character knobs, slotted between `Oversample` and the matrix depths.

use vxn_core_app::{ParamDesc, ParamKind, Taper};
use vxn_dsp::{AdsrShape, FilterMode, FilterSlope, LfoShape, NoiseColor, Waveform};

// ── Param-value enums (variant indices stored as f32) ───────────────────────

/// Lanes per note — the *voicing* half of what VXN1's four-way assign mode
/// used to conflate (ADR 0003, ticket 0266). Powers of two only: the width
/// divides the lane pool exactly, so there are never orphaned lanes.
///
/// Simultaneous notes = [`MAX_VOICES_1B`](crate::MAX_VOICES) `/ width`, so the
/// widest setting (32, the whole pool — ticket 0264) is monophonic *by
/// capacity* while still being polyphonic *by behaviour*: a new note steals the
/// stack and retriggers, with no legato. That combination is unreachable in
/// VXN1's enum, and it is the point of splitting the two.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(usize)]
pub enum StackWidth {
    #[default]
    One,
    Two,
    Four,
    Eight,
    Sixteen,
    ThirtyTwo,
}

impl StackWidth {
    pub const COUNT: usize = StackWidth::ThirtyTwo as usize + 1;

    pub fn from_index(i: usize) -> StackWidth {
        match i {
            1 => StackWidth::Two,
            2 => StackWidth::Four,
            3 => StackWidth::Eight,
            4 => StackWidth::Sixteen,
            5 => StackWidth::ThirtyTwo,
            _ => StackWidth::One,
        }
    }

    /// Lanes per note.
    #[inline]
    pub fn lanes(self) -> usize {
        1 << (self as usize)
    }
}

/// Keyboard behaviour — the *articulation* half. Orthogonal to [`StackWidth`]:
/// any width can be played either way.
///
/// - `Poly` — each note takes its own stack; a new note steals when the pool is
///   full and always retriggers.
/// - `Solo` — one stack, last-note priority, with the notes beneath it held on
///   a stack so releasing the top reveals what is under it. `Legato` then
///   decides whether the reveal slides or articulates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(usize)]
pub enum VoiceMode {
    #[default]
    Poly,
    Solo,
}

impl VoiceMode {
    pub const COUNT: usize = VoiceMode::Solo as usize + 1;

    pub fn from_index(i: usize) -> VoiceMode {
        match i {
            1 => VoiceMode::Solo,
            _ => VoiceMode::Poly,
        }
    }
}

/// Oscillator-interaction type (Off / Sync / PM ("FM" in labels) / Ring).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(usize)]
pub enum CrossModType {
    #[default]
    Off,
    Sync,
    Pm,
    Ring,
}

impl CrossModType {
    pub const COUNT: usize = 4;

    pub fn from_index(i: usize) -> CrossModType {
        match i {
            1 => CrossModType::Sync,
            2 => CrossModType::Pm,
            3 => CrossModType::Ring,
            _ => CrossModType::Off,
        }
    }
}

// ── Param id enum ───────────────────────────────────────────────────────────

/// Define a `#[repr(usize)]` param-id enum with contiguous discriminants and a
/// **safe** `from_index` (exhaustive match, no `unsafe transmute`), plus
/// `COUNT`, `index`, and `all`. The variant list is written once, so a new param
/// can't leave `from_index`/`COUNT` out of sync. (Ported verbatim from VXN1.)
macro_rules! indexed_param_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident { $($variant:ident),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[repr(usize)]
        $vis enum $name { $($variant),+ }

        impl $name {
            /// Number of variants (= the flat table length).
            pub const COUNT: usize = [$($name::$variant),+].len();

            /// Every variant in discriminant order.
            pub fn all() -> impl Iterator<Item = $name> {
                [$($name::$variant),+].into_iter()
            }

            /// Discriminant = CLAP id = index into the param table.
            #[inline]
            pub fn index(self) -> usize {
                self as usize
            }

            /// Inverse of [`Self::index`]; `None` past the last variant.
            pub fn from_index(i: usize) -> Option<$name> {
                match i {
                    $(x if x == $name::$variant as usize => Some($name::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

indexed_param_enum! {
/// VXN1b parameter ids. Discriminant = CLAP id = index into [`PARAMS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamId {
    // ── Osc / mixer ──
    Osc1Wave,
    Osc1Coarse,
    Osc1Fine,
    Osc1Octave,
    Osc1Level,
    Osc1PulseWidth,
    Osc2Wave,
    Osc2Coarse,
    Osc2Fine,
    Osc2Octave,
    Osc2Level,
    Osc2PulseWidth,
    SubLevel,
    CrossModType,
    CrossModAmount,
    NoiseLevel,
    NoiseColor,
    // ── Filter ──
    Cutoff,
    Resonance,
    Drive,
    FilterMode,
    FilterSlope,
    HpfCutoff,
    // Key-track (0245) is a dedicated param, not a matrix route: it is filter
    // *calibration*, not modulation. At 1.0 the cutoff shifts `note − 12` st,
    // pivoting at C0 like VXN1's `filter_key_track`, so cutoff-at-minimum
    // (16.3516 Hz = C0) tracks the played note exactly. A Key→Cutoff matrix
    // route stacks on top for the free-form cases (KEY_CUTOFF_UNITY_DEPTH).
    FilterKeyTrack,
    // UI-only display-mode toggle, ported from VXN1 (0250): when on, the editor
    // maps the Cutoff fader as note-quantised Hz over MIDI C0..C4 with a
    // note-name readout (dispatch.js's cutoff overrides). The ENGINE
    // DELIBERATELY NEVER READS THIS — cutoff stays a plain Hz param either way.
    // It is still a persisted param so the display mode travels with
    // presets/state, exactly as VXN1's does.
    CutoffTuned,
    // ── Envelopes ──
    Env1Attack,
    Env1Decay,
    Env1Sustain,
    Env1Release,
    Env1Shape,
    Env2Attack,
    Env2Decay,
    Env2Sustain,
    Env2Release,
    Env2Shape,
    // ── Amp ──
    AmpEnvBypass,
    // ── Layer mix (0220) ──
    // Per-layer, so they live in the patch block and the two-layer expansion
    // gives one instance per synth. A preset therefore carries its own layer
    // balance, which is the point — this replaces ADR 0002 §7's single global
    // "layer balance" control (a balance knob cannot set absolute levels).
    LayerLevel,
    LayerMute,
    // Placement of the layer in the stereo image (0248). Bipolar, centre 0.
    // Everything downstream of the voice is already stereo, so this is one
    // multiply in the mix loop that already applies `LayerLevel`.
    LayerPan,
    // Tuning of the whole layer in cents (0263) — a third beating axis,
    // distinct from `UnisonDetune` (per lane, within a voice) and `Osc2Fine`
    // (per oscillator, within a layer). Lands on the layer's pitch base, so
    // both oscillators and the sub move together.
    LayerDetune,
    // ── LFO 1 ──
    Lfo1Shape,
    Lfo1Rate,
    Lfo1Sync,
    Lfo1DelayTime,
    Lfo1Fade,
    Lfo1FreeRun,
    // ── LFO 2 ──
    Lfo2Shape,
    Lfo2Rate,
    Lfo2Sync,
    // ── Pitch bend (hardwired global, ADR §3) ──
    PitchBendRange,
    // ── Voice ──
    StackWidth,
    VoiceMode,
    Legato,
    UnisonDetune,
    PortamentoTime,
    Spread,
    // ── Master ──
    MasterTune,
    MasterVolume,
    MasterDrift,
    LimiterOn,
    Oversample,
    // ── FX chain (serial: chorus → phaser → delay → reverb → dynamics, 0207) ──
    ChorusOn,
    ChorusRate,
    ChorusDepth,
    ChorusMix,
    PhaserOn,
    PhaserRate,
    PhaserDepth,
    PhaserFeedback,
    PhaserMix,
    PhaserStereo,
    DelayOn,
    DelayTime,
    DelayFeedback,
    DelayMix,
    DelaySync,
    DelayPingPong,
    ReverbOn,
    ReverbSize,
    ReverbDecay,
    ReverbDamp,
    ReverbMix,
    DynamicsOn,
    DynamicsThreshold,
    DynamicsRatio,
    DynamicsAttack,
    DynamicsRelease,
    DynamicsMakeup,
    DynamicsDrive,
    DynamicsMix,
    // ── Matrix slot depths (bipolar, automatable) ──
    MatrixSlot0Depth,
    MatrixSlot1Depth,
    MatrixSlot2Depth,
    MatrixSlot3Depth,
    MatrixSlot4Depth,
    MatrixSlot5Depth,
    MatrixSlot6Depth,
    MatrixSlot7Depth,
    MatrixSlot8Depth,
    MatrixSlot9Depth,
    MatrixSlot10Depth,
    MatrixSlot11Depth,
    MatrixSlot12Depth,
    MatrixSlot13Depth,
    MatrixSlot14Depth,
    MatrixSlot15Depth,
}
}

/// Number of mod-matrix slots whose depths are params (ADR 0001 §2).
pub const MATRIX_SLOTS: usize = 16;

impl ParamId {
    /// Resolve a [`ParamDesc::name`] string to its param (preset key lookup).
    pub fn from_name(name: &str) -> Option<ParamId> {
        PARAMS
            .iter()
            .position(|d| d.name == name)
            .and_then(Self::from_index)
    }

    #[inline]
    pub fn desc(self) -> &'static ParamDesc {
        &PARAMS[self.index()]
    }

    /// The depth param for matrix slot `slot` (`0..MATRIX_SLOTS`).
    pub fn slot_depth(slot: usize) -> Option<ParamId> {
        (slot < MATRIX_SLOTS)
            .then(|| Self::from_index(ParamId::MatrixSlot0Depth as usize + slot))
            .flatten()
    }

    /// Inverse of [`Self::slot_depth`]: the slot index a CLAP id addresses, or
    /// `None` if the id is not a slot-depth param. Lets `set_param` mirror a
    /// depth edit into the matrix without a range test (0205).
    pub fn slot_depth_index(clap_id: usize) -> Option<usize> {
        let base = ParamId::MatrixSlot0Depth as usize;
        (base..base + MATRIX_SLOTS)
            .contains(&clap_id)
            .then(|| clap_id - base)
    }
}

// ── Two-layer CLAP map (0216, ADR 0002 §4) ──────────────────────────────────
//
// The inner `ParamId`/`PARAMS`/`Params` above are **per-synth** — each `Synth`
// owns one full table (0214). The *host-facing* CLAP surface is a two-layer
// expansion of it, mirroring VXN1's `Layer::COUNT * PATCH_COUNT + GLOBAL_COUNT`
// layout: Layer 1's patch block first, then Layer 2's, then one shared global
// block. So CLAP id → (layer, inner id) or (global, inner id); the UI's existing
// layer-offset machinery keys on `patchCount` = [`PATCH_COUNT`] (Layer 2 = Layer
// 1 + `PATCH_COUNT`).

/// A synth layer. `L1` drives synth 0 (Upper), `L2` drives synth 1 (Lower).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Layer {
    L1 = 0,
    L2 = 1,
}

impl Layer {
    pub const ALL: [Layer; 2] = [Layer::L1, Layer::L2];

    /// Host param-tree module label — VXN1's "Upper"/"Lower".
    pub fn module(self) -> &'static str {
        match self {
            Layer::L1 => "Upper",
            Layer::L2 => "Lower",
        }
    }
}

/// The per-layer **patch** params, in CLAP order — every control duplicated per
/// synth (osc, mixer, filter, envelopes, layer level/mute, LFO 1/2, voice, and
/// the 16 matrix depths). Order *defines* the patch-block CLAP id layout; Layer
/// 2's block is the same list offset by [`PATCH_COUNT`].
pub const PATCH_PARAMS: [ParamId; 71] = {
    use ParamId::*;
    [
        // Osc / mixer (17)
        Osc1Wave, Osc1Coarse, Osc1Fine, Osc1Octave, Osc1Level, Osc1PulseWidth,
        Osc2Wave, Osc2Coarse, Osc2Fine, Osc2Octave, Osc2Level, Osc2PulseWidth,
        SubLevel, CrossModType, CrossModAmount, NoiseLevel, NoiseColor,
        // Filter (8)
        Cutoff, Resonance, Drive, FilterMode, FilterSlope, HpfCutoff,
        FilterKeyTrack, CutoffTuned,
        // Envelopes (10)
        Env1Attack, Env1Decay, Env1Sustain, Env1Release, Env1Shape,
        Env2Attack, Env2Decay, Env2Sustain, Env2Release, Env2Shape,
        // Amp (1)
        AmpEnvBypass,
        // Layer mix (4)
        LayerLevel, LayerMute, LayerPan, LayerDetune,
        // LFO 1 (6)
        Lfo1Shape, Lfo1Rate, Lfo1Sync, Lfo1DelayTime, Lfo1Fade, Lfo1FreeRun,
        // LFO 2 (3)
        Lfo2Shape, Lfo2Rate, Lfo2Sync,
        // Voice (5)
        StackWidth, VoiceMode, Legato, UnisonDetune, PortamentoTime, Spread,
        // Matrix depths (16)
        MatrixSlot0Depth, MatrixSlot1Depth, MatrixSlot2Depth, MatrixSlot3Depth,
        MatrixSlot4Depth, MatrixSlot5Depth, MatrixSlot6Depth, MatrixSlot7Depth,
        MatrixSlot8Depth, MatrixSlot9Depth, MatrixSlot10Depth, MatrixSlot11Depth,
        MatrixSlot12Depth, MatrixSlot13Depth, MatrixSlot14Depth, MatrixSlot15Depth,
    ]
};

/// The single **global** params, in CLAP order — one instance, applied to both
/// synths (ADR 0002 §7): pitch-bend range, master level/tune/drift/limiter,
/// oversample, and the whole FX chain.
pub const GLOBAL_PARAMS: [ParamId; 35] = {
    use ParamId::*;
    [
        PitchBendRange,
        MasterTune, MasterVolume, MasterDrift, LimiterOn, Oversample,
        ChorusOn, ChorusRate, ChorusDepth, ChorusMix,
        PhaserOn, PhaserRate, PhaserDepth, PhaserFeedback, PhaserMix, PhaserStereo,
        DelayOn, DelayTime, DelayFeedback, DelayMix, DelaySync, DelayPingPong,
        ReverbOn, ReverbSize, ReverbDecay, ReverbDamp, ReverbMix,
        DynamicsOn, DynamicsThreshold, DynamicsRatio, DynamicsAttack,
        DynamicsRelease, DynamicsMakeup, DynamicsDrive, DynamicsMix,
    ]
};

/// Per-layer patch-param count — the UI's `patchCount` and the Layer-2 CLAP id
/// offset.
pub const PATCH_COUNT: usize = PATCH_PARAMS.len();

/// Global-param count (shared across layers).
pub const GLOBAL_COUNT: usize = GLOBAL_PARAMS.len();

/// What a CLAP id resolves to: a per-layer patch param or a shared global.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClapRef {
    /// A patch param on a specific layer → that synth's inner param.
    Patch(Layer, ParamId),
    /// A global param → applied to both synths.
    Global(ParamId),
}

impl ClapRef {
    /// The inner [`ParamId`] this CLAP id addresses (layer aside).
    #[inline]
    pub fn inner(self) -> ParamId {
        match self {
            ClapRef::Patch(_, p) | ClapRef::Global(p) => p,
        }
    }
}

/// Decode a CLAP id into its layer/global target, or `None` past the table.
#[inline]
pub fn clap_ref(clap_id: usize) -> Option<ClapRef> {
    if clap_id < PATCH_COUNT {
        Some(ClapRef::Patch(Layer::L1, PATCH_PARAMS[clap_id]))
    } else if clap_id < 2 * PATCH_COUNT {
        Some(ClapRef::Patch(Layer::L2, PATCH_PARAMS[clap_id - PATCH_COUNT]))
    } else if clap_id < TOTAL_PARAMS {
        Some(ClapRef::Global(GLOBAL_PARAMS[clap_id - 2 * PATCH_COUNT]))
    } else {
        None
    }
}

/// The CLAP id of a patch param on a given layer (its position in
/// [`PATCH_PARAMS`] offset by the layer). `None` if `p` is not a patch param.
pub fn patch_clap_id(layer: Layer, p: ParamId) -> Option<usize> {
    PATCH_PARAMS
        .iter()
        .position(|&q| q == p)
        .map(|pos| layer as usize * PATCH_COUNT + pos)
}

/// The CLAP id of a global param. `None` if `p` is not a global param.
pub fn global_clap_id(p: ParamId) -> Option<usize> {
    GLOBAL_PARAMS
        .iter()
        .position(|&q| q == p)
        .map(|pos| 2 * PATCH_COUNT + pos)
}

/// The CLAP id addressing inner `p` on `layer` — patch id if per-layer, else the
/// global id (layer ignored). Panics only if `p` is in neither table, which the
/// `partition` test rules out.
pub fn clap_id_of(layer: Layer, p: ParamId) -> usize {
    patch_clap_id(layer, p)
        .or_else(|| global_clap_id(p))
        .expect("every ParamId is either patch or global")
}

/// Host param-tree module label for a CLAP id: "Upper"/"Lower" for patch params,
/// "" for globals.
pub fn clap_module(clap_id: usize) -> &'static str {
    match clap_ref(clap_id) {
        Some(ClapRef::Patch(layer, _)) => layer.module(),
        _ => "",
    }
}

// ── Descriptor table ────────────────────────────────────────────────────────

const WAVE_LABELS: &[&str] = &["Sine", "Triangle", "Saw", "Pulse"];
const FILTER_MODE_LABELS: &[&str] = &["LP", "HP", "BP", "Notch"];
const SLOPE_LABELS: &[&str] = &["12", "24"];
const NOISE_LABELS: &[&str] = &["White", "Pink"];
const SHAPE_LABELS: &[&str] = &["Lin", "Exp"];
const LFO_LABELS: &[&str] = &["Sine", "Tri", "Saw+", "Saw-", "Square", "S&H"];
const OVERSAMPLE_LABELS: &[&str] = &["O/S OFF", "2x", "4x", "8x"];
/// Lanes per note. Labelled by the number itself — it is a count, not a name.
const WIDTH_LABELS: &[&str] = &["1", "2", "4", "8", "16", "32"];
const VOICE_MODE_LABELS: &[&str] = &["Poly", "Solo"];
/// PM is labelled "FM" — players expect that name (VXN1 ADR 0004 §3).
const CROSS_MOD_LABELS: &[&str] = &["Off", "Sync", "FM", "Ring"];

const fn f(
    name: &'static str,
    label: &'static str,
    min: f32,
    max: f32,
    default: f32,
    unit: &'static str,
    taper: Taper,
) -> ParamDesc {
    ParamDesc {
        name,
        label,
        min,
        max,
        default,
        kind: ParamKind::Float { unit, taper },
    }
}
const fn e(
    name: &'static str,
    label: &'static str,
    variants: &'static [&'static str],
    default: f32,
) -> ParamDesc {
    ParamDesc {
        name,
        label,
        min: 0.0,
        max: (variants.len() - 1) as f32,
        default,
        kind: ParamKind::Enum { variants },
    }
}
const fn b(name: &'static str, label: &'static str, default: f32) -> ParamDesc {
    ParamDesc {
        name,
        label,
        min: 0.0,
        max: 1.0,
        default,
        kind: ParamKind::Bool,
    }
}
const fn i(
    name: &'static str,
    label: &'static str,
    min: f32,
    max: f32,
    default: f32,
    unit: &'static str,
) -> ParamDesc {
    ParamDesc {
        name,
        label,
        min,
        max,
        default,
        kind: ParamKind::Int { unit },
    }
}
/// A bipolar matrix slot-depth descriptor (`[-1, 1]`, linear, unitless). The
/// evaluator scales this normalised depth per destination (0202).
const fn slot(name: &'static str, label: &'static str) -> ParamDesc {
    f(name, label, -1.0, 1.0, 0.0, "", Taper::Linear)
}

/// Pulse-width rails (VXN1): a width outside these degenerates to silence. They
/// bound the two PW params in [`PARAMS`] *and* the modulated width the render
/// cooks (`bank::cooked_pw`) — a matrix route must not be able to reach a duty
/// cycle the knob itself refuses.
pub const PW_MIN: f32 = 0.05;
pub const PW_MAX: f32 = 0.95;

/// The flat descriptor table. Index = [`ParamId`] discriminant = CLAP id.
/// Synthesis param ranges/defaults carry over verbatim from VXN1.
pub static PARAMS: [ParamDesc; ParamId::COUNT] = [
    // ── Osc / mixer ──
    e("osc1_wave", "Osc 1 Wave", WAVE_LABELS, 2.0),
    i("osc1_coarse", "Osc 1 Coarse", -7.0, 7.0, 0.0, "st"),
    f("osc1_fine", "Osc 1 Fine", -50.0, 50.0, 0.0, "ct", Taper::Linear),
    i("osc1_octave", "Osc 1 Octave", -4.0, 4.0, 0.0, "oct"),
    f("osc1_level", "Osc 1 Level", 0.0, 1.0, 0.8, "", Taper::Linear),
    f("osc1_pw", "Osc 1 PW", PW_MIN, PW_MAX, 0.5, "", Taper::Linear),
    e("osc2_wave", "Osc 2 Wave", WAVE_LABELS, 2.0),
    i("osc2_coarse", "Osc 2 Coarse", -7.0, 7.0, 0.0, "st"),
    f("osc2_fine", "Osc 2 Fine", -50.0, 50.0, 0.0, "ct", Taper::Linear),
    i("osc2_octave", "Osc 2 Octave", -4.0, 4.0, -1.0, "oct"),
    f("osc2_level", "Osc 2 Level", 0.0, 1.0, 0.6, "", Taper::Linear),
    f("osc2_pw", "Osc 2 PW", PW_MIN, PW_MAX, 0.5, "", Taper::Linear),
    f("sub_level", "Sub Level", 0.0, 1.0, 0.0, "", Taper::Linear),
    e("cross_mod_type", "Cross Mod", CROSS_MOD_LABELS, 0.0),
    f("cross_mod_amount", "Cross Mod Amt", 0.0, 4.0, 0.0, "", Taper::Linear),
    f("noise_level", "Noise Level", 0.0, 1.0, 0.0, "", Taper::Linear),
    e("noise_color", "Noise Colour", NOISE_LABELS, 0.0),
    // ── Filter ──
    f("cutoff", "Cutoff", 16.3516, 16000.0, 1000.0, "Hz", Taper::Exp { mid: 800.0 }),
    f("resonance", "Resonance", 0.0, 1.0, 0.2, "", Taper::Linear),
    f("drive", "Drive", 0.1, 4.0, 1.0, "", Taper::Exp { mid: 1.0 }),
    e("filter_mode", "Filter Mode", FILTER_MODE_LABELS, 0.0),
    e("filter_slope", "Filter Slope", SLOPE_LABELS, 1.0),
    f("hpf_cutoff", "HPF Cutoff", 20.0, 18000.0, 20.0, "Hz", Taper::Exp { mid: 1000.0 }),
    f("filter_key_track", "Key Track", 0.0, 1.0, 0.0, "", Taper::Linear),
    b("cutoff_tuned", "Tuned", 0.0),
    // ── Envelopes ──
    f("env1_attack", "Env 1 Attack", 0.001, 10.0, 0.005, "s", Taper::Exp { mid: 1.0 }),
    f("env1_decay", "Env 1 Decay", 0.001, 10.0, 0.3, "s", Taper::Exp { mid: 1.0 }),
    f("env1_sustain", "Env 1 Sustain", 0.0, 1.0, 0.0, "", Taper::Linear),
    f("env1_release", "Env 1 Release", 0.001, 10.0, 0.3, "s", Taper::Exp { mid: 1.0 }),
    e("env1_shape", "Env 1 Shape", SHAPE_LABELS, 0.0),
    f("env2_attack", "Env 2 Attack", 0.001, 10.0, 0.005, "s", Taper::Exp { mid: 1.0 }),
    f("env2_decay", "Env 2 Decay", 0.001, 10.0, 0.2, "s", Taper::Exp { mid: 1.0 }),
    f("env2_sustain", "Env 2 Sustain", 0.0, 1.0, 0.8, "", Taper::Linear),
    f("env2_release", "Env 2 Release", 0.001, 10.0, 0.3, "s", Taper::Exp { mid: 1.0 }),
    e("env2_shape", "Env 2 Shape", SHAPE_LABELS, 1.0),
    // ── Amp ──
    b("amp_env_bypass", "Amp Gate", 0.0),
    // ── Layer mix (0220) ──
    // Unity default so a layer switched on sits at full level — turning Layer 2
    // on must not require finding a fader before anything is heard.
    f("layer_level", "Layer Level", 0.0, 1.0, 1.0, "", Taper::Linear),
    b("layer_mute", "Layer Mute", 0.0),
    // Centre default: a layer switched on sits where it was placed by the
    // player, not off to one side. Same bipolar shape as a matrix slot depth.
    f("layer_pan", "Layer Pan", -1.0, 1.0, 0.0, "", Taper::Linear),
    // ±50 ct, but the musical range is the inner part of it: past ~25 ct two
    // layers read as out of tune rather than wide. `BipolarExp { mid: 20 }`
    // puts ±20 ct at half travel each way, so the useful span occupies most of
    // the slider and the extremes stay reachable (0263).
    f("layer_detune", "Layer Detune", -50.0, 50.0, 0.0, "ct", Taper::BipolarExp { mid: 20.0 }),
    // ── LFO 1 ──
    e("lfo1_shape", "LFO 1 Shape", LFO_LABELS, 0.0),
    f("lfo1_rate", "LFO 1 Rate", 0.01, 40.0, 5.0, "Hz", Taper::Exp { mid: 5.0 }),
    b("lfo1_sync", "LFO 1 Sync", 0.0),
    f("lfo1_delay_time", "LFO 1 Delay", 0.0, 4.0, 0.0, "s", Taper::Linear),
    f("lfo1_fade", "LFO 1 Fade", 0.0, 4.0, 0.0, "s", Taper::Linear),
    b("lfo1_free_run", "LFO 1 Free", 0.0),
    // ── LFO 2 ──
    e("lfo2_shape", "LFO 2 Shape", LFO_LABELS, 0.0),
    f("lfo2_rate", "LFO 2 Rate", 0.01, 40.0, 5.0, "Hz", Taper::Exp { mid: 5.0 }),
    b("lfo2_sync", "LFO 2 Sync", 0.0),
    // ── Pitch bend (hardwired, ADR §3) — was VXN1's PitchWheelDepth ──
    f("pitch_bend_range", "Bend Range", 0.0, 12.0, 2.0, "st", Taper::Linear),
    // ── Voice ──
    e("stack_width", "Width", WIDTH_LABELS, 0.0),
    e("voice_mode", "Voice", VOICE_MODE_LABELS, 0.0),
    b("legato", "Legato", 0.0),
    f("unison_detune", "Detune", 0.0, 50.0, 12.0, "ct", Taper::Linear),
    f("portamento_time", "Glide Time", 0.0, 0.5, 0.0, "s", Taper::Exp { mid: 0.1 }),
    f("spread", "Spread", 0.0, 1.0, 0.0, "", Taper::Linear),
    // ── Master ──
    f("master_tune", "Master Tune", -12.0, 12.0, 0.0, "st", Taper::Linear),
    f("master_volume", "Volume", 0.0, 1.0, 0.7, "", Taper::Linear),
    f("master_drift", "Drift", 0.0, 1.0, 0.0, "", Taper::Linear),
    b("limiter_on", "Limiter", 0.0),
    e("oversample", "Oversample", OVERSAMPLE_LABELS, 1.0),
    // ── FX chain (chorus → phaser → delay → reverb → dynamics, 0207) ──
    // Every effect defaults off/neutral so the factory patch is FX-free. Ranges
    // mirror VXN1's FX section; the dynamics eight mirror VXN2's kernel clamps.
    b("chorus_on", "Chorus", 0.0),
    f("chorus_rate", "Chorus Rate", 0.05, 8.0, 0.6, "Hz", Taper::Linear),
    f("chorus_depth", "Chorus Depth", 0.0, 1.0, 0.5, "", Taper::Linear),
    f("chorus_mix", "Chorus Mix", 0.0, 1.0, 0.4, "", Taper::Linear),
    b("phaser_on", "Phaser", 0.0),
    f("phaser_rate", "Phaser Rate", 0.05, 10.0, 0.5, "Hz", Taper::Exp { mid: 1.0 }),
    f("phaser_depth", "Phaser Depth", 0.0, 1.0, 0.7, "", Taper::Linear),
    f("phaser_fb", "Phaser FB", -0.9, 0.9, 0.0, "", Taper::Linear),
    f("phaser_mix", "Phaser Mix", 0.0, 1.0, 0.5, "", Taper::Linear),
    // L/R sweep offset (0277). 180° is the anti-phase sweep the kernel was
    // pinned to, so it is the default and existing patches are unchanged; 0°
    // sweeps both cascades in lockstep (near-mono).
    f("phaser_stereo", "Phaser Stereo", 0.0, 180.0, 180.0, "°", Taper::Linear),
    b("delay_on", "Delay", 0.0),
    f("delay_time", "Delay Time", 0.01, 2.0, 0.35, "s", Taper::Linear),
    f("delay_feedback", "Delay FB", 0.0, 0.95, 0.4, "", Taper::Linear),
    f("delay_mix", "Delay Mix", 0.0, 1.0, 0.25, "", Taper::Linear),
    // Tempo sync (0267): when on, Delay Time's fader position selects a musical
    // subdivision instead of literal seconds. Same rate/sync pairing as the two
    // LFOs — see `crate::sync`.
    b("delay_sync", "Delay Sync", 0.0),
    // Feedback crossfeed (0277). On is how the kernel has always run, so it
    // stays the default; off keeps each line's feedback on its own side.
    b("delay_pingpong", "Ping-Pong", 1.0),
    b("reverb_on", "Reverb", 0.0),
    f("reverb_size", "Reverb Size", 0.0, 1.0, 0.5, "", Taper::Linear),
    f("reverb_decay", "Reverb Decay", 0.2, 10.0, 2.5, "s", Taper::Exp { mid: 2.0 }),
    f("reverb_damp", "Reverb Damp", 0.0, 1.0, 0.4, "", Taper::Linear),
    f("reverb_mix", "Reverb Mix", 0.0, 1.0, 0.3, "", Taper::Linear),
    b("dynamics_on", "Dynamics", 0.0),
    f("dynamics_threshold", "Dyn Threshold", -60.0, 0.0, -12.0, "dB", Taper::Linear),
    f("dynamics_ratio", "Dyn Ratio", 1.0, 20.0, 4.0, "", Taper::Linear),
    f("dynamics_attack", "Dyn Attack", 0.1, 200.0, 10.0, "ms", Taper::Exp { mid: 10.0 }),
    f("dynamics_release", "Dyn Release", 5.0, 1000.0, 100.0, "ms", Taper::Exp { mid: 100.0 }),
    f("dynamics_makeup", "Dyn Makeup", 0.0, 24.0, 0.0, "dB", Taper::Linear),
    f("dynamics_drive", "Dyn Drive", 0.0, 36.0, 0.0, "dB", Taper::Linear),
    f("dynamics_mix", "Dyn Mix", 0.0, 1.0, 1.0, "", Taper::Linear),
    // ── Matrix slot depths ──
    slot("matrix_slot0_depth", "Slot 1 Depth"),
    slot("matrix_slot1_depth", "Slot 2 Depth"),
    slot("matrix_slot2_depth", "Slot 3 Depth"),
    slot("matrix_slot3_depth", "Slot 4 Depth"),
    slot("matrix_slot4_depth", "Slot 5 Depth"),
    slot("matrix_slot5_depth", "Slot 6 Depth"),
    slot("matrix_slot6_depth", "Slot 7 Depth"),
    slot("matrix_slot7_depth", "Slot 8 Depth"),
    slot("matrix_slot8_depth", "Slot 9 Depth"),
    slot("matrix_slot9_depth", "Slot 10 Depth"),
    slot("matrix_slot10_depth", "Slot 11 Depth"),
    slot("matrix_slot11_depth", "Slot 12 Depth"),
    slot("matrix_slot12_depth", "Slot 13 Depth"),
    slot("matrix_slot13_depth", "Slot 14 Depth"),
    slot("matrix_slot14_depth", "Slot 15 Depth"),
    slot("matrix_slot15_depth", "Slot 16 Depth"),
];

/// Total CLAP-exposed params: two per-layer patch blocks + one global block
/// (ADR 0002 §4). **Not** the inner per-synth count ([`ParamId::COUNT`]) — the
/// CLAP surface is the two-layer expansion.
pub const TOTAL_PARAMS: usize = 2 * PATCH_COUNT + GLOBAL_COUNT;

/// Descriptor for a CLAP id, or `None` past the table. Layer 1 and Layer 2 share
/// the inner descriptor (same name/range); the host tells them apart by the
/// module label ([`clap_module`]).
#[inline]
pub fn desc_for_clap_id(clap_id: usize) -> Option<&'static ParamDesc> {
    clap_ref(clap_id).map(|r| r.inner().desc())
}

// ── Value store ─────────────────────────────────────────────────────────────

#[inline]
fn enum_index(value: f32, max: usize) -> usize {
    (value.round() as usize).min(max)
}

/// The engine-side `f32` value store: one flat block seeded from descriptor
/// defaults. CLAP id = index, so [`Self::get_index`]/[`Self::set_index`] are the
/// host boundary; typed [`Self::get`]/[`Self::set`] are the engine's. `set`
/// clamps to the descriptor range. DSP-typed readers resolve enum params back to
/// their `vxn_dsp` types without a second match table.
#[derive(Clone, Debug)]
pub struct Params {
    v: [f32; ParamId::COUNT],
}

impl Default for Params {
    fn default() -> Self {
        let mut v = [0.0; ParamId::COUNT];
        for (idx, d) in PARAMS.iter().enumerate() {
            v[idx] = d.default;
        }
        Self { v }
    }
}

impl Params {
    #[inline]
    pub fn get(&self, p: ParamId) -> f32 {
        self.v[p.index()]
    }

    #[inline]
    pub fn get_index(&self, index: usize) -> f32 {
        self.v[index]
    }

    #[inline]
    pub fn set(&mut self, p: ParamId, value: f32) {
        self.v[p.index()] = p.desc().clamp(value);
    }

    #[inline]
    pub fn set_index(&mut self, index: usize, value: f32) {
        if let Some(p) = ParamId::from_index(index) {
            self.set(p, value);
        }
    }

    #[inline]
    pub fn bool(&self, p: ParamId) -> bool {
        self.get(p) >= 0.5
    }

    /// Depth of matrix slot `slot` (`0..MATRIX_SLOTS`); `0.0` past the range.
    #[inline]
    pub fn slot_depth(&self, slot: usize) -> f32 {
        ParamId::slot_depth(slot).map_or(0.0, |p| self.get(p))
    }

    pub fn osc_wave(&self, p: ParamId) -> Waveform {
        Waveform::ALL[enum_index(self.get(p), Waveform::ALL.len() - 1)]
    }

    pub fn filter_mode(&self) -> FilterMode {
        FilterMode::ALL[enum_index(self.get(ParamId::FilterMode), FilterMode::COUNT - 1)]
    }

    pub fn filter_slope(&self) -> FilterSlope {
        if enum_index(self.get(ParamId::FilterSlope), 1) == 0 {
            FilterSlope::Pole2
        } else {
            FilterSlope::Pole4
        }
    }

    pub fn noise_color(&self) -> NoiseColor {
        NoiseColor::ALL[enum_index(self.get(ParamId::NoiseColor), NoiseColor::ALL.len() - 1)]
    }

    pub fn lfo1_shape(&self) -> LfoShape {
        LfoShape::ALL[enum_index(self.get(ParamId::Lfo1Shape), LfoShape::ALL.len() - 1)]
    }

    pub fn lfo2_shape(&self) -> LfoShape {
        LfoShape::ALL[enum_index(self.get(ParamId::Lfo2Shape), LfoShape::ALL.len() - 1)]
    }

    pub fn stack_width(&self) -> StackWidth {
        StackWidth::from_index(enum_index(self.get(ParamId::StackWidth), StackWidth::COUNT - 1))
    }

    pub fn voice_mode(&self) -> VoiceMode {
        VoiceMode::from_index(enum_index(self.get(ParamId::VoiceMode), VoiceMode::COUNT - 1))
    }

    pub fn cross_mod_type(&self) -> CrossModType {
        CrossModType::from_index(enum_index(
            self.get(ParamId::CrossModType),
            CrossModType::COUNT - 1,
        ))
    }

    pub fn env1_shape(&self) -> AdsrShape {
        self.adsr_shape(ParamId::Env1Shape)
    }

    pub fn env2_shape(&self) -> AdsrShape {
        self.adsr_shape(ParamId::Env2Shape)
    }

    fn adsr_shape(&self, p: ParamId) -> AdsrShape {
        if enum_index(self.get(p), 1) == 0 {
            AdsrShape::Linear
        } else {
            AdsrShape::Exponential
        }
    }

    pub fn oversample_factor(&self) -> usize {
        match enum_index(self.get(ParamId::Oversample), 3) {
            0 => 1,
            1 => 2,
            2 => 4,
            _ => 8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_len_matches_count() {
        assert_eq!(PARAMS.len(), ParamId::COUNT);
    }

    #[test]
    fn index_roundtrips_for_every_param() {
        for p in ParamId::all() {
            assert_eq!(ParamId::from_index(p.index()), Some(p));
        }
        assert_eq!(ParamId::from_index(ParamId::COUNT), None);
    }

    #[test]
    fn every_param_formats() {
        // Total display/format coverage: every param renders its default, min
        // and max to a non-empty string (no panic, no gap).
        for d in PARAMS.iter() {
            for v in [d.default, d.min, d.max] {
                assert!(!d.display(v).is_empty(), "{} failed to format {v}", d.name);
            }
        }
    }

    #[test]
    fn defaults_in_range() {
        // Iterate the CLAP surface; resolve each id to its inner default.
        let p = Params::default();
        for id in 0..TOTAL_PARAMS {
            let d = desc_for_clap_id(id).unwrap();
            let val = p.get(clap_ref(id).unwrap().inner());
            assert!(val >= d.min && val <= d.max, "{} default OOR", d.name);
        }
    }

    #[test]
    fn patch_and_global_partition_every_param() {
        // The two CLAP blocks together cover every inner ParamId exactly once —
        // no param is both per-layer and global, none is dropped.
        assert_eq!(PATCH_COUNT + GLOBAL_COUNT, ParamId::COUNT, "partition size");
        for p in ParamId::all() {
            let in_patch = PATCH_PARAMS.contains(&p);
            let in_global = GLOBAL_PARAMS.contains(&p);
            assert!(in_patch ^ in_global, "{p:?} must be exactly one of patch/global");
        }
        assert_eq!(TOTAL_PARAMS, 2 * 71 + 35);
    }

    #[test]
    fn clap_ref_layout_is_l1_l2_globals() {
        // Layer 1 patch block, then Layer 2 (same list, offset), then globals.
        assert_eq!(clap_ref(0), Some(ClapRef::Patch(Layer::L1, PATCH_PARAMS[0])));
        assert_eq!(
            clap_ref(PATCH_COUNT),
            Some(ClapRef::Patch(Layer::L2, PATCH_PARAMS[0]))
        );
        assert_eq!(
            clap_ref(2 * PATCH_COUNT),
            Some(ClapRef::Global(GLOBAL_PARAMS[0]))
        );
        assert_eq!(clap_ref(TOTAL_PARAMS), None);
        // Round-trip the id helpers for every param.
        for p in PATCH_PARAMS {
            for layer in Layer::ALL {
                let id = clap_id_of(layer, p);
                assert_eq!(clap_ref(id), Some(ClapRef::Patch(layer, p)));
            }
        }
        for p in GLOBAL_PARAMS {
            let id = clap_id_of(Layer::L1, p);
            assert_eq!(clap_ref(id), Some(ClapRef::Global(p)));
        }
    }

    #[test]
    fn module_labels_split_by_layer() {
        assert_eq!(clap_module(0), "Upper");
        assert_eq!(clap_module(PATCH_COUNT), "Lower");
        assert_eq!(clap_module(2 * PATCH_COUNT), ""); // global
    }

    #[test]
    fn names_are_unique() {
        for (i, a) in PARAMS.iter().enumerate() {
            for b in PARAMS.iter().skip(i + 1) {
                assert_ne!(a.name, b.name, "duplicate param name {}", a.name);
            }
        }
    }

    #[test]
    fn enum_display_and_parse_roundtrip() {
        let d = ParamId::Osc1Wave.desc();
        assert_eq!(d.display(2.0), "Saw");
        assert_eq!(d.variant_index("saw"), Some(2));
        assert_eq!(d.parse("Pulse"), Some(3.0));
        let cm = ParamId::CrossModType.desc();
        assert_eq!(cm.display(2.0), "FM"); // PM labelled FM
    }

    #[test]
    fn fixed_mod_panel_params_are_gone() {
        // The matrix replaces every fixed-route selector/depth of VXN1 ADR 0004.
        for gone in [
            "pitch_lfo_src",
            "pitch_lfo_depth",
            "pitch_env_src",
            "pitch_env_depth",
            "pitch_wheel_depth",
            "pwm_lfo_src",
            "pwm_lfo_depth",
            "pwm_env_src",
            "pwm_env_depth",
            "cutoff_lfo1_depth",
            "cutoff_lfo2_depth",
            "cutoff_env_depth",
            "vel_cutoff_depth",
            "amp_lfo_src",
            "amp_lfo_depth",
            "mod_wheel_pwm",
            "mod_wheel_cutoff",
            "mod_wheel_reso",
            "mod_wheel_cross_mod_sweep",
        ] {
            assert!(ParamId::from_name(gone).is_none(), "{gone} should be removed");
        }
    }

    #[test]
    fn layer_mix_params_are_per_layer() {
        // 0220/0248: level, mute and pan are PATCH params, not globals, so the
        // two-layer expansion gives each synth its own — a preset carries its
        // own mix.
        for p in [ParamId::LayerLevel, ParamId::LayerMute, ParamId::LayerPan] {
            assert!(PATCH_PARAMS.contains(&p), "{p:?} must be a patch param");
            assert!(!GLOBAL_PARAMS.contains(&p), "{p:?} must not be global");
            assert_ne!(
                clap_id_of(Layer::L1, p),
                clap_id_of(Layer::L2, p),
                "{p:?} must have a distinct id per layer"
            );
        }
        // Unity default: switching a layer on must be audible without hunting
        // for a fader first.
        assert_eq!(ParamId::LayerLevel.desc().default, 1.0);
        assert_eq!(ParamId::LayerMute.desc().default, 0.0);
        // Pan is bipolar and defaults to centre.
        let pan = ParamId::LayerPan.desc();
        assert_eq!((pan.min, pan.max, pan.default), (-1.0, 1.0, 0.0));
    }

    #[test]
    fn sixteen_bipolar_slot_depths_exist() {
        for s in 0..MATRIX_SLOTS {
            let p = ParamId::slot_depth(s).expect("slot depth exists");
            let d = p.desc();
            // Automatable Float, bipolar around zero.
            assert!(matches!(d.kind, ParamKind::Float { .. }), "slot {s} not float");
            assert_eq!((d.min, d.max, d.default), (-1.0, 1.0, 0.0));
        }
        assert!(ParamId::slot_depth(MATRIX_SLOTS).is_none());
    }

    #[test]
    fn slot_depth_index_is_the_inverse_of_slot_depth() {
        for s in 0..MATRIX_SLOTS {
            let id = ParamId::slot_depth(s).unwrap().index();
            assert_eq!(ParamId::slot_depth_index(id), Some(s));
        }
        // Non-slot ids (below the slot block and past the table) return None
        // without underflowing — the `then` vs `then_some` trap (0205).
        assert_eq!(ParamId::slot_depth_index(ParamId::Cutoff as usize), None);
        assert_eq!(ParamId::slot_depth_index(0), None);
        assert_eq!(ParamId::slot_depth_index(ParamId::COUNT), None);
    }

    #[test]
    fn pitch_bend_range_is_a_hardwired_global() {
        // §3: bend range stays a dedicated param (not a matrix route).
        let p = ParamId::from_name("pitch_bend_range").expect("bend range present");
        assert_eq!(p, ParamId::PitchBendRange);
        assert_eq!((p.desc().min, p.desc().max, p.desc().default), (0.0, 12.0, 2.0));
    }

    #[test]
    fn set_clamps_to_range() {
        let mut p = Params::default();
        p.set(ParamId::Resonance, 5.0);
        assert_eq!(p.get(ParamId::Resonance), 1.0);
        p.set(ParamId::Cutoff, -100.0);
        assert_eq!(p.get(ParamId::Cutoff), ParamId::Cutoff.desc().min);
    }

    #[test]
    fn typed_enum_readers_resolve() {
        let mut p = Params::default();
        p.set(ParamId::Osc1Wave, 2.0);
        assert_eq!(p.osc_wave(ParamId::Osc1Wave), Waveform::ALL[2]);
        p.set(ParamId::CrossModType, 1.0);
        assert_eq!(p.cross_mod_type(), CrossModType::Sync);
        p.set(ParamId::Oversample, 2.0);
        assert_eq!(p.oversample_factor(), 4);
        p.set(ParamId::StackWidth, 4.0);
        assert_eq!(p.stack_width(), StackWidth::Sixteen);
        assert_eq!(p.stack_width().lanes(), 16);
        p.set(ParamId::VoiceMode, 1.0);
        assert_eq!(p.voice_mode(), VoiceMode::Solo);
    }
}
