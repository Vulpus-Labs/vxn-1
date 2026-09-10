//! The patch descriptor table: every field of a [`Patch`](crate::patch::Patch)
//! given a stable name, a range, a taper and a default (ticket 0381).
//!
//! This is the prerequisite for both halves of E052. The preset codec keys its
//! TOML by these names, so a file survives the field order changing and adopts
//! improved defaults for anything it does not override; the faceplate binds its
//! controls by the same names, so the page and the engine cannot drift apart
//! without a test noticing.
//!
//! ## Two id spaces, deliberately
//!
//! [`ParamId`] indexes **this** table — 252 entries, everything a patch holds
//! plus the eleven host params. `clap_id` indexes the host's parameter list —
//! eleven entries, and no more, for the reasons set out in
//! `vxn4_clap::params`: exposing 80 modulation destinations would bake a
//! patch's routing topology into every saved project.
//!
//! vxn-1b does not need this distinction because its descriptor table *is* its
//! CLAP table. vxn-4's is not, so the two are separate spaces joined only by
//! [`clap_id_for`] and [`param_for_clap`]. There are no casts between them
//! anywhere; a `usize` from one space is meaningless in the other.
//!
//! ## Two regions
//!
//! The table splits at [`PATCH_PARAMS`]:
//!
//! - **Patch region** (`0 .. PATCH_PARAMS`) — what a patch *is*, and therefore
//!   exactly what the preset codec writes. [`is_patch_field`] is the predicate.
//! - **Host region** (`PATCH_PARAMS ..`) — which patch is loaded, oversampling
//!   quality, output trim, and the eight macro knobs. These are **performance
//!   and project state, not patch state**: a macro's position belongs to the
//!   song, not to the sound. They are in the table so the faceplate can bind
//!   them by name like everything else, and out of the preset body so that
//!   loading a preset does not stamp on the player's knobs.
//!
//! `master-gain` and `gain` are different things and both exist: `gain` is the
//! patch's own measured loudness trim (`Patch::gain`, set so a six-note chord
//! lands near −6 dBFS), and `master-gain` is the player's output trim. The
//! first is patch state; the second is not.
//!
//! ## Naming
//!
//! Kebab, zero-based, index embedded — the convention
//! [`crate::matrix`]'s `DEST_NAMES` already uses, so the two read alike:
//!
//! ```text
//! op-3-damp-hz     operator 3's damping corner
//! op-3-eg-t2       operator 3's EG, decay-1 time
//! pm-3-5           the authored PM depth into op 3 from op 5
//! out-3            operator 3's sum-bus send
//! matrix-07-depth  matrix slot 7's raw depth
//! ```
//!
//! Names are `&'static str` built with `concat!` rather than formatted at
//! runtime, which is what lets [`ParamDesc`] stay a plain static.
//!
//! ## Params and the destinations that modulate them
//!
//! Several patch fields are also modulation destinations: the total the engine
//! renders is the authored value from this table plus whatever the matrix
//! contributes. [`dest_for`] is that pairing, and `every_dest_has_a_param`
//! asserts it is total — add a `DestId` family without a matching param and the
//! tests fail rather than the preset silently losing a field.
//!
//! The pairing is *not* by name, because for two families the units differ: a
//! `damp-N` destination is in **octaves** and shifts the corner
//! multiplicatively, while `op-N-damp-hz` is the corner itself in Hz; a
//! `ratio-N` destination is in **semitones** and multiplies, while
//! `op-N-ratio` is the bare frequency ratio. Sharing a name would have implied
//! they share a unit.

use vxn4_dsp::ops::{DEFAULT_DAMP_HZ, NOPS};
use vxn4_dsp::wavetable::Waveform;

pub use vxn_core_app::ParamId;
use vxn_core_app::{ParamDesc, ParamKind, Taper};

use crate::engine::{MAX_DAMP_HZ, MAX_MASTER_GAIN, MIN_DAMP_HZ};
use crate::matrix::{
    N_MATRIX_SLOTS, damp_dest_index, out_dest_index, pan_dest_index, pm_dest_index,
    ratio_dest_index, spread_dest_index,
};
use crate::patch::N_PATCHES;

/// Per-operator scalar fields, in table order.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum OpField {
    Wave = 0,
    Ratio = 1,
    Level = 2,
    Pan = 3,
    DampHz = 4,
    Phase = 5,
    PhaseSpread = 6,
}

/// Per-operator envelope fields: four segment times, then four levels — the
/// layout of [`EgParams`]'s `t` and `l` arrays.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum EgField {
    T1 = 0,
    T2 = 1,
    T3 = 2,
    T4 = 3,
    L1 = 4,
    L2 = 5,
    L3 = 6,
    L4 = 7,
}

/// The eleven host params, in `clap_id` order. Their ids in *this* table are
/// [`PATCH_PARAMS`] plus these discriminants, which is what makes
/// [`clap_id_for`] a subtraction rather than a match.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum HostField {
    Patch = 0,
    Quality = 1,
    MasterGain = 2,
    /// `Macro1` through `Macro8` follow contiguously.
    Macro1 = 3,
}

/// Scalar fields per operator.
pub const N_OP_SCALARS: usize = 7;
/// Envelope fields per operator.
pub const N_OP_EG: usize = 8;
/// Descriptors per operator.
pub const N_PER_OP: usize = N_OP_SCALARS + N_OP_EG;

/// Number of macro knobs. Mirrors [`crate::matrix::N_MACROS`]; asserted equal.
pub const N_MACROS: usize = 8;

// Block offsets. The table is laid out as contiguous blocks so that decoding is
// arithmetic rather than a lookup, and so that adding a block cannot renumber
// an existing one from the middle.
const OPS_BASE: usize = 0;
const PM_BASE: usize = OPS_BASE + NOPS * N_PER_OP;
const OUT_BASE: usize = PM_BASE + NOPS * NOPS;
const MATRIX_BASE: usize = OUT_BASE + NOPS;
const PATCH_GAIN: usize = MATRIX_BASE + N_MATRIX_SLOTS;

/// End of the patch region: ids below this are what a preset serialises.
pub const PATCH_PARAMS: usize = PATCH_GAIN + 1;

/// Host params: patch, quality, master gain, and the eight macros.
pub const N_HOST_PARAMS: usize = 3 + N_MACROS;

/// Every descriptor, patch region then host region.
pub const N_PARAMS: usize = PATCH_PARAMS + N_HOST_PARAMS;

/// The decoded meaning of a [`ParamId`].
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Param {
    /// A per-operator scalar.
    Op { op: usize, field: OpField },
    /// A per-operator envelope segment time or level.
    OpEg { op: usize, field: EgField },
    /// The authored PM depth into `dest` from `src`; `dest == src` is that
    /// operator's self-feedback.
    Pm { dest: usize, src: usize },
    /// An operator's send into the stereo sum bus.
    Out { op: usize },
    /// A matrix slot's raw, untapered depth.
    MatrixDepth { slot: usize },
    /// The patch's own measured loudness trim.
    PatchGain,
    /// A host param — not patch state.
    Host(HostField),
}

/// Decode an id, or `None` if it is out of range.
pub fn decode(id: ParamId) -> Option<Param> {
    let i = id.raw();
    if i >= N_PARAMS {
        return None;
    }
    Some(if i < PM_BASE {
        let op = i / N_PER_OP;
        let field = i % N_PER_OP;
        if field < N_OP_SCALARS {
            Param::Op {
                op,
                field: OP_FIELDS[field],
            }
        } else {
            Param::OpEg {
                op,
                field: EG_FIELDS[field - N_OP_SCALARS],
            }
        }
    } else if i < OUT_BASE {
        let k = i - PM_BASE;
        Param::Pm {
            dest: k / NOPS,
            src: k % NOPS,
        }
    } else if i < MATRIX_BASE {
        Param::Out { op: i - OUT_BASE }
    } else if i < PATCH_GAIN {
        Param::MatrixDepth {
            slot: i - MATRIX_BASE,
        }
    } else if i == PATCH_GAIN {
        Param::PatchGain
    } else {
        let h = i - PATCH_PARAMS;
        Param::Host(match h {
            0 => HostField::Patch,
            1 => HostField::Quality,
            2 => HostField::MasterGain,
            // Macro1..Macro8 are contiguous from discriminant 3; the decode
            // keeps them as `Macro1` plus an offset rather than eight variants,
            // and `macro_index` is how a caller recovers which knob.
            _ => HostField::Macro1,
        })
    })
}

/// Encode a [`Param`] back to its id. Total — every `Param` this crate can
/// construct has an id, so there is no failure case to handle at call sites.
///
/// # Panics
///
/// On an out-of-range operator, source or slot index. These are internal
/// indices, never user input: a caller that has an `op` at all got it from
/// `0..NOPS`, and a panic here is a bug in this crate rather than a condition
/// to recover from.
pub fn encode(p: Param) -> ParamId {
    ParamId::new(match p {
        Param::Op { op, field } => {
            assert!(op < NOPS, "operator index {op} out of range");
            OPS_BASE + op * N_PER_OP + field as usize
        }
        Param::OpEg { op, field } => {
            assert!(op < NOPS, "operator index {op} out of range");
            OPS_BASE + op * N_PER_OP + N_OP_SCALARS + field as usize
        }
        Param::Pm { dest, src } => {
            assert!(
                dest < NOPS && src < NOPS,
                "route {dest}<-{src} out of range"
            );
            PM_BASE + pm_dest_index(dest, src)
        }
        Param::Out { op } => {
            assert!(op < NOPS, "operator index {op} out of range");
            OUT_BASE + op
        }
        Param::MatrixDepth { slot } => {
            assert!(slot < N_MATRIX_SLOTS, "matrix slot {slot} out of range");
            MATRIX_BASE + slot
        }
        Param::PatchGain => PATCH_GAIN,
        Param::Host(h) => PATCH_PARAMS + h as usize,
    })
}

/// The id of macro knob `m` (`0..N_MACROS`).
pub fn macro_id(m: usize) -> ParamId {
    assert!(m < N_MACROS, "macro index {m} out of range");
    ParamId::new(PATCH_PARAMS + HostField::Macro1 as usize + m)
}

/// Which macro knob an id names, if it names one.
pub fn macro_index(id: ParamId) -> Option<usize> {
    let base = PATCH_PARAMS + HostField::Macro1 as usize;
    (id.raw() >= base && id.raw() < base + N_MACROS).then(|| id.raw() - base)
}

/// Whether an id is patch state — i.e. whether the preset codec writes it.
#[inline]
pub fn is_patch_field(id: ParamId) -> bool {
    id.raw() < PATCH_PARAMS
}

/// The host parameter id for a descriptor id, for the eleven that have one.
///
/// The patch region has no host params in it at all: a patch field is not
/// automatable by design, and `Patch::gain` is not `master-gain`.
pub fn clap_id_for(id: ParamId) -> Option<usize> {
    (id.raw() >= PATCH_PARAMS).then(|| id.raw() - PATCH_PARAMS)
}

/// Inverse of [`clap_id_for`].
pub fn param_for_clap(clap_id: usize) -> Option<ParamId> {
    (clap_id < N_HOST_PARAMS).then(|| ParamId::new(PATCH_PARAMS + clap_id))
}

/// The modulation destination that adds to this param, if any.
///
/// Note the units: `Damp` is octaves against a corner in Hz and `Ratio` is
/// semitones against a bare ratio, both multiplicative. The rest are additive
/// in the param's own unit. See [`crate::engine`] for where each total is
/// formed.
pub fn dest_for(id: ParamId) -> Option<usize> {
    Some(match decode(id)? {
        Param::Op {
            op,
            field: OpField::Ratio,
        } => ratio_dest_index(op),
        Param::Op {
            op,
            field: OpField::Pan,
        } => pan_dest_index(op),
        Param::Op {
            op,
            field: OpField::DampHz,
        } => damp_dest_index(op),
        Param::Op {
            op,
            field: OpField::PhaseSpread,
        } => spread_dest_index(op),
        Param::Pm { dest, src } => pm_dest_index(dest, src),
        Param::Out { op } => out_dest_index(op),
        _ => return None,
    })
}

const OP_FIELDS: [OpField; N_OP_SCALARS] = [
    OpField::Wave,
    OpField::Ratio,
    OpField::Level,
    OpField::Pan,
    OpField::DampHz,
    OpField::Phase,
    OpField::PhaseSpread,
];

const EG_FIELDS: [EgField; N_OP_EG] = [
    EgField::T1,
    EgField::T2,
    EgField::T3,
    EgField::T4,
    EgField::L1,
    EgField::L2,
    EgField::L3,
    EgField::L4,
];

// ── Ranges ──────────────────────────────────────────────────────────────────

/// Waveform variant labels, in [`Waveform::ALL`] order. Stored in a preset
/// rather than the discriminant, so inserting a waveform cannot re-point an
/// existing file at a different one.
pub const WAVE_VARIANTS: [&str; 4] = ["sine", "triangle", "saw", "square"];

/// Frequency-ratio bounds. Five octaves either side of unison, which covers
/// every factory patch (the widest is `epiano`'s strike modulator at 14×) with
/// room for the inharmonic ratios `bell` and `web` are built from.
const RATIO_MIN: f32 = 0.031_25;
const RATIO_MAX: f32 = 64.0;

/// Authored PM depth bounds. The factory patches span 0.03..1.2 and the
/// destination totals add on top without a clamp, so the range is set wide
/// enough that a preset can hold anything the matrix can drive it to.
///
/// Bipolar: a negative depth inverts the modulator, which cancels where the
/// positive would reinforce. Nothing in the factory bank uses it yet.
const PM_MAX: f32 = 4.0;

/// Longest EG segment. Twenty seconds is past any musical release and short
/// enough that the taper keeps millisecond resolution at the bottom.
const EG_T_MAX: f32 = 20.0;

const fn float(unit: &'static str, taper: Taper) -> ParamKind {
    ParamKind::Float { unit, taper }
}

// ── Per-operator descriptors ────────────────────────────────────────────────

// One block per operator. The digit is a literal so `concat!` can build a
// `&'static str` from it; the alternative is formatting names at runtime, which
// `ParamDesc` cannot hold.
macro_rules! op_block {
    ($d:literal) => {
        [
            ParamDesc {
                name: concat!("op-", $d, "-wave"),
                label: concat!("Op", $d, " Wave"),
                min: 0.0,
                max: (WAVE_VARIANTS.len() - 1) as f32,
                default: 0.0,
                kind: ParamKind::Enum {
                    variants: &WAVE_VARIANTS,
                },
            },
            ParamDesc {
                name: concat!("op-", $d, "-ratio"),
                label: concat!("Op", $d, " Ratio"),
                min: RATIO_MIN,
                max: RATIO_MAX,
                default: 1.0,
                kind: float("x", Taper::Exp { mid: 1.0 }),
            },
            ParamDesc {
                name: concat!("op-", $d, "-level"),
                label: concat!("Op", $d, " Level"),
                min: 0.0,
                max: 1.0,
                default: 1.0,
                kind: float("", Taper::Linear),
            },
            ParamDesc {
                name: concat!("op-", $d, "-pan"),
                label: concat!("Op", $d, " Pan"),
                min: -1.0,
                max: 1.0,
                default: 0.0,
                kind: float("", Taper::Linear),
            },
            ParamDesc {
                name: concat!("op-", $d, "-damp-hz"),
                label: concat!("Op", $d, " Damp"),
                min: MIN_DAMP_HZ,
                max: MAX_DAMP_HZ,
                default: DEFAULT_DAMP_HZ,
                // The range is the engine's own clamp, so a preset cannot hold
                // a corner the engine would silently move. That makes the top
                // of the fader inaudible on purpose — everything above ~20 kHz
                // reads as bypass — so the taper puts the audible decade across
                // the lower half.
                kind: float("Hz", Taper::Exp { mid: 2_000.0 }),
            },
            ParamDesc {
                name: concat!("op-", $d, "-phase"),
                label: concat!("Op", $d, " Phase"),
                min: 0.0,
                max: 1.0,
                default: 0.0,
                kind: float("turns", Taper::Linear),
            },
            ParamDesc {
                name: concat!("op-", $d, "-phase-spread"),
                label: concat!("Op", $d, " Phase Decorr"),
                min: 0.0,
                max: 1.0,
                // 1.0 is the historical unconditional scatter, and the default
                // for that reason: a patch that does not mention this field
                // must sound as it did before the field existed.
                default: 1.0,
                kind: float("", Taper::Linear),
            },
            eg_time!($d, "1", 0),
            eg_time!($d, "2", 1),
            eg_time!($d, "3", 2),
            eg_time!($d, "4", 3),
            eg_level!($d, "1", 0),
            eg_level!($d, "2", 1),
            eg_level!($d, "3", 2),
            eg_level!($d, "4", 3),
        ]
    };
}

macro_rules! eg_time {
    ($d:literal, $n:literal, $i:expr) => {
        ParamDesc {
            name: concat!("op-", $d, "-eg-t", $n),
            label: concat!("Op", $d, " EG T", $n),
            min: 0.0,
            max: EG_T_MAX,
            default: EG_DEFAULT_T[$i],
            kind: float("s", Taper::Exp { mid: 0.5 }),
        }
    };
}

macro_rules! eg_level {
    ($d:literal, $n:literal, $i:expr) => {
        ParamDesc {
            name: concat!("op-", $d, "-eg-l", $n),
            label: concat!("Op", $d, " EG L", $n),
            min: 0.0,
            max: 1.0,
            default: EG_DEFAULT_L[$i],
            kind: float("", Taper::Linear),
        }
    };
}

/// EG defaults, lifted from [`EgParams::default`] so the two cannot drift.
/// `EgParams: Default` is not `const`, so these are duplicated here and pinned
/// by `eg_defaults_match_egparams`.
const EG_DEFAULT_T: [f32; 4] = [0.005, 0.30, 1.0, 0.25];
const EG_DEFAULT_L: [f32; 4] = [1.0, 0.7, 0.5, 0.0];

const OP_DESCS: [[ParamDesc; N_PER_OP]; NOPS] = [
    op_block!("0"),
    op_block!("1"),
    op_block!("2"),
    op_block!("3"),
    op_block!("4"),
    op_block!("5"),
    op_block!("6"),
    op_block!("7"),
];

// ── PM route descriptors ────────────────────────────────────────────────────

macro_rules! pm_desc {
    ($d:literal, $s:literal) => {
        ParamDesc {
            name: concat!("pm-", $d, "-", $s),
            label: concat!("Op", $d, " <- Op", $s),
            min: -PM_MAX,
            max: PM_MAX,
            default: 0.0,
            // The musical range is the inner tenth of the span, which is what
            // BipolarExp is for: half travel either way reads ±0.5.
            kind: float("", Taper::BipolarExp { mid: 0.5 }),
        }
    };
    // Diagonal — an operator's own feedback. Labelled as `DEST_LABELS` labels
    // it, so the two surfaces name the same route the same way.
    ($d:literal) => {
        ParamDesc {
            name: concat!("pm-", $d, "-", $d),
            label: concat!("Op", $d, " self"),
            min: -PM_MAX,
            max: PM_MAX,
            default: 0.0,
            kind: float("", Taper::BipolarExp { mid: 0.5 }),
        }
    };
}

#[rustfmt::skip]
const PM_DESCS: [[ParamDesc; NOPS]; NOPS] = [
    [pm_desc!("0"), pm_desc!("0","1"), pm_desc!("0","2"), pm_desc!("0","3"), pm_desc!("0","4"), pm_desc!("0","5"), pm_desc!("0","6"), pm_desc!("0","7")],
    [pm_desc!("1","0"), pm_desc!("1"), pm_desc!("1","2"), pm_desc!("1","3"), pm_desc!("1","4"), pm_desc!("1","5"), pm_desc!("1","6"), pm_desc!("1","7")],
    [pm_desc!("2","0"), pm_desc!("2","1"), pm_desc!("2"), pm_desc!("2","3"), pm_desc!("2","4"), pm_desc!("2","5"), pm_desc!("2","6"), pm_desc!("2","7")],
    [pm_desc!("3","0"), pm_desc!("3","1"), pm_desc!("3","2"), pm_desc!("3"), pm_desc!("3","4"), pm_desc!("3","5"), pm_desc!("3","6"), pm_desc!("3","7")],
    [pm_desc!("4","0"), pm_desc!("4","1"), pm_desc!("4","2"), pm_desc!("4","3"), pm_desc!("4"), pm_desc!("4","5"), pm_desc!("4","6"), pm_desc!("4","7")],
    [pm_desc!("5","0"), pm_desc!("5","1"), pm_desc!("5","2"), pm_desc!("5","3"), pm_desc!("5","4"), pm_desc!("5"), pm_desc!("5","6"), pm_desc!("5","7")],
    [pm_desc!("6","0"), pm_desc!("6","1"), pm_desc!("6","2"), pm_desc!("6","3"), pm_desc!("6","4"), pm_desc!("6","5"), pm_desc!("6"), pm_desc!("6","7")],
    [pm_desc!("7","0"), pm_desc!("7","1"), pm_desc!("7","2"), pm_desc!("7","3"), pm_desc!("7","4"), pm_desc!("7","5"), pm_desc!("7","6"), pm_desc!("7")],
];

// ── Sum-bus sends ───────────────────────────────────────────────────────────

macro_rules! out_desc {
    ($d:literal) => {
        ParamDesc {
            name: concat!("out-", $d),
            label: concat!("Op", $d, " Out"),
            min: 0.0,
            max: 1.0,
            // Zero: an operator is silent until the patch sends it somewhere,
            // which is what makes a sparse preset's omissions safe.
            default: 0.0,
            kind: float("", Taper::Linear),
        }
    };
}

const OUT_DESCS: [ParamDesc; NOPS] = [
    out_desc!("0"),
    out_desc!("1"),
    out_desc!("2"),
    out_desc!("3"),
    out_desc!("4"),
    out_desc!("5"),
    out_desc!("6"),
    out_desc!("7"),
];

// ── Matrix slot depths ──────────────────────────────────────────────────────

// Depth is a param; the slot's source, destination and curves are topology and
// serialise separately (0383). Raw and untapered — the slot's own `cook_depth`
// applies the destination's taper, and applying one here would cube an
// already-cubed depth.
macro_rules! slot_desc {
    ($n:literal) => {
        ParamDesc {
            name: concat!("matrix-", $n, "-depth"),
            label: concat!("Slot ", $n, " Depth"),
            min: -1.0,
            max: 1.0,
            default: 0.0,
            kind: float("", Taper::Linear),
        }
    };
}

#[rustfmt::skip]
const MATRIX_DESCS: [ParamDesc; N_MATRIX_SLOTS] = [
    slot_desc!("00"), slot_desc!("01"), slot_desc!("02"), slot_desc!("03"),
    slot_desc!("04"), slot_desc!("05"), slot_desc!("06"), slot_desc!("07"),
    slot_desc!("08"), slot_desc!("09"), slot_desc!("10"), slot_desc!("11"),
    slot_desc!("12"), slot_desc!("13"), slot_desc!("14"), slot_desc!("15"),
    slot_desc!("16"), slot_desc!("17"), slot_desc!("18"), slot_desc!("19"),
    slot_desc!("20"), slot_desc!("21"), slot_desc!("22"), slot_desc!("23"),
    slot_desc!("24"), slot_desc!("25"), slot_desc!("26"), slot_desc!("27"),
    slot_desc!("28"), slot_desc!("29"), slot_desc!("30"), slot_desc!("31"),
    slot_desc!("32"), slot_desc!("33"), slot_desc!("34"), slot_desc!("35"),
    slot_desc!("36"), slot_desc!("37"), slot_desc!("38"), slot_desc!("39"),
    slot_desc!("40"), slot_desc!("41"), slot_desc!("42"), slot_desc!("43"),
    slot_desc!("44"), slot_desc!("45"), slot_desc!("46"), slot_desc!("47"),
];

// ── Patch trim and the host region ──────────────────────────────────────────

const PATCH_GAIN_DESC: ParamDesc = ParamDesc {
    name: "gain",
    label: "Patch Trim",
    min: 0.0,
    max: 2.0,
    // Unity, not any factory patch's value: the factory trims are measured
    // per patch and serialise as deviations from neutral.
    default: 1.0,
    kind: float("", Taper::Linear),
};

pub const QUALITY_VARIANTS: [&str; 2] = ["8x", "16x"];

macro_rules! macro_desc {
    ($n:literal) => {
        ParamDesc {
            name: concat!("macro-", $n),
            label: concat!("Macro ", $n),
            min: 0.0,
            max: 1.0,
            // Zero, not centre: every macro at zero is the patch exactly as
            // its author left it, which is the invariant the matrix rests on.
            default: 0.0,
            kind: float("", Taper::Linear),
        }
    };
}

#[rustfmt::skip]
const HOST_DESCS: [ParamDesc; N_HOST_PARAMS] = [
    ParamDesc {
        name: "patch",
        label: "Patch",
        min: 0.0,
        max: (N_PATCHES - 1) as f32,
        default: 0.0,
        kind: ParamKind::Int { unit: "" },
    },
    ParamDesc {
        name: "quality",
        label: "Quality",
        min: 0.0,
        max: (QUALITY_VARIANTS.len() - 1) as f32,
        default: 0.0,
        kind: ParamKind::Enum { variants: &QUALITY_VARIANTS },
    },
    ParamDesc {
        name: "master-gain",
        label: "Master Gain",
        min: 0.0,
        max: MAX_MASTER_GAIN,
        default: 1.0,
        kind: float("", Taper::Linear),
    },
    macro_desc!("1"), macro_desc!("2"), macro_desc!("3"), macro_desc!("4"),
    macro_desc!("5"), macro_desc!("6"), macro_desc!("7"), macro_desc!("8"),
];

// ── Lookup ──────────────────────────────────────────────────────────────────

/// The descriptor for an id, or `None` if it is out of range.
pub fn desc(id: ParamId) -> Option<&'static ParamDesc> {
    let i = id.raw();
    Some(if i < PM_BASE {
        &OP_DESCS[i / N_PER_OP][i % N_PER_OP]
    } else if i < OUT_BASE {
        let k = i - PM_BASE;
        &PM_DESCS[k / NOPS][k % NOPS]
    } else if i < MATRIX_BASE {
        &OUT_DESCS[i - OUT_BASE]
    } else if i < PATCH_GAIN {
        &MATRIX_DESCS[i - MATRIX_BASE]
    } else if i == PATCH_GAIN {
        &PATCH_GAIN_DESC
    } else if i < N_PARAMS {
        &HOST_DESCS[i - PATCH_PARAMS]
    } else {
        return None;
    })
}

/// Resolve a machine name to its id.
///
/// Linear over the table. Only ever called from the main thread — a preset load
/// resolves a few hundred names once — so the scan is not worth an index.
pub fn id_for_name(name: &str) -> Option<ParamId> {
    (0..N_PARAMS)
        .map(ParamId::new)
        .find(|&id| desc(id).is_some_and(|d| d.name == name))
}

/// Every id in the table, patch region first.
pub fn all_ids() -> impl Iterator<Item = ParamId> {
    (0..N_PARAMS).map(ParamId::new)
}

/// Every id the preset codec writes.
pub fn patch_ids() -> impl Iterator<Item = ParamId> {
    (0..PATCH_PARAMS).map(ParamId::new)
}

/// The waveform a `wave` param's value names.
pub fn waveform_from(v: f32) -> Waveform {
    Waveform::ALL[(v.round().max(0.0) as usize).min(Waveform::ALL.len() - 1)]
}

/// Resolve an enum param's stored **variant label** to its value, falling back
/// to the descriptor's default when the label is not one this build knows.
///
/// The fallback is the point. A preset written by a later build can name a
/// waveform this one has never heard of, and the alternative to defaulting is
/// refusing to load the whole file over one field. The codec surfaces the
/// substitution as a warning (0383); it must not be silent, but it must not be
/// fatal either.
///
/// Non-enum descriptors return their default, since a label means nothing to
/// them — a caller with a number should be going through
/// [`ParamDesc::parse`](vxn_core_app::ParamDesc::parse).
pub fn variant_or_default(d: &ParamDesc, label: &str) -> f32 {
    match d.kind {
        ParamKind::Enum { .. } => d.variant_index(label).map_or(d.default, |i| i as f32),
        _ => d.default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::eg::EgParams;
    use crate::matrix::ROSTER_DEST_NAMES;

    #[test]
    fn the_table_is_the_size_the_layout_implies() {
        assert_eq!(PM_BASE, NOPS * N_PER_OP);
        assert_eq!(
            PATCH_PARAMS,
            NOPS * N_PER_OP + NOPS * NOPS + NOPS + N_MATRIX_SLOTS + 1
        );
        assert_eq!(N_PARAMS, PATCH_PARAMS + N_HOST_PARAMS);
        // Every id resolves. A block added without extending `desc` fails here
        // rather than at the first preset load.
        assert!(all_ids().all(|id| desc(id).is_some()));
        assert!(desc(ParamId::new(N_PARAMS)).is_none());
    }

    #[test]
    fn every_id_round_trips_through_its_name() {
        for id in all_ids() {
            let name = desc(id).unwrap().name;
            assert_eq!(
                id_for_name(name),
                Some(id),
                "name {name} did not round-trip"
            );
        }
    }

    #[test]
    fn every_id_round_trips_through_decode() {
        for id in all_ids() {
            let p = decode(id).expect("id decodes");
            // Macro knobs decode to `Macro1` plus an offset, so they are the
            // one family `encode` cannot invert positionally.
            if macro_index(id).is_some() {
                continue;
            }
            assert_eq!(encode(p), id, "{p:?} did not round-trip");
        }
        for m in 0..N_MACROS {
            assert_eq!(macro_index(macro_id(m)), Some(m));
        }
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<&str> = all_ids().map(|id| desc(id).unwrap().name).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "duplicate param name in the table");
    }

    #[test]
    fn pm_ids_agree_with_the_matrix_layout() {
        for dest in 0..NOPS {
            for src in 0..NOPS {
                let id = encode(Param::Pm { dest, src });
                assert_eq!(decode(id), Some(Param::Pm { dest, src }));
                // The descriptor's name must match the destination's, since
                // both name the same route.
                let d = desc(id).unwrap();
                assert_eq!(d.name, ROSTER_DEST_NAMES[pm_dest_index(dest, src)]);
            }
        }
    }

    #[test]
    fn every_per_op_dest_family_has_a_param() {
        // The four per-operator families that pair with a param, plus the two
        // route families. `Damp`, `Ratio`, `Pan` and `Spread` are the ones a
        // future field could quietly fail to cover.
        for op in 0..NOPS {
            let pairs = [
                (OpField::Ratio, ratio_dest_index(op)),
                (OpField::Pan, pan_dest_index(op)),
                (OpField::DampHz, damp_dest_index(op)),
                (OpField::PhaseSpread, spread_dest_index(op)),
            ];
            for (field, dest) in pairs {
                assert_eq!(dest_for(encode(Param::Op { op, field })), Some(dest));
            }
            assert_eq!(
                dest_for(encode(Param::Out { op })),
                Some(out_dest_index(op))
            );
        }
    }

    #[test]
    fn the_patch_region_excludes_the_host_params() {
        assert!(is_patch_field(encode(Param::PatchGain)));
        assert!(!is_patch_field(macro_id(0)));
        assert!(!is_patch_field(encode(Param::Host(HostField::Patch))));
        assert!(patch_ids().all(is_patch_field));
        // A macro's position belongs to the project, not the sound.
        assert!(patch_ids().all(|id| macro_index(id).is_none()));
    }

    #[test]
    fn the_two_id_spaces_meet_only_at_the_host_region() {
        assert!(patch_ids().all(|id| clap_id_for(id).is_none()));
        for clap_id in 0..N_HOST_PARAMS {
            let id = param_for_clap(clap_id).expect("host param resolves");
            assert_eq!(clap_id_for(id), Some(clap_id));
        }
        assert!(param_for_clap(N_HOST_PARAMS).is_none());
        // The host order is `vxn4_clap::params::decode`'s: patch, quality,
        // master gain, then the macros.
        assert_eq!(desc(param_for_clap(0).unwrap()).unwrap().name, "patch");
        assert_eq!(
            desc(param_for_clap(2).unwrap()).unwrap().name,
            "master-gain"
        );
        assert_eq!(desc(param_for_clap(3).unwrap()).unwrap().name, "macro-1");
    }

    #[test]
    fn eg_defaults_match_egparams() {
        let d = EgParams::default();
        assert_eq!(EG_DEFAULT_T, d.t);
        assert_eq!(EG_DEFAULT_L, d.l);
    }

    #[test]
    fn defaults_sit_inside_their_ranges() {
        for id in all_ids() {
            let d = desc(id).unwrap();
            assert!(d.min <= d.max, "{} has an inverted range", d.name);
            assert!(
                d.default >= d.min && d.default <= d.max,
                "{} default {} is outside [{}, {}]",
                d.name,
                d.default,
                d.min,
                d.max
            );
        }
    }

    #[test]
    fn every_taper_is_invertible() {
        // A BipolarExp whose `mid` is not strictly below `max/2` silently falls
        // back to linear, and an Exp pinned outside its range emits NaN into a
        // fader. Both are configuration errors in this file, so they are caught
        // here rather than seen as a control that feels wrong.
        for id in all_ids() {
            let d = desc(id).unwrap();
            for step in 0..=20 {
                let n = step as f32 / 20.0;
                let v = d.from_fader(n);
                assert!(v.is_finite(), "{} produced {v} at fader {n}", d.name);
                let back = d.to_fader(v);
                assert!(back.is_finite(), "{} produced {back} for {v}", d.name);
                assert!(
                    (back - n).abs() < 1e-3,
                    "{} did not round-trip fader {n} (got {back} via {v})",
                    d.name
                );
            }
        }
    }

    #[test]
    fn wave_variants_match_the_dsp_enum() {
        assert_eq!(WAVE_VARIANTS.len(), Waveform::ALL.len());
        for (i, w) in Waveform::ALL.iter().enumerate() {
            assert_eq!(waveform_from(i as f32), *w);
        }
        // Out-of-range clamps rather than panicking: an unknown label decodes
        // to the default, and a corrupt value must not take the process down.
        assert_eq!(waveform_from(-3.0), Waveform::Sine);
        assert_eq!(waveform_from(99.0), Waveform::Square);
    }

    #[test]
    fn macro_count_matches_the_matrix() {
        assert_eq!(N_MACROS, crate::matrix::N_MACROS);
    }

    #[test]
    fn an_unknown_enum_label_falls_back_to_the_default() {
        let wave = desc(encode(Param::Op {
            op: 3,
            field: OpField::Wave,
        }))
        .unwrap();
        assert_eq!(variant_or_default(wave, "saw"), 2.0);
        // Case-insensitive, since a hand-edited preset is a plain text file.
        assert_eq!(variant_or_default(wave, "SAW"), 2.0);
        // A waveform from a later build defaults rather than refusing the file.
        assert_eq!(variant_or_default(wave, "abs-sine"), wave.default);
        assert_eq!(variant_or_default(wave, ""), wave.default);

        let quality = desc(param_for_clap(1).unwrap()).unwrap();
        assert_eq!(variant_or_default(quality, "16x"), 1.0);
    }
}
