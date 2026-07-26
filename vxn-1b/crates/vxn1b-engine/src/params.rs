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
//! - **FX params deferred** to E037 (0206) — not in this table yet.

use vxn_core_app::{ParamDesc, ParamKind, Taper};
use vxn_dsp::{AdsrShape, FilterMode, FilterSlope, LfoShape, NoiseColor, Waveform};

// ── Param-value enums (variant indices stored as f32) ───────────────────────

/// Voice-assignment mode. Forked from VXN1 (`vxn-app`) — the engine needs it to
/// resolve the `AssignMode` param without depending on VXN1's app crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(usize)]
pub enum AssignMode {
    #[default]
    Poly,
    Unison,
    Solo,
    Twin,
}

impl AssignMode {
    pub const COUNT: usize = AssignMode::Twin as usize + 1;

    pub fn from_index(i: usize) -> AssignMode {
        match i {
            1 => AssignMode::Unison,
            2 => AssignMode::Solo,
            3 => AssignMode::Twin,
            _ => AssignMode::Poly,
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
    AssignMode,
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
}

// ── Descriptor table ────────────────────────────────────────────────────────

const WAVE_LABELS: &[&str] = &["Sine", "Triangle", "Saw", "Pulse"];
const FILTER_MODE_LABELS: &[&str] = &["LP", "HP", "BP", "Notch"];
const SLOPE_LABELS: &[&str] = &["12", "24"];
const NOISE_LABELS: &[&str] = &["White", "Pink"];
const SHAPE_LABELS: &[&str] = &["Lin", "Exp"];
const LFO_LABELS: &[&str] = &["Sine", "Tri", "Saw+", "Saw-", "Square", "S&H"];
const OVERSAMPLE_LABELS: &[&str] = &["O/S OFF", "2x", "4x", "8x"];
const ASSIGN_LABELS: &[&str] = &["Poly", "Unison", "Solo", "Twin"];
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

/// The flat descriptor table. Index = [`ParamId`] discriminant = CLAP id.
/// Synthesis param ranges/defaults carry over verbatim from VXN1.
pub static PARAMS: [ParamDesc; ParamId::COUNT] = [
    // ── Osc / mixer ──
    e("osc1_wave", "Osc 1 Wave", WAVE_LABELS, 2.0),
    i("osc1_coarse", "Osc 1 Coarse", -7.0, 7.0, 0.0, "st"),
    f("osc1_fine", "Osc 1 Fine", -50.0, 50.0, 0.0, "ct", Taper::Linear),
    i("osc1_octave", "Osc 1 Octave", -4.0, 4.0, 0.0, "oct"),
    f("osc1_level", "Osc 1 Level", 0.0, 1.0, 0.8, "", Taper::Linear),
    f("osc1_pw", "Osc 1 PW", 0.05, 0.95, 0.5, "", Taper::Linear),
    e("osc2_wave", "Osc 2 Wave", WAVE_LABELS, 2.0),
    i("osc2_coarse", "Osc 2 Coarse", -7.0, 7.0, 0.0, "st"),
    f("osc2_fine", "Osc 2 Fine", -50.0, 50.0, 0.0, "ct", Taper::Linear),
    i("osc2_octave", "Osc 2 Octave", -4.0, 4.0, -1.0, "oct"),
    f("osc2_level", "Osc 2 Level", 0.0, 1.0, 0.6, "", Taper::Linear),
    f("osc2_pw", "Osc 2 PW", 0.05, 0.95, 0.5, "", Taper::Linear),
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
    e("assign_mode", "Assign", ASSIGN_LABELS, 0.0),
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

/// Total CLAP-exposed params. Identity map: CLAP id = [`ParamId`] index.
pub const TOTAL_PARAMS: usize = ParamId::COUNT;

/// Descriptor for a CLAP id, or `None` past the table.
#[inline]
pub fn desc_for_clap_id(clap_id: usize) -> Option<&'static ParamDesc> {
    ParamId::from_index(clap_id).map(ParamId::desc)
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

    pub fn assign_mode(&self) -> AssignMode {
        AssignMode::from_index(enum_index(self.get(ParamId::AssignMode), AssignMode::COUNT - 1))
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
        for id in 0..TOTAL_PARAMS {
            let d = desc_for_clap_id(id).unwrap();
            let val = Params::default().get_index(id);
            assert!(val >= d.min && val <= d.max, "{} default OOR", d.name);
        }
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
            "filter_key_track",
            "layer_level",
        ] {
            assert!(ParamId::from_name(gone).is_none(), "{gone} should be removed");
        }
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
        p.set(ParamId::AssignMode, 1.0);
        assert_eq!(p.assign_mode(), AssignMode::Unison);
    }
}
