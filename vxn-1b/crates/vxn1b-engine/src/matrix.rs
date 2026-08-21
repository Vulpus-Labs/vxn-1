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
    /// The voice's own place in the stereo image (0260): the lane's fixed
    /// position scaled by the `Spread` param, so a route into [`DestId::Pan`]
    /// at depth 1 reproduces VXN1's hard-wired unison spread exactly. Keeping
    /// the param's scaling *inside* the source is what lets Spread stay a
    /// front-panel knob instead of becoming "slot 3's depth".
    Spread = 11,
}

/// Count of non-sentinel sources (`None` excluded).
pub const N_SOURCES: usize = 11;

impl SourceId {
    /// Index into a per-voice source lookup, or `None` for the sentinel.
    #[inline]
    pub const fn idx(self) -> Option<usize> {
        match self {
            SourceId::None => None,
            _ => Some(self as usize - 1),
        }
    }

    /// Index into a per-voice source table, with the sentinel folded to 0.
    ///
    /// The companion to [`SourceId::idx`]: reach for `idx` where the sentinel
    /// is a real case to branch on (an empty slot), and for this where the
    /// caller already knows it holds a real source and only wants to index.
    /// Folding rather than panicking keeps the method `const`, so index
    /// expressions stay compile-time constants.
    #[inline]
    pub const fn index(self) -> usize {
        match self.idx() {
            Some(i) => i,
            None => 0,
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
            11 => SourceId::Spread,
            _ => SourceId::None,
        }
    }

    /// Whether this source emits a **bipolar** `[-1, 1]` shape (vs a unipolar
    /// `[0, 1]` one). Consumed by the evaluator's `scale_norm` (0202) to fold a
    /// bipolar scale source into the `[0, 1]` VCA range. **Exhaustive match** —
    /// a new source forces a polarity decision at compile time (the
    /// `is_bipolar` discipline of VXN2 ADR 0009).
    ///
    /// - **Bipolar:** `Lfo1`, `Lfo2`, `PitchWheel`, `Spread` — genuinely swing
    ///   ±. (`Spread` is a *position*: lanes sit either side of centre.)
    /// - **Unipolar:** `Env1`, `Env2`, `Velocity`, `Key`, `ModWheel`,
    ///   `Aftertouch`, `NoteRandom`. The envelopes are `[0, 1]` ADSR shapes:
    ///   treating them as bipolar would map `[0, 1]` through `(x+1)/2` → `[0.5,
    ///   1]` as a scale VCA and never gate to zero (the same trap VXN2 flags for
    ///   `VoiceRand`), so they stay unipolar passthrough. `Key` is unipolar for
    ///   *scale* purposes; as a primary source its value is signed octaves rel
    ///   C4 (see [`KEY_CUTOFF_UNITY_DEPTH`]).
    #[inline]
    pub const fn is_bipolar(self) -> bool {
        match self {
            SourceId::Lfo1 | SourceId::Lfo2 | SourceId::PitchWheel | SourceId::Spread => true,
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
    "aftertouch", "note-random", "spread",
];

/// Source display label. Same indexing as [`SOURCE_NAMES`].
pub const SOURCE_LABELS: [&str; N_SOURCES + 1] = [
    "—", "Env 1", "Env 2", "LFO 1", "LFO 2", "Velocity", "Key", "Mod Wheel", "Pitch Wheel",
    "Aftertouch", "Note Rnd", "Spread",
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
    /// Voice position in the stereo image, `[-1, 1]` (0260). Replaces VXN1's
    /// hard-wired `pan_position(lane) × spread`: the default patch routes
    /// [`SourceId::Spread`] here at depth 1, and anything else routed on top
    /// (LFO auto-pan, an envelope throwing a transient left) sums with it.
    Pan = 9,
    /// Osc 1's pulse width alone (0261). Sums with [`DestId::Pwm`], which stays
    /// as the both-oscillators route: osc 1's offset is `Pwm + Osc1Pwm`.
    /// Two detuned pulse oscs get their thickness from the widths sweeping
    /// *independently*, which a single shared dest cannot express.
    Osc1Pwm = 10,
    /// Osc 2's pulse width alone (0261). Mirror of [`DestId::Osc1Pwm`].
    Osc2Pwm = 11,
    /// Envelope 1's **A/D/R times**, as a multiplier cooked at note-on (0268):
    /// `2^x` over the dest total clamped to `[-1, 1]`, so the reachable range
    /// is 0.5× (half as long) .. 2.0× (twice as long) with 0 exactly unity.
    /// Sustain is a *level*, not a time, and is deliberately untouched.
    ///
    /// Unlike every other dest this one is **not continuous**: the multiplier
    /// is latched when the voice triggers and held for the life of the note
    /// (see [`crate::eval::env_time_scale`]).
    Env1Scale = 12,
    /// Envelope 2's A/D/R times (0268). Mirror of [`DestId::Env1Scale`].
    Env2Scale = 13,
    /// Per-voice LFO 1's **rate**, as a multiplier on the resolved Hz (0269):
    /// `2^x` over the dest total clamped to `[-2, 2]`, so a full-depth route
    /// spans 0.25× .. 4× the panel rate with 0 exactly unity.
    ///
    /// Multiplicative on the *resolved* rate, so it composes with tempo sync
    /// (0267): a synced LFO stays on the grid under any power-of-two amount,
    /// and lands between subdivisions otherwise — the same freedom the Rate
    /// fader has when sync is off, now per voice.
    ///
    /// Unlike every other dest this one reads the **previous** control block's
    /// total: LFO 1 is itself a source, and the lanes tick before the matrix is
    /// evaluated, so a same-block read would be circular. The lag is one
    /// control block (32 samples, ~0.7 ms at 48 kHz).
    Lfo1Rate = 14,
    /// Envelope 1's **sustain level**, as an offset cooked at note-on (0270):
    /// the dest total is *added* to the patch sustain and clamped to `[0, 1]`,
    /// so depth 1 spans the whole range.
    ///
    /// Additive where the time dests ([`DestId::Env1Scale`]) are multiplicative,
    /// because sustain is an absolute level rather than a duration: a
    /// multiplier can never lift a sustain of 0, and never reach the ceiling
    /// from a low one — which is exactly what a velocity or wheel route wants
    /// to do.
    ///
    /// Latched at note-on like the time scales: the sustain level is the
    /// envelope's *held* value, so tracking it continuously would step a
    /// ringing note (and, through the decay rate it also sets, bend a decay
    /// already in flight).
    Env1Sustain = 15,
    /// Envelope 2's sustain level (0270). Mirror of [`DestId::Env1Sustain`].
    Env2Sustain = 16,
}

/// Count of non-sentinel destinations.
pub const N_DESTS: usize = 16;

impl DestId {
    /// Index into a per-voice dest accumulator, or `None` for the sentinel.
    #[inline]
    pub const fn idx(self) -> Option<usize> {
        match self {
            DestId::None => None,
            _ => Some(self as usize - 1),
        }
    }

    /// Index into a per-voice dest accumulator, with the sentinel folded to 0.
    /// The companion to [`DestId::idx`] — see [`SourceId::index`] for when to
    /// reach for which.
    #[inline]
    pub const fn index(self) -> usize {
        match self.idx() {
            Some(i) => i,
            None => 0,
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
            9 => DestId::Pan,
            10 => DestId::Osc1Pwm,
            11 => DestId::Osc2Pwm,
            12 => DestId::Env1Scale,
            13 => DestId::Env2Scale,
            14 => DestId::Lfo1Rate,
            15 => DestId::Env1Sustain,
            16 => DestId::Env2Sustain,
            _ => DestId::None,
        }
    }

    /// Cubic depth taper for the semitone `Pitch` dest (VXN2's `cook_depth`
    /// idiom). With a linear depth the whole vibrato range lives in the bottom
    /// sliver of fader travel — VXN1's default 0.05 st is 0.4% of the ±12 st
    /// span, so a single pixel of movement is a semitone-scale jump and precise
    /// vibrato is undialable. `d³` keeps the sign and the full ±12 st reach
    /// while widening the musical low end: 25% travel ≈ ±0.19 st, 50% ≈ ±1.5 st,
    /// 100% ≈ ±12 st.
    ///
    /// Applied at *consumption* ([`crate::eval::eval_dests`]), never to the
    /// stored slot depth — the CLAP param, preset file and state blob all stay
    /// linear, so automation and round-trips are unaffected.
    ///
    /// Non-pitch dests pass through untouched. `Cutoff` / `HpfCutoff` are
    /// deliberately excluded: their gain is already log/semitone-shaped, so a
    /// depth taper would double-bend the response (same rule as VXN2). The
    /// ±48 st `XModSweep` is left linear for now — it is a sweep amount, not a
    /// tuning offset.
    #[inline]
    pub fn cook_depth(self, depth: f32) -> f32 {
        match self {
            DestId::Pitch => depth * depth * depth,
            _ => depth,
        }
    }
}

/// Destination machine id (kebab-case wire name). Index = `DestId as u8`.
pub const DEST_NAMES: [&str; N_DESTS + 1] = [
    "none", "pitch", "xmod-sweep", "pwm", "cutoff", "resonance", "hpf-cutoff", "amp",
    "cross-mod-amount", "pan", "osc1-pwm", "osc2-pwm", "env1-scale", "env2-scale",
    "lfo1-rate", "env1-sustain", "env2-sustain",
];

/// Destination display label. Same indexing as [`DEST_NAMES`].
pub const DEST_LABELS: [&str; N_DESTS + 1] = [
    "—",
    "Pitch",
    // Spelled out to match the Cross Mod panel it sweeps; the wire name stays
    // `"xmod-sweep"`, so presets and state blobs written before the rename
    // decode unchanged.
    "Cross Mod Sweep",
    // 0261 relabelled this one; its wire name stays `"pwm"`, so presets and
    // state blobs written before the split decode unchanged.
    "PWM (Both)",
    "Cutoff",
    "Resonance",
    "HPF Cutoff",
    "Amp",
    "Cross Mod Amt",
    "Pan",
    "Osc 1 PWM",
    "Osc 2 PWM",
    "Env 1 Scale",
    "Env 2 Scale",
    "LFO 1 Rate",
    "Env 1 Sustain",
    "Env 2 Sustain",
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

impl MatrixTable {
    /// Install the default `Spread → Pan` route if this table has **no** route
    /// into [`DestId::Pan`] at all, using the first free slot. Returns whether
    /// one was installed.
    ///
    /// Why loading needs this (0260): before pan was a destination, spread was
    /// hard-wired DSP, so every patch written until now carries no pan route
    /// and would load dead-centre — a silent regression on every existing
    /// preset. Seeding on load fixes that without a format change, since the
    /// preset text is name-keyed and sparse rather than positional.
    ///
    /// A patch that *does* route `Pan` — even from some other source, even at
    /// depth 0 — is left alone: it has an opinion about pan, and overriding it
    /// would be worse than the problem being solved. A table with all 16 slots
    /// occupied is likewise left alone rather than evicting the player's work.
    pub fn ensure_pan_route(&mut self) -> bool {
        if self.slots.iter().any(|s| s.dest == DestId::Pan) {
            return false;
        }
        match self.slots.iter_mut().find(|s| !s.is_active()) {
            Some(slot) => {
                *slot = SPREAD_TO_PAN;
                true
            }
            None => false,
        }
    }
}

/// Both layers' topology as a **view payload** (0247): the engine→page echo that
/// keeps the mod-matrix combos honest when the patch changes underneath an open
/// editor (preset load, host state load, undo).
///
/// Topology is not a CLAP param, so the host replays nothing for it; the page is
/// seeded once at editor-open ([`crate::PluginState`] → the faceplate splice,
/// 0246) and thereafter learns of changes only through this. Depths are
/// deliberately absent — they *are* params and ride `ParamChanged`, and echoing
/// them here would give the page two sources of truth for one value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatrixSnapshot {
    pub layers: [MatrixTable; 2],
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
/// **This is the *extra*, freely-routed tracking, not the VXN1 control** (0245).
/// VXN1's key-track is [`ParamId::FilterKeyTrack`](crate::params::ParamId), a
/// dedicated `0..1` param pivoting at **C0** — matching pivot as well as slope,
/// which is what makes "cutoff at minimum (16.3516 Hz = C0) + track at 1.0 ⇒
/// cutoff *is* the played note" hold. A matrix route pivots at C4 like every
/// other Key route, and stacks additively on top of the param: use it for the
/// things the param cannot do (envelope-scaled, negative, or curved tracking,
/// or more than 1 oct/oct).
pub const KEY_CUTOFF_UNITY_DEPTH: f32 = 0.25;

/// Matrix depth reproducing VXN1's default LFO1→pitch **vibrato** of 0.05 st
/// (`pitch_lfo_depth` default). Pitch takes the cubic depth taper
/// ([`DestId::cook_depth`]) before the dest's native gain
/// ([`crate::eval::DEST_GAIN`]`[Pitch]` = 12 st), so the depth is the *cube
/// root* of the semitone value over that gain: `∛(0.05/12) ≈ 0.1609`, giving
/// `source·depth³·gain` = 0.05 st at full LFO swing. Cross-checked against both
/// in `eval::tests::default_vibrato_is_the_vxn1_005_st`.
pub const DEFAULT_VIBRATO_DEPTH: f32 = 0.160_918;

/// The factory default patch: seeds the routes that reproduce **VXN1's default
/// sound**, leaving every other slot inert. The rest of the modulation surface
/// is the player's to fill.
///
/// - **Slot 0 — Env2 → Amp @ 1.0.** Reproduces VXN1's hardwired VCA = Env2: the
///   amp envelope drives the VCA at full depth (VXN1 ADR 0004's Amp column).
///   Essential — without it the VCA never opens.
/// - **Slot 1 — LFO1 → Pitch @ [`DEFAULT_VIBRATO_DEPTH`].** Reproduces VXN1's
///   gentle default vibrato (0.05 st), so VXN1b's factory patch matches VXN1's
///   real default — the render-parity target (0202).
///
/// - **Slot 2 — Spread → Pan @ 1.0.** Reproduces VXN1's hard-wired unison
///   spread, which used to be a line of DSP (`pan_position(lane) × spread`) and
///   is now a route like any other (0260). Depth 1.0 is the identity, so the
///   `Spread` knob keeps its full range and meaning; delete the route and the
///   knob goes inert, which is the honest consequence of routing being visible.
///
/// Filter key-track is **not** here: it used to occupy a pre-wired Key→Cutoff
/// slot standing in for VXN1's missing param, and 0245 gave it back its own
/// param ([`ParamId::FilterKeyTrack`](crate::params::ParamId), default `0.0`
/// like VXN1's). The slot is the player's again.
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
        source: SourceId::Lfo1,
        dest: DestId::Pitch,
        depth: DEFAULT_VIBRATO_DEPTH,
        curve: Curve::Lin,
        scale_src: SourceId::None,
    };
    table.slots[2] = SPREAD_TO_PAN;
    table
}

/// The `Spread → Pan` route the default patch seeds and
/// [`MatrixTable::ensure_pan_route`] restores (0260). Depth 1.0 is the
/// identity: the `Spread` param does the scaling inside the source.
pub const SPREAD_TO_PAN: MatrixSlot = MatrixSlot {
    source: SourceId::Spread,
    dest: DestId::Pan,
    depth: 1.0,
    curve: Curve::Lin,
    scale_src: SourceId::None,
};

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
    fn cook_depth_tapers_pitch_only() {
        // Cubic on Pitch: sign and endpoints kept, low end widened.
        assert_eq!(DestId::Pitch.cook_depth(0.0), 0.0);
        assert_eq!(DestId::Pitch.cook_depth(1.0), 1.0);
        assert_eq!(DestId::Pitch.cook_depth(-1.0), -1.0);
        assert!((DestId::Pitch.cook_depth(0.5) - 0.125).abs() < 1e-6);
        assert!((DestId::Pitch.cook_depth(-0.25) + 0.015_625).abs() < 1e-6);
        // Every other dest passes through untouched.
        for d in [
            DestId::None,
            DestId::XModSweep,
            DestId::Pwm,
            DestId::Cutoff,
            DestId::Resonance,
            DestId::HpfCutoff,
            DestId::Amp,
            DestId::CrossModAmount,
            DestId::Pan,
            DestId::Osc1Pwm,
            DestId::Osc2Pwm,
        ] {
            assert_eq!(d.cook_depth(0.5), 0.5, "{d:?} should stay linear");
            assert_eq!(d.cook_depth(-0.3), -0.3, "{d:?} should stay linear");
        }
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
        assert_eq!(SourceId::Spread.idx(), Some(N_SOURCES - 1));
        assert_eq!(DestId::None.idx(), None);
        assert_eq!(DestId::Pitch.idx(), Some(0));
        assert_eq!(DestId::Pan.idx(), Some(8));
        assert_eq!(DestId::Osc2Pwm.idx(), Some(10));
        assert_eq!(DestId::Env2Scale.idx(), Some(12));
        assert_eq!(DestId::Lfo1Rate.idx(), Some(13));
        assert_eq!(DestId::Env2Sustain.idx(), Some(N_DESTS - 1));
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

    // ── ensure_pan_route (0260) ─────────────────────────────────────────────

    #[test]
    fn ensure_pan_route_seeds_a_patch_with_no_pan_opinion() {
        let mut t = MatrixTable::default();
        t.slots[0] = MatrixSlot {
            source: SourceId::Env2,
            dest: DestId::Amp,
            depth: 1.0,
            curve: Curve::Lin,
            scale_src: SourceId::None,
        };
        assert!(t.ensure_pan_route(), "a pan-less patch must be seeded");
        assert_eq!(t.slots[1], SPREAD_TO_PAN, "seeded into the first free slot");
        // Idempotent: a second pass finds the route it just installed.
        assert!(!t.ensure_pan_route());
    }

    #[test]
    fn ensure_pan_route_leaves_an_existing_pan_opinion_alone() {
        // Any route into Pan counts — including one from another source, and
        // including one parked at depth 0. The patch has an opinion; honour it.
        for existing in [
            MatrixSlot { source: SourceId::Lfo1, dest: DestId::Pan, depth: 0.5,
                         curve: Curve::Lin, scale_src: SourceId::None },
            MatrixSlot { source: SourceId::Spread, dest: DestId::Pan, depth: 0.0,
                         curve: Curve::Lin, scale_src: SourceId::None },
        ] {
            let mut t = MatrixTable::default();
            t.slots[4] = existing;
            assert!(!t.ensure_pan_route(), "must not seed over {existing:?}");
            assert_eq!(t.slots[4], existing);
            assert!(
                t.slots.iter().filter(|s| s.dest == DestId::Pan).count() == 1,
                "exactly one pan route"
            );
        }
    }

    #[test]
    fn ensure_pan_route_gives_up_rather_than_evicting() {
        // Sixteen live routes, none of them pan: the player's work wins over
        // the convenience seed.
        let mut t = MatrixTable::default();
        for slot in t.slots.iter_mut() {
            *slot = MatrixSlot {
                source: SourceId::Lfo1,
                dest: DestId::Cutoff,
                depth: 0.5,
                curve: Curve::Lin,
                scale_src: SourceId::None,
            };
        }
        assert!(!t.ensure_pan_route(), "a full table must not be evicted");
        assert!(t.slots.iter().all(|s| s.dest == DestId::Cutoff));
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
    fn default_patch_seeds_amp_vibrato_and_spread_pan() {
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
        // Slot 1: LFO1 → Pitch gentle vibrato (matches VXN1's real default).
        assert_eq!(
            t.slots[1],
            MatrixSlot {
                source: SourceId::Lfo1,
                dest: DestId::Pitch,
                depth: DEFAULT_VIBRATO_DEPTH,
                curve: Curve::Lin,
                scale_src: SourceId::None,
            }
        );
        // Slot 2: Spread → Pan at unity — VXN1's unison spread, as a route
        // rather than hard-wired DSP (0260).
        assert_eq!(t.slots[2], SPREAD_TO_PAN);
        assert_eq!(t.slots[2].depth, 1.0);
        // Key-track is a param (0245), not a pre-wired slot: nothing in the
        // factory patch touches Cutoff, and every remaining slot is the
        // player's.
        for s in &t.slots[3..] {
            assert!(!s.is_active(), "slot past the seeds must be inert");
        }
        assert!(
            !t.slots.iter().any(|s| s.dest == DestId::Cutoff),
            "the factory patch must not pre-wire a Cutoff route"
        );
        // The calibration constant survives as the *extra* tracking's unity mark.
        assert_eq!(KEY_CUTOFF_UNITY_DEPTH, 0.25);
    }
}
