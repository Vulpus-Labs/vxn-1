//! Mod-matrix data model + seeded default patch (ticket 0201).
//!
//! The routing model for VXN1b, adapted from VXN2's matrix
//! ([`vxn-2/crates/vxn2-engine/src/matrix.rs`]) with VXN1's source/destination
//! sets (ADR 0001 §2–§3). This ticket is **types + defaults only** — evaluation
//! (source fan-out, curve/scale application, dest smoothing) is 0202.
//!
//! VXN1b is a **flat 16-voice** instrument, so — unlike VXN2 — there are no
//! stacks/lanes and no granularity *tiers* or coherence rules: every source and
//! destination is a per-voice (or patch-global) scalar the evaluator will read
//! once per voice per control block.
//!
//! A slot is `(source, dest, depth, curve, scale_src)`. `depth` mirrors the
//! CLAP param (0200); the other four fields are **patch topology** (state + TOML,
//! 0203). Slots to the same dest sum (additive) — the evaluator's job.

use crate::params::MATRIX_SLOTS;

/// Slot count — the single source of truth is the param table's slot-depth count
/// ([`crate::params::MATRIX_SLOTS`]), so the topology and the automatable depths
/// can never disagree on how many slots exist.
pub const N_SLOTS: usize = MATRIX_SLOTS;

// ── SourceId ────────────────────────────────────────────────────────────────

/// Modulation source. `None` is the empty-slot sentinel (index 0); a slot whose
/// source is `None` is inert and skipped by the evaluator.
///
/// The ten real sources are VXN1's fixed-route inputs (Env/LFO/Velocity/Key/
/// wheels) plus the two VXN1 lacks: `Aftertouch` (MPE per-voice pressure from
/// 0198) and `NoteRandom` (per-voice latch from 0199).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum SourceId {
    #[default]
    None = 0,
    Env1 = 1,
    Env2 = 2,
    Lfo1 = 3,
    Lfo2 = 4,
    Velocity = 5,
    Key = 6,
    ModWheel = 7,
    PitchWheel = 8,
    Aftertouch = 9,
    NoteRandom = 10,
}

/// Count of non-sentinel sources (`None` excluded).
pub const N_SOURCES: usize = 10;

impl SourceId {
    /// Index into a per-voice source lookup, or `None` for the sentinel.
    #[inline]
    pub const fn idx(self) -> Option<usize> {
        match self {
            SourceId::None => None,
            _ => Some(self as usize - 1),
        }
    }

    /// Decode a wire-format `u8`. Out-of-range → [`SourceId::None`] so a corrupt
    /// patch blob degrades to an inert slot rather than panicking (0203).
    #[inline]
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => SourceId::Env1,
            2 => SourceId::Env2,
            3 => SourceId::Lfo1,
            4 => SourceId::Lfo2,
            5 => SourceId::Velocity,
            6 => SourceId::Key,
            7 => SourceId::ModWheel,
            8 => SourceId::PitchWheel,
            9 => SourceId::Aftertouch,
            10 => SourceId::NoteRandom,
            _ => SourceId::None,
        }
    }

    /// Whether this source emits a **bipolar** `[-1, 1]` shape (vs a unipolar
    /// `[0, 1]` one). Consumed by the evaluator's `scale_norm` (0202) to fold a
    /// bipolar scale source into the `[0, 1]` VCA range. **Exhaustive match** —
    /// a new source forces a polarity decision at compile time (the
    /// `is_bipolar` discipline of VXN2 ADR 0009).
    ///
    /// - **Bipolar:** `Lfo1`, `Lfo2`, `PitchWheel` — genuinely swing ±.
    /// - **Unipolar:** `Env1`, `Env2`, `Velocity`, `Key`, `ModWheel`,
    ///   `Aftertouch`, `NoteRandom`. The envelopes are `[0, 1]` ADSR shapes:
    ///   treating them as bipolar would map `[0, 1]` through `(x+1)/2` → `[0.5,
    ///   1]` as a scale VCA and never gate to zero (the same trap VXN2 flags for
    ///   `VoiceRand`), so they stay unipolar passthrough. `Key` is unipolar for
    ///   *scale* purposes; as a primary source its value is signed semitones rel
    ///   C4 (see [`default_patch`]).
    #[inline]
    pub const fn is_bipolar(self) -> bool {
        match self {
            SourceId::Lfo1 | SourceId::Lfo2 | SourceId::PitchWheel => true,
            SourceId::None
            | SourceId::Env1
            | SourceId::Env2
            | SourceId::Velocity
            | SourceId::Key
            | SourceId::ModWheel
            | SourceId::Aftertouch
            | SourceId::NoteRandom => false,
        }
    }
}

/// Source machine id (kebab-case wire name). Index = `SourceId as u8`.
pub const SOURCE_NAMES: [&str; N_SOURCES + 1] = [
    "none", "env1", "env2", "lfo1", "lfo2", "velocity", "key", "mod-wheel", "pitch-wheel",
    "aftertouch", "note-random",
];

/// Source display label. Same indexing as [`SOURCE_NAMES`].
pub const SOURCE_LABELS: [&str; N_SOURCES + 1] = [
    "—", "Env 1", "Env 2", "LFO 1", "LFO 2", "Velocity", "Key", "Mod Wheel", "Pitch Wheel",
    "Aftertouch", "Note Rnd",
];

// ── DestId ──────────────────────────────────────────────────────────────────

/// Modulation destination. `None` is the empty-slot sentinel. The core v1 dest
/// set (ADR 0001 §2): the continuous synthesis targets the fixed VXN1 routes
/// used to reach. `XModSweep` is the wide, mode-aware osc-sweep target (inherits
/// VXN1 ADR 0004's mode-gated behaviour); `CrossModAmount` modulates the FM/sync
/// index.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum DestId {
    #[default]
    None = 0,
    Pitch = 1,
    XModSweep = 2,
    Pwm = 3,
    Cutoff = 4,
    Resonance = 5,
    HpfCutoff = 6,
    Amp = 7,
    CrossModAmount = 8,
}

/// Count of non-sentinel destinations.
pub const N_DESTS: usize = 8;

impl DestId {
    /// Index into a per-voice dest accumulator, or `None` for the sentinel.
    #[inline]
    pub const fn idx(self) -> Option<usize> {
        match self {
            DestId::None => None,
            _ => Some(self as usize - 1),
        }
    }

    /// Decode a wire-format `u8`. Out-of-range → [`DestId::None`].
    #[inline]
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => DestId::Pitch,
            2 => DestId::XModSweep,
            3 => DestId::Pwm,
            4 => DestId::Cutoff,
            5 => DestId::Resonance,
            6 => DestId::HpfCutoff,
            7 => DestId::Amp,
            8 => DestId::CrossModAmount,
            _ => DestId::None,
        }
    }
}

/// Destination machine id (kebab-case wire name). Index = `DestId as u8`.
pub const DEST_NAMES: [&str; N_DESTS + 1] = [
    "none", "pitch", "xmod-sweep", "pwm", "cutoff", "resonance", "hpf-cutoff", "amp",
    "cross-mod-amount",
];

/// Destination display label. Same indexing as [`DEST_NAMES`].
pub const DEST_LABELS: [&str; N_DESTS + 1] = [
    "—", "Pitch", "X-Mod Sweep", "PWM", "Cutoff", "Resonance", "HPF Cutoff", "Amp", "Cross-Mod Amt",
];

// ── Curve ───────────────────────────────────────────────────────────────────

/// Curve applied to a source value before depth scaling (per VXN2's model).
///
/// - `Lin` — identity passthrough.
/// - `Exp` — signed square `sign(v)·v²`: more extreme excursions.
/// - `Log` — signed root `sign(v)·√|v|`: compresses toward 0.
/// - `Bipolar` — AC-couple a unipolar `[0, 1]` source to `[-1, 1]` via `2v − 1`
///   (centred swing when routing mod-wheel/aftertouch into a bipolar dest).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Curve {
    #[default]
    Lin = 0,
    Exp = 1,
    Log = 2,
    Bipolar = 3,
}

/// Count of curve variants.
pub const N_CURVES: usize = 4;

impl Curve {
    /// Decode a wire-format `u8`. Out-of-range → [`Curve::Lin`].
    #[inline]
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Curve::Exp,
            2 => Curve::Log,
            3 => Curve::Bipolar,
            _ => Curve::Lin,
        }
    }
}

/// Curve machine id. Index = `Curve as u8`.
pub const CURVE_NAMES: [&str; N_CURVES] = ["lin", "exp", "log", "bipolar"];

/// Curve display label. Same indexing as [`CURVE_NAMES`].
pub const CURVE_LABELS: [&str; N_CURVES] = ["Lin", "Exp", "Log", "Bipolar"];

// ── MatrixSlot / MatrixTable ────────────────────────────────────────────────

/// One matrix route. `depth` mirrors the slot's CLAP param (0200, bipolar
/// `[-1, 1]`); `source`/`dest`/`curve`/`scale_src` are patch topology.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatrixSlot {
    pub source: SourceId,
    pub dest: DestId,
    pub depth: f32,
    pub curve: Curve,
    /// Optional secondary "scale" source — the per-route VCA of VXN2 ADR 0009.
    /// When non-`None`, the slot's contribution is multiplied by this source's
    /// value normalised to `[0, 1]` (evaluator, 0202), e.g. mod-wheel gating an
    /// LFO→pitch vibrato. `None` is identity. A *leaf* value (read from the same
    /// per-voice source table), so it can never form a cycle.
    pub scale_src: SourceId,
}

impl Default for MatrixSlot {
    fn default() -> Self {
        Self {
            source: SourceId::None,
            dest: DestId::None,
            depth: 0.0,
            curve: Curve::Lin,
            scale_src: SourceId::None,
        }
    }
}

impl MatrixSlot {
    /// A slot is **active** (contributes to a dest) only when both endpoints are
    /// real. The evaluator additionally skips `depth == 0` slots; an inactive
    /// slot here is inert regardless of depth.
    #[inline]
    pub fn is_active(&self) -> bool {
        self.source != SourceId::None && self.dest != DestId::None
    }
}

/// The 16-slot patch topology + depths.
#[derive(Clone, Copy, Debug, PartialEq)]
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

/// Key→Cutoff depth that reproduces **exactly 1 octave of cutoff per octave of
/// key** (ADR 0001 §3), under the convention **locked by the 0202 evaluator**
/// ([`crate::eval`]):
///
/// - the **Key** source emits signed **octaves relative to C4** (`(note−60)/12`);
/// - the **Cutoff** destination has native-unit gain **48 semitones**
///   ([`crate::eval::DEST_GAIN`]).
///
/// Key→Cutoff at depth `d` shifts cutoff by `octaves_from_c4 · d · 48`
/// semitones; over one octave of key (`octaves_from_c4` changes by 1) that is
/// `d · 48` semitones. For 12 st (1 oct of cutoff) → `d = 12/48 = 0.25`.
///
/// The factory patch does **not** seed this depth — key-track defaults **off**
/// to match VXN1 (whose `filter_key_track` default is `0.0`); see
/// [`default_patch`]. This is the UI/automation calibration point for a full
/// octave of tracking (and the constant the 0202 parity work hinges on: change
/// the Cutoff gain or Key normalisation and this value re-derives).
pub const KEY_CUTOFF_UNITY_DEPTH: f32 = 0.25;

/// Matrix depth reproducing VXN1's default LFO1→pitch **vibrato** of 0.05 st
/// (`pitch_lfo_depth` default). The matrix depth is the semitone value divided
/// by the Pitch dest's native gain ([`crate::eval::DEST_GAIN`]`[Pitch]` = 12 st),
/// so `source·depth·gain` = 0.05 st at full LFO swing. Cross-checked against the
/// gain in `eval::tests::default_vibrato_is_the_vxn1_005_st`.
pub const DEFAULT_VIBRATO_DEPTH: f32 = 0.05 / 12.0;

/// The factory default patch: seeds the routes that reproduce **VXN1's default
/// sound**, leaving every other slot inert. The rest of the modulation surface
/// is the player's to fill.
///
/// - **Slot 0 — Env2 → Amp @ 1.0.** Reproduces VXN1's hardwired VCA = Env2: the
///   amp envelope drives the VCA at full depth (VXN1 ADR 0004's Amp column).
///   Essential — without it the VCA never opens.
/// - **Slot 1 — Key → Cutoff @ 0.0.** The filter key-track route is *pre-wired*
///   for convenience (so the player only dials the depth), but its amount
///   defaults to **zero** — VXN1's `filter_key_track` default is `0.0`, so the
///   factory sound has no key-track. Raising the depth toward
///   [`KEY_CUTOFF_UNITY_DEPTH`] restores 1 oct/oct.
/// - **Slot 2 — LFO1 → Pitch @ [`DEFAULT_VIBRATO_DEPTH`].** Reproduces VXN1's
///   gentle default vibrato (0.05 st), so VXN1b's factory patch matches VXN1's
///   real default — the render-parity target (0202).
pub fn default_patch() -> MatrixTable {
    let mut table = MatrixTable::default();
    table.slots[0] = MatrixSlot {
        source: SourceId::Env2,
        dest: DestId::Amp,
        depth: 1.0,
        curve: Curve::Lin,
        scale_src: SourceId::None,
    };
    table.slots[1] = MatrixSlot {
        source: SourceId::Key,
        dest: DestId::Cutoff,
        depth: 0.0,
        curve: Curve::Lin,
        scale_src: SourceId::None,
    };
    table.slots[2] = MatrixSlot {
        source: SourceId::Lfo1,
        dest: DestId::Pitch,
        depth: DEFAULT_VIBRATO_DEPTH,
        curve: Curve::Lin,
        scale_src: SourceId::None,
    };
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_count_tracks_param_table() {
        assert_eq!(N_SLOTS, MATRIX_SLOTS);
    }

    #[test]
    fn source_u8_roundtrips_and_degrades() {
        for v in 0..=(N_SOURCES as u8) {
            let s = SourceId::from_u8(v);
            assert_eq!(s as u8, v);
        }
        // Out-of-range decodes to the inert sentinel.
        assert_eq!(SourceId::from_u8(200), SourceId::None);
    }

    #[test]
    fn dest_u8_roundtrips_and_degrades() {
        for v in 0..=(N_DESTS as u8) {
            assert_eq!(DestId::from_u8(v) as u8, v);
        }
        assert_eq!(DestId::from_u8(200), DestId::None);
    }

    #[test]
    fn curve_u8_roundtrips_and_degrades() {
        for v in 0..(N_CURVES as u8) {
            assert_eq!(Curve::from_u8(v) as u8, v);
        }
        assert_eq!(Curve::from_u8(200), Curve::Lin);
    }

    #[test]
    fn idx_maps_reals_and_skips_sentinel() {
        assert_eq!(SourceId::None.idx(), None);
        assert_eq!(SourceId::Env1.idx(), Some(0));
        assert_eq!(SourceId::NoteRandom.idx(), Some(N_SOURCES - 1));
        assert_eq!(DestId::None.idx(), None);
        assert_eq!(DestId::Pitch.idx(), Some(0));
        assert_eq!(DestId::CrossModAmount.idx(), Some(N_DESTS - 1));
    }

    #[test]
    fn name_and_label_tables_are_sized() {
        assert_eq!(SOURCE_NAMES.len(), N_SOURCES + 1);
        assert_eq!(SOURCE_LABELS.len(), N_SOURCES + 1);
        assert_eq!(DEST_NAMES.len(), N_DESTS + 1);
        assert_eq!(DEST_LABELS.len(), N_DESTS + 1);
        assert_eq!(CURVE_NAMES.len(), N_CURVES);
        assert_eq!(CURVE_LABELS.len(), N_CURVES);
    }

    #[test]
    fn polarity_table_is_locked() {
        // Bipolar: genuine ± swingers.
        for s in [SourceId::Lfo1, SourceId::Lfo2, SourceId::PitchWheel] {
            assert!(s.is_bipolar(), "{s:?} should be bipolar");
        }
        // Unipolar: everything else, incl. the [0,1] envelopes (see doc).
        for s in [
            SourceId::Env1,
            SourceId::Env2,
            SourceId::Velocity,
            SourceId::Key,
            SourceId::ModWheel,
            SourceId::Aftertouch,
            SourceId::NoteRandom,
        ] {
            assert!(!s.is_bipolar(), "{s:?} should be unipolar");
        }
    }

    #[test]
    fn default_slot_is_inert() {
        let s = MatrixSlot::default();
        assert!(!s.is_active());
        assert_eq!(s.source, SourceId::None);
        assert_eq!(s.dest, DestId::None);
        // A None-endpoint slot is inert even if a stale depth rides along.
        let with_depth = MatrixSlot { depth: 0.9, ..MatrixSlot::default() };
        assert!(!with_depth.is_active());
    }

    #[test]
    fn default_patch_seeds_amp_and_prewires_keytrack_off() {
        let t = default_patch();
        // Slot 0: Env2 → Amp @ 1.0 — essential, drives the VCA.
        assert_eq!(
            t.slots[0],
            MatrixSlot {
                source: SourceId::Env2,
                dest: DestId::Amp,
                depth: 1.0,
                curve: Curve::Lin,
                scale_src: SourceId::None,
            }
        );
        // Slot 1: Key → Cutoff pre-wired but at depth 0.0 (key-track off, VXN1
        // parity — `filter_key_track` defaults to 0.0).
        assert_eq!(
            t.slots[1],
            MatrixSlot {
                source: SourceId::Key,
                dest: DestId::Cutoff,
                depth: 0.0,
                curve: Curve::Lin,
                scale_src: SourceId::None,
            }
        );
        // The key-track route is wired (both endpoints real) so the UI shows it,
        // but its zero depth means the evaluator (0202) skips it — no key-track
        // in the factory sound.
        assert!(t.slots[1].is_active());
        assert_eq!(t.slots[1].depth, 0.0);
        // The calibration constant is the full-octave target, not the default.
        assert_eq!(KEY_CUTOFF_UNITY_DEPTH, 0.25);
        // Slot 2: LFO1 → Pitch gentle vibrato (matches VXN1's real default).
        assert_eq!(
            t.slots[2],
            MatrixSlot {
                source: SourceId::Lfo1,
                dest: DestId::Pitch,
                depth: DEFAULT_VIBRATO_DEPTH,
                curve: Curve::Lin,
                scale_src: SourceId::None,
            }
        );
        // Every remaining slot is inert.
        for s in &t.slots[3..] {
            assert!(!s.is_active(), "slot past the seeds must be inert");
        }
    }
}
