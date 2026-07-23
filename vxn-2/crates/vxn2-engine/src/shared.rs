//! Lock-free parameter store + audio-thread snapshot.
//!
//! A flat `[AtomicU32; TOTAL_PARAMS]` of plain `f32` values stored as bits,
//! indexed by stable CLAP id. The store is the single source of truth between the
//! host (CLAP automation), the UI (knob writes), and the audio thread
//! (snapshot once per block into [`EngineParams`]).
//!
//! Writes clamp to the descriptor's plain range; normalised getters/setters
//! apply the descriptor taper. All atomics use `Ordering::Relaxed` — the
//! audio thread tolerates seeing a partial write under contention (one
//! sample lands one cycle late) and stronger orderings would buy nothing for
//! scalar param updates.
//!
//! ## Snapshot
//!
//! [`EngineParams`] is the audio-side mirror: one [`Patch`], patch-mod params
//! (LFO1), delay + reverb + master + a single [`AllocParams`] derived from the
//! assignment block, and the 8-slot CLAP-automatable matrix depths.
//!
//! [`EngineParams::snapshot_from`] walks the flat store once per control
//! block and routes each id into the matching field — straight indexed
//! reads, no allocation.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use vxn2_dsp::delay::StereoDelayParams;
use vxn2_dsp::eg::EgCurve;
use vxn2_dsp::envelope::{AdsrShape, ModEnvParams, PitchEgParams};
use vxn2_dsp::filter::{FilterMode, FilterSlope};
use vxn2_dsp::lfo::{Lfo1Params, Lfo2Params, LfoShape};
use vxn2_dsp::op::{OpParams, RatioMode};
use vxn2_dsp::dynamics::DynamicsParams;
use vxn2_dsp::phaser::PhaserParams;
use vxn2_dsp::reverb::FdnReverbParams;
use vxn2_dsp::stack::{StackDistrib, StackParams};
use vxn2_dsp::voice::VoiceParams;

use crate::alloc::{AllocParams, AssignMode};
use crate::master::MasterParams;
use crate::matrix::{N_CLAP_DEPTH_SLOTS, N_SLOTS as N_MATRIX_RUNTIME_SLOTS};
use crate::modulation::PatchModParams;
use crate::params::{
    N_OPS, N_PER_OP, OFF_ALGO, OFF_ASSIGN, OFF_DELAY, OFF_FEEDBACK, OFF_LFO1,
    OFF_FILTER, OFF_HP, OFF_LFO2, OFF_LIMITER, OFF_MASTER, OFF_MOD_ENV, OFF_MTX, OFF_PEG,
    OFF_DYNAMICS, OFF_PHASER, OFF_REVERB, OFF_STACK,
    PARAMS, PATCH_BASE, TOTAL_PARAMS, core_desc_for_clap_id,
};

/// A complete patch parameter set: one stack + voice pair. The matrix slot
/// table lives next to the engine (one [`crate::matrix::MatrixTable`] per patch).
#[derive(Clone, Copy, Debug, Default)]
pub struct Patch {
    pub stack: StackParams,
    pub voice: VoiceParams,
}

/// Errors returned by [`ParamModel::load_bytes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamLoadError {
    /// First 4 bytes were not `b"VXN2"`.
    MagicMismatch,
    /// Version field is not [`BLOB_VERSION`] — only the current version loads.
    UnsupportedVersion(u16),
    /// Header count differs from [`TOTAL_PARAMS`] (exact match required).
    CountMismatch { expected: u16, got: u16 },
    /// Payload length not equal to `8 + count × 4` (header + values).
    LengthMismatch { expected: usize, got: usize },
}

impl std::fmt::Display for ParamLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MagicMismatch => write!(f, "param blob magic mismatch"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported param blob version {v}"),
            Self::CountMismatch { expected, got } => {
                write!(f, "param count {got} (expected {expected})")
            }
            Self::LengthMismatch { expected, got } => {
                write!(f, "param payload length {got} (expected {expected})")
            }
        }
    }
}

impl std::error::Error for ParamLoadError {}

/// Magic prefix on every VXN2 host-state blob.
pub const BLOB_MAGIC: &[u8; 4] = b"VXN2";
/// Blob format version. **One version only**: vxn-2 carries no blob back-compat
/// obligation pre-1.0.0. [`load_bytes`](SharedParams::load_bytes) accepts only
/// `version == BLOB_VERSION`; any other version is rejected with
/// [`ParamLoadError::UnsupportedVersion`].
///
/// Layout: header (8 B) + `f32` values for every CLAP id, then three fixed
/// trailers — matrix topology + non-automatable slot depths
/// ([`BLOB_MATRIX_LEN`]), packed per-side KS level-curve selectors
/// ([`BLOB_KS_CURVE_LEN`]), and packed per-op EG level-curve selectors
/// ([`BLOB_EG_CURVE_LEN`]). Append discipline is enforced mechanically by the
/// section-offset `const` asserts below.
pub const BLOB_VERSION: u16 = 1;
/// Header byte length: 4 magic + 2 version + 2 count.
pub const BLOB_HEADER_LEN: usize = 8;
/// Trailing matrix-meta byte length:
/// `N_MATRIX_SLOTS * 4 + (N_MATRIX_SLOTS - N_MATRIX_CLAP_SLOTS) * 4`.
pub const BLOB_MATRIX_LEN: usize =
    N_MATRIX_SLOTS * 4 + (N_MATRIX_SLOTS - N_MATRIX_CLAP_SLOTS) * 4;

// Section-offset compile-time guards.
//
// Each `OFF_*` offset (and the op-block stride) names a section anchor in the
// flat `PARAMS` table that the section readers + `snapshot_from` index by
// `base + k`. Inserting a param mid-section silently shifts every later offset;
// these `const _` asserts pin each anchor to its expected descriptor id so a
// mid-section insert fails to *compile* instead of corrupting the decode. Zero
// runtime cost. Adding a param appended at the tail of a section requires
// moving that section's downstream anchor here in lock-step — by design.

/// `const`-context string equality for the section-offset asserts below.
const fn id_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

// Op block: anchor (op1) + stride (op2) + the two trailing slots `read_op`
// reads by literal index (20 = ratio-mode, 21 = phase).
const _: () = assert!(id_eq(PARAMS[0].id, "op1-num"));
const _: () = assert!(id_eq(PARAMS[N_PER_OP].id, "op2-num"));
const _: () = assert!(id_eq(PARAMS[20].id, "op1-ratio-mode"));
const _: () = assert!(id_eq(PARAMS[21].id, "op1-phase"));

// Per-patch sections (absolute ids in the per-patch space).
const _: () = assert!(id_eq(PARAMS[OFF_ALGO].id, "algo"));
const _: () = assert!(id_eq(PARAMS[OFF_FEEDBACK].id, "feedback"));
const _: () = assert!(id_eq(PARAMS[OFF_LFO2].id, "lfo2-shape"));
const _: () = assert!(id_eq(PARAMS[OFF_LFO2 + 4].id, "lfo2-sync"));
const _: () = assert!(id_eq(PARAMS[OFF_PEG].id, "peg-r1"));
const _: () = assert!(id_eq(PARAMS[OFF_PEG + 8].id, "peg-depth"));
const _: () = assert!(id_eq(PARAMS[OFF_MOD_ENV].id, "mod-env-a"));
const _: () = assert!(id_eq(PARAMS[OFF_MOD_ENV + 4].id, "mod-env-shape"));
const _: () = assert!(id_eq(PARAMS[OFF_ASSIGN].id, "assign-mode"));
const _: () = assert!(id_eq(PARAMS[OFF_ASSIGN + 2].id, "glide-time"));
const _: () = assert!(id_eq(PARAMS[OFF_STACK].id, "stack-density"));
const _: () = assert!(id_eq(PARAMS[OFF_STACK + 4].id, "stack-distrib"));
const _: () = assert!(id_eq(PARAMS[OFF_MTX].id, "mtx1-depth"));
const _: () = assert!(id_eq(PARAMS[OFF_MTX + 7].id, "mtx8-depth"));

// Patch-level sections (relative to `PATCH_BASE`). Each anchor + the section's
// trailing field, so both the start and the width are locked.
const _: () = assert!(id_eq(PARAMS[PATCH_BASE + OFF_LFO1].id, "lfo1-shape"));
const _: () = assert!(id_eq(PARAMS[PATCH_BASE + OFF_LFO1 + 2].id, "lfo1-sync"));
const _: () = assert!(id_eq(PARAMS[PATCH_BASE + OFF_DELAY].id, "delay-on"));
const _: () = assert!(id_eq(PARAMS[PATCH_BASE + OFF_DELAY + 5].id, "delay-pingpong"));
const _: () = assert!(id_eq(PARAMS[PATCH_BASE + OFF_REVERB].id, "reverb-on"));
const _: () = assert!(id_eq(PARAMS[PATCH_BASE + OFF_REVERB + 4].id, "reverb-mix"));
const _: () = assert!(id_eq(PARAMS[PATCH_BASE + OFF_MASTER].id, "master-tune"));
const _: () = assert!(id_eq(PARAMS[PATCH_BASE + OFF_MASTER + 1].id, "master-volume"));
const _: () = assert!(id_eq(PARAMS[PATCH_BASE + OFF_FILTER].id, "filter-enable"));
const _: () = assert!(id_eq(PARAMS[PATCH_BASE + OFF_FILTER + 7].id, "filter-cutoff-tuned"));
const _: () = assert!(id_eq(PARAMS[PATCH_BASE + OFF_LIMITER].id, "limiter-on"));
const _: () = assert!(id_eq(PARAMS[PATCH_BASE + OFF_HP].id, "hp-cutoff"));
const _: () = assert!(id_eq(PARAMS[PATCH_BASE + OFF_PHASER].id, "phaser-on"));
const _: () = assert!(id_eq(PARAMS[PATCH_BASE + OFF_PHASER + 4].id, "phaser-mix"));
const _: () = assert!(id_eq(PARAMS[PATCH_BASE + OFF_DYNAMICS].id, "dyn-on"));
const _: () = assert!(id_eq(PARAMS[PATCH_BASE + OFF_DYNAMICS + 7].id, "dyn-mix"));

/// KS-curve trailer byte length: one `u32` packing all
/// `N_OPS * 2` per-side curve selectors (2 bits each — see
/// [`SharedParams::ks_curve_meta`]).
pub const BLOB_KS_CURVE_LEN: usize = 4;

/// Number of independently-set KS level-curve fields: one per op, per side.
pub const N_KS_CURVES: usize = N_OPS * 2;

/// Bit offset of op `op`'s `side` (0 = left/below BP, 1 = right/above BP)
/// curve selector within the packed `ks_curve_meta` word. 2 bits per field.
#[inline]
const fn ks_curve_shift(op: usize, side: usize) -> u32 {
    ((op * 2 + side) * 2) as u32
}

/// Default packed `ks_curve_meta`: every left side = `NegLin` (0), every
/// right side = `NegExp` (2).
const fn default_ks_curve_meta() -> u32 {
    let mut packed = 0u32;
    let mut op = 0;
    while op < N_OPS {
        // left stays NegLin (0); right = NegExp (discriminant 2).
        packed |= 2u32 << ks_curve_shift(op, 1);
        op += 1;
    }
    packed
}

/// Decode a 2-bit curve field to [`vxn2_dsp::ks::KsCurve`].
#[inline]
fn ks_curve_from_bits(bits: u8) -> vxn2_dsp::ks::KsCurve {
    use vxn2_dsp::ks::KsCurve::*;
    match bits & 0b11 {
        0 => NegLin,
        1 => PosLin,
        2 => NegExp,
        _ => PosExp,
    }
}

/// EG-curve trailer byte length: one `u32` packing all `N_OPS` per-op
/// level→amplitude curve selectors (1 bit each — see
/// [`SharedParams::eg_curve_meta`]).
pub const BLOB_EG_CURVE_LEN: usize = 4;

/// Number of independently-set EG level-curve fields: one per op.
pub const N_EG_CURVES: usize = N_OPS;

/// Bit offset of op `op`'s EG level-curve selector within the packed
/// `eg_curve_meta` word. 1 bit per op (`0 = Exp`, `1 = Lin`).
#[inline]
const fn eg_curve_shift(op: usize) -> u32 {
    op as u32
}

/// Default packed `eg_curve_meta`: every op = `Exp` (0) — the log curve is the
/// shipped default.
const fn default_eg_curve_meta() -> u32 {
    0
}

/// Decode a 1-bit EG-curve field to [`EgCurve`].
#[inline]
fn eg_curve_from_bits(bits: u8) -> EgCurve {
    if bits & 0b1 == 0 {
        EgCurve::Exp
    } else {
        EgCurve::Lin
    }
}

/// Indexed read access into a param store, keyed by CLAP id.
///
/// Internal supertrait of [`ParamModel`]; both the atomic store and the
/// audio-thread mirror in `vxn2-clap::local` implement it so the section
/// readers below (and [`EngineParams::snapshot_from`]) can drive either.
///
/// `matrix_row_raw` carries the non-CLAP topology fields (source / dest /
/// curve / active) plus the depth — slots 1..=N_MATRIX_CLAP_SLOTS take depth
/// from the CLAP `values` table, slots past that ride a parallel
/// non-automatable depth field. Default impl returns an inert row so callers
/// that don't need topology (older test fixtures) compile unchanged.
pub trait ParamView {
    fn get(&self, id: usize) -> f32;
    /// Read the matrix row at `slot`. Out-of-range → zeroed row (source/dest
    /// = None, depth = 0). Override in stores that carry topology; default is
    /// inert so [`EngineParams::snapshot_from`] callers that don't need
    /// topology don't have to implement it.
    fn matrix_row_raw(&self, _slot: usize) -> MatrixRowRaw {
        MatrixRowRaw::default()
    }
    /// Read op `op`'s `side` (0 = left, 1 = right) KS level curve. Default
    /// (left `NegLin`, right `NegExp`) so stores that don't carry curve state
    /// behave as the shipped default.
    fn ks_curve(&self, _op: usize, side: usize) -> vxn2_dsp::ks::KsCurve {
        if side == 0 {
            vxn2_dsp::ks::KsCurve::NegLin
        } else {
            vxn2_dsp::ks::KsCurve::NegExp
        }
    }
    /// Read op `op`'s EG level→amplitude curve. Default [`EgCurve::Exp`] (the
    /// log curve) so stores that don't carry curve state behave as the shipped
    /// default.
    fn eg_curve(&self, _op: usize) -> EgCurve {
        EgCurve::Exp
    }
}

/// Main-thread parameter-model surface bound by the CLAP params + state
/// extensions. A second implementation backs a view-side mirror with
/// view-event emission; both satisfy this trait so the CLAP shell stays
/// swappable.
pub trait ParamModel: ParamView {
    fn total(&self) -> usize;
    fn get_normalised(&self, id: usize) -> f32;
    fn snapshot_bytes(&self) -> Vec<u8>;
    fn load_bytes(&self, bytes: &[u8]) -> Result<(), ParamLoadError>;
}

/// Number of mod-matrix slots (CLAP-automatable + patch state).
pub const N_MATRIX_SLOTS: usize = 16;
/// CLAP-automatable matrix slots. The remaining
/// `N_MATRIX_SLOTS - N_MATRIX_CLAP_SLOTS` are patch state — their depth
/// rides [`SharedParams::matrix_extra_depth`].
pub const N_MATRIX_CLAP_SLOTS: usize = 8;
/// Gesture bitset word count (one `AtomicU64` per 64 params, rounded up).
const GESTURE_WORDS: usize = (TOTAL_PARAMS + 63) / 64;
/// Dirty-bitset word count for the value table. One bit per CLAP id;
/// flipped on every `set` / `set_normalised` / `set_matrix_row_raw` write
/// and drained by the main-thread tick.
pub const N_DIRTY_VALUE_WORDS: usize = (TOTAL_PARAMS + 63) / 64;

/// Mask of the valid bits in dirty-value word `w` (out-of-range bits in
/// the last word stay zero so a full re-broadcast doesn't emit phantom
/// ids past [`TOTAL_PARAMS`]).
#[inline]
const fn dirty_values_full_word(w: usize) -> u64 {
    let start = w * 64;
    if start >= TOTAL_PARAMS {
        0
    } else {
        let n = TOTAL_PARAMS - start;
        if n >= 64 {
            u64::MAX
        } else {
            (1u64 << n) - 1
        }
    }
}

/// All matrix-slot dirty bits set (16-bit fully-occupied mask). Used to
/// force a whole-table `MatrixSnapshot` after a bulk store (state load,
/// reset to defaults, first tick post-init).
const DIRTY_MATRIX_ALL: u64 = (1u64 << N_MATRIX_SLOTS) - 1;

/// Lock-free param store. Sized to [`TOTAL_PARAMS`]. Cheap to
/// share via `Arc` — every field is an atomic.
///
/// Beyond the CLAP-automatable `values` array the store also holds the
/// non-CLAP shared state the controller needs to read / write: per-param
/// gesture flags, matrix-row topology (source / dest / curve / active),
/// slot 9-16 depths.
pub struct SharedParams {
    values: [AtomicU32; TOTAL_PARAMS],
    /// Bitset (one bit per CLAP id): set while the editor is mid-gesture
    /// on that param. Host automation arriving while the bit is set is
    /// applied to `values` but not echoed back to the page (the page is
    /// the source of truth during a drag — see
    /// `vxn_core_app::Controller::handle_host`).
    gestures: [AtomicU64; GESTURE_WORDS],
    /// Per-slot packed `(source, dest, curve, active)` as
    /// `source << 24 | dest << 16 | curve << 8 | active`.
    matrix_meta: [AtomicU32; N_MATRIX_SLOTS],
    /// Slot 9-16 depth (`f32` bits). Slots 1-8 depth lives in the CLAP
    /// `values` table (see [`OFF_MTX`]).
    matrix_extra_depth: [AtomicU32; N_MATRIX_SLOTS - N_MATRIX_CLAP_SLOTS],
    /// Dirty bitset for the value table — the canonical Model → View
    /// change channel. Every `set` / `set_normalised` /
    /// `set_matrix_row_raw` write site flips the matching bit with
    /// `fetch_or(Release)`; the main-thread tick drains via
    /// `take_dirty_values` (`swap(Acquire)`). The Release/Acquire pair
    /// guarantees a reader that observes the bit sees the value the
    /// writer stored before flipping it.
    ///
    /// Seeded with every valid bit set so the first tick after open
    /// broadcasts the whole table.
    dirty_values: [AtomicU64; N_DIRTY_VALUE_WORDS],
    /// Dirty bitset for matrix-slot topology (one bit per slot). Any
    /// non-zero word triggers a whole-table `MatrixSnapshot` push on the
    /// next tick. Slot bits cover both meta drift and the slot-9-16
    /// depth side-table; slot 1-8 depth drift also rides
    /// [`dirty_values`] (its CLAP id lives in [`OFF_MTX`]).
    dirty_matrix: AtomicU64,
    /// Per-op, per-side KS level-curve selectors packed 2 bits each
    /// (`N_OPS * 2` fields). Non-CLAP / non-automatable patch state —
    /// persisted in the blob trailer and the preset `params` table,
    /// mirrored into the audio thread via [`ParamView::ks_curve`]. Field
    /// layout: [`ks_curve_shift`]; values: [`vxn2_dsp::ks::KsCurve`]
    /// discriminants. Default [`default_ks_curve_meta`].
    ks_curve_meta: AtomicU32,
    /// Set by `set_ks_curve_raw`; drained by the main-thread tick
    /// ([`take_dirty_ks_curve`]) to push a `KsCurveSnapshot` to the page.
    dirty_ks_curve: AtomicBool,
    /// Per-op EG level→amplitude curve selectors packed 1 bit each (`N_OPS`
    /// fields, `0 = Exp`, `1 = Lin`). Non-CLAP / non-automatable patch state —
    /// persisted in the blob trailer and the preset `params` table,
    /// mirrored into the audio thread via [`ParamView::eg_curve`]. Field
    /// layout: [`eg_curve_shift`]; values: [`EgCurve`] discriminants. Default
    /// [`default_eg_curve_meta`] (every op `Exp`).
    eg_curve_meta: AtomicU32,
    /// Set by `set_eg_curve_raw`; drained by the main-thread tick
    /// ([`take_dirty_eg_curve`]).
    dirty_eg_curve: AtomicBool,
    /// Monotonic counter bumped by every bulk patch swap
    /// ([`load_bytes`](Self::load_bytes), [`reset_to_defaults`](Self::reset_to_defaults)).
    /// The audio engine snapshots it once per block; a change is the cue that a
    /// new preset was loaded, so it can silence still-ringing voices from the
    /// previous patch — they would otherwise sound through the new algorithm,
    /// often mis-roled (a former modulator now a carrier).
    load_epoch: AtomicU64,
}

impl Default for SharedParams {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedParams {
    /// Initialise every slot to its illustrative default-patch value
    /// (see [`crate::default_patch::default_param_values`]).
    pub fn new() -> Self {
        let defaults = crate::default_patch::default_param_values();
        let default_matrix = crate::default_patch::default_matrix();
        Self {
            values: std::array::from_fn(|i| AtomicU32::new(defaults[i].to_bits())),
            gestures: std::array::from_fn(|_| AtomicU64::new(0)),
            matrix_meta: std::array::from_fn(|s| {
                let slot = default_matrix.slots[s];
                let active = slot.source != crate::matrix::SourceId::None
                    && slot.dest != crate::matrix::DestId::None;
                AtomicU32::new(pack_matrix_meta(
                    slot.source as u8,
                    slot.dest as u8,
                    slot.curve as u8,
                    active,
                    slot.scale_src as u8,
                ))
            }),
            matrix_extra_depth: std::array::from_fn(|s| {
                let slot_idx = s + N_MATRIX_CLAP_SLOTS;
                AtomicU32::new(default_matrix.slots[slot_idx].depth.to_bits())
            }),
            // Full-broadcast seed: first tick after open pushes every id
            // + a MatrixSnapshot, hydrating the editor with current
            // state without a bespoke push from the caller.
            dirty_values: std::array::from_fn(|w| AtomicU64::new(dirty_values_full_word(w))),
            dirty_matrix: AtomicU64::new(DIRTY_MATRIX_ALL),
            ks_curve_meta: AtomicU32::new(default_ks_curve_meta()),
            // Seed dirty so the first tick pushes a KsCurveSnapshot alongside
            // the full value + matrix re-broadcast.
            dirty_ks_curve: AtomicBool::new(true),
            eg_curve_meta: AtomicU32::new(default_eg_curve_meta()),
            dirty_eg_curve: AtomicBool::new(true),
            load_epoch: AtomicU64::new(0),
        }
    }

    /// Monotonic patch-swap counter. Bumped whenever a whole patch is loaded in
    /// bulk ([`load_bytes`](Self::load_bytes) /
    /// [`reset_to_defaults`](Self::reset_to_defaults)); left untouched by
    /// per-parameter [`set`](Self::set)s (automation, single-knob edits). The
    /// audio engine reads it each block to detect a preset change.
    #[inline]
    pub fn load_epoch(&self) -> u64 {
        self.load_epoch.load(Ordering::Acquire)
    }

    /// Bump the load epoch to signal a patch swap. The native host
    /// shares one `SharedParams`, so `reset_to_defaults` / `load_bytes` bump it
    /// directly. The web build's controller and worklet hold SEPARATE
    /// `SharedParams`, and the epoch is not a value param, so it can't ride the
    /// store mirror — the controller pushes an `EV_PATCH_SWAP` ring event that
    /// calls this on the worklet's copy so `snapshot_params` silences the old
    /// patch's still-ringing voices before the new patch's params take effect.
    #[inline]
    pub fn bump_load_epoch(&self) {
        self.load_epoch.fetch_add(1, Ordering::Release);
    }

    /// SAFETY: `get` and `set` use `Ordering::Relaxed` — sound because CLAP
    /// guarantees the audio thread (`process`) and the main thread
    /// (`params_flush`, UI callbacks) never run concurrently on the same plugin
    /// instance. The audio thread's `LocalParams` mirror is a plain `[f32]`
    /// (no atomics) for the same reason. Any field added to `LocalParams` that
    /// crosses the audio/main-thread boundary must use an atomic, or the CLAP
    /// non-overlap guarantee must be verified to cover it.
    #[inline]
    pub fn get(&self, id: usize) -> f32 {
        f32::from_bits(self.values[id].load(Ordering::Relaxed))
    }

    /// Store `value` clamped to the descriptor's plain range.
    ///
    /// Flips the matching `dirty_values` bit with `fetch_or(Release)`
    /// after the value store so the main-thread tick observes the
    /// change on its next drain.
    #[inline]
    pub fn set(&self, id: usize, value: f32) {
        if id < TOTAL_PARAMS {
            let d = &PARAMS[id];
            self.values[id].store(d.clamp(value).to_bits(), Ordering::Relaxed);
            self.dirty_values[id / 64].fetch_or(1u64 << (id % 64), Ordering::Release);
        }
    }

    /// Read as a normalised `[0, 1]` value (taper-aware).
    #[inline]
    pub fn get_normalised(&self, id: usize) -> f32 {
        if id < TOTAL_PARAMS {
            PARAMS[id].to_normalised(self.get(id))
        } else {
            0.0
        }
    }

    /// Write from a normalised `[0, 1]` value. Inverse of [`get_normalised`].
    #[inline]
    pub fn set_normalised(&self, id: usize, n: f32) {
        if id < TOTAL_PARAMS {
            self.set(id, PARAMS[id].from_normalised(n));
        }
    }

    /// Restore every slot to its illustrative default-patch value
    /// (see [`crate::default_patch::default_param_values`]). Triggers a
    /// full Model → View re-broadcast on the next tick by flipping all
    /// dirty bits.
    pub fn reset_to_defaults(&self) {
        let defaults = crate::default_patch::default_param_values();
        for i in 0..TOTAL_PARAMS {
            self.values[i].store(defaults[i].to_bits(), Ordering::Relaxed);
        }
        let default_matrix = crate::default_patch::default_matrix();
        for s in 0..N_MATRIX_SLOTS {
            let slot = default_matrix.slots[s];
            let active = slot.source != crate::matrix::SourceId::None
                && slot.dest != crate::matrix::DestId::None;
            let packed = pack_matrix_meta(
                slot.source as u8,
                slot.dest as u8,
                slot.curve as u8,
                active,
                slot.scale_src as u8,
            );
            self.matrix_meta[s].store(packed, Ordering::Relaxed);
        }
        for s in 0..(N_MATRIX_SLOTS - N_MATRIX_CLAP_SLOTS) {
            self.matrix_extra_depth[s].store(
                default_matrix.slots[s + N_MATRIX_CLAP_SLOTS].depth.to_bits(),
                Ordering::Relaxed,
            );
        }
        self.ks_curve_meta.store(default_ks_curve_meta(), Ordering::Relaxed);
        self.eg_curve_meta.store(default_eg_curve_meta(), Ordering::Relaxed);
        self.mark_all_dirty();
        // A reset is a patch swap — cue the engine to silence held voices.
        self.load_epoch.fetch_add(1, Ordering::Release);
    }

    /// Set every valid dirty bit (values + matrix). Used by bulk-store
    /// paths (`reset_to_defaults`, `load_bytes`) and the initial seed in
    /// [`Self::new`] to force a full re-broadcast on the next tick.
    /// Also exposed through `Vxn2Params::mark_all_dirty` so the page can
    /// re-seed itself on demand (e.g. after late-binding primitives).
    pub fn mark_all_dirty(&self) {
        for w in 0..N_DIRTY_VALUE_WORDS {
            self.dirty_values[w].fetch_or(dirty_values_full_word(w), Ordering::Release);
        }
        self.dirty_matrix.fetch_or(DIRTY_MATRIX_ALL, Ordering::Release);
        self.dirty_ks_curve.store(true, Ordering::Release);
        self.dirty_eg_curve.store(true, Ordering::Release);
    }

    /// Drain the value dirty bitset, returning a snapshot of the bits
    /// that were set since the last drain. Single-reader contract: only
    /// the main-thread tick calls this; concurrent readers would race
    /// each other on the per-word `swap`. Writers are unrestricted.
    ///
    /// `swap(Acquire)` pairs with each writer's `fetch_or(Release)` so
    /// a subsequent `get(id)` for a popped bit sees the value the
    /// writer stored before flipping the bit.
    pub fn take_dirty_values(&self) -> [u64; N_DIRTY_VALUE_WORDS] {
        let mut out = [0u64; N_DIRTY_VALUE_WORDS];
        for w in 0..N_DIRTY_VALUE_WORDS {
            out[w] = self.dirty_values[w].swap(0, Ordering::Acquire);
        }
        out
    }

    /// Drain the matrix dirty bitset. Same contract as
    /// [`Self::take_dirty_values`]: single reader on the main thread.
    pub fn take_dirty_matrix(&self) -> u64 {
        self.dirty_matrix.swap(0, Ordering::Acquire)
    }

    /// Drain the KS-curve dirty flag. Same single-reader contract as
    /// [`Self::take_dirty_matrix`]; `true` means a `KsCurveSnapshot` should
    /// be pushed to the page this tick.
    pub fn take_dirty_ks_curve(&self) -> bool {
        self.dirty_ks_curve.swap(false, Ordering::Acquire)
    }

    /// Drain the EG-curve dirty flag. Same single-reader contract as
    /// [`Self::take_dirty_ks_curve`].
    pub fn take_dirty_eg_curve(&self) -> bool {
        self.dirty_eg_curve.swap(false, Ordering::Acquire)
    }

    /// Read op `op`'s `side` (0 = left, 1 = right) level-curve discriminant
    /// (0..=3 → `NegLin`/`PosLin`/`NegExp`/`PosExp`). Out-of-range →
    /// default (left `NegLin`, right `NegExp`).
    pub fn ks_curve_raw(&self, op: usize, side: usize) -> u8 {
        if op >= N_OPS || side > 1 {
            return if side == 0 { 0 } else { 2 };
        }
        let packed = self.ks_curve_meta.load(Ordering::Relaxed);
        ((packed >> ks_curve_shift(op, side)) & 0b11) as u8
    }

    /// Write op `op`'s `side` level-curve selector. Single-writer (main
    /// thread); flips `dirty_ks_curve` so the next tick re-pushes the
    /// snapshot.
    pub fn set_ks_curve_raw(&self, op: usize, side: usize, curve: u8) {
        if op >= N_OPS || side > 1 {
            return;
        }
        let shift = ks_curve_shift(op, side);
        let cur = self.ks_curve_meta.load(Ordering::Relaxed);
        let next = (cur & !(0b11 << shift)) | (((curve as u32) & 0b11) << shift);
        self.ks_curve_meta.store(next, Ordering::Relaxed);
        self.dirty_ks_curve.store(true, Ordering::Release);
    }

    /// Read op `op`'s EG level-curve discriminant (0 = `Exp`, 1 = `Lin`).
    /// Out-of-range → default (`Exp`, the log curve).
    pub fn eg_curve_raw(&self, op: usize) -> u8 {
        if op >= N_OPS {
            return 0;
        }
        let packed = self.eg_curve_meta.load(Ordering::Relaxed);
        ((packed >> eg_curve_shift(op)) & 0b1) as u8
    }

    /// Write op `op`'s EG level-curve selector. Single-writer (main thread);
    /// flips `dirty_eg_curve` so the next tick re-broadcasts.
    pub fn set_eg_curve_raw(&self, op: usize, curve: u8) {
        if op >= N_OPS {
            return;
        }
        let shift = eg_curve_shift(op);
        let cur = self.eg_curve_meta.load(Ordering::Relaxed);
        let next = (cur & !(0b1 << shift)) | (((curve as u32) & 0b1) << shift);
        self.eg_curve_meta.store(next, Ordering::Relaxed);
        self.dirty_eg_curve.store(true, Ordering::Release);
    }

    #[inline]
    pub fn gesture(&self, id: usize) -> bool {
        if id >= TOTAL_PARAMS {
            return false;
        }
        let (w, b) = (id / 64, id % 64);
        (self.gestures[w].load(Ordering::Relaxed) >> b) & 1 != 0
    }

    #[inline]
    pub fn set_gesture(&self, id: usize, on: bool) {
        if id >= TOTAL_PARAMS {
            return;
        }
        let (w, b) = (id / 64, id % 64);
        let mask = 1u64 << b;
        if on {
            self.gestures[w].fetch_or(mask, Ordering::Relaxed);
        } else {
            self.gestures[w].fetch_and(!mask, Ordering::Relaxed);
        }
    }

    /// Read the packed `(source, dest, curve, active, depth)` for a
    /// matrix slot. Slot index is `0..N_MATRIX_SLOTS`; out-of-range returns
    /// a zeroed-default row.
    pub fn matrix_row_raw(&self, slot: usize) -> MatrixRowRaw {
        if slot >= N_MATRIX_SLOTS {
            return MatrixRowRaw::default();
        }
        let packed = self.matrix_meta[slot].load(Ordering::Relaxed);
        let depth = if slot < N_MATRIX_CLAP_SLOTS {
            self.get(OFF_MTX + slot)
        } else {
            f32::from_bits(
                self.matrix_extra_depth[slot - N_MATRIX_CLAP_SLOTS].load(Ordering::Relaxed),
            )
        };
        MatrixRowRaw {
            source: ((packed >> 24) & 0xFF) as u8,
            dest: ((packed >> 16) & 0xFF) as u8,
            curve: ((packed >> 8) & 0xFF) as u8,
            active: (packed & 0x01) != 0,
            depth,
            scale_src: ((packed >> 1) & 0x7F) as u8,
        }
    }

    /// Write a matrix row. For slot indices `< N_MATRIX_CLAP_SLOTS`,
    /// `depth` also writes the matching CLAP param so host automation
    /// stays in sync.
    ///
    /// Flips the slot bit in `dirty_matrix` (whole-table snapshot
    /// trigger). For slot 1-8 the inner `set()` also flips the matching
    /// `dirty_values` bit so the depth fader follows automation through
    /// the standard primitive bind.
    pub fn set_matrix_row_raw(&self, slot: usize, row: MatrixRowRaw) {
        if slot >= N_MATRIX_SLOTS {
            return;
        }
        let packed = pack_matrix_meta(row.source, row.dest, row.curve, row.active, row.scale_src);
        self.matrix_meta[slot].store(packed, Ordering::Relaxed);
        if slot < N_MATRIX_CLAP_SLOTS {
            self.set(OFF_MTX + slot, row.depth);
        } else {
            self.matrix_extra_depth[slot - N_MATRIX_CLAP_SLOTS]
                .store(row.depth.clamp(-1.0, 1.0).to_bits(), Ordering::Relaxed);
        }
        self.dirty_matrix.fetch_or(1u64 << slot, Ordering::Release);
    }
}

/// Wire-shape mirror of [`vxn2_app::MatrixRow`] without taking a dep on
/// `vxn2-app` for the inherent methods — the `Vxn2Params` impl below
/// converts between the two.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MatrixRowRaw {
    pub source: u8,
    pub dest: u8,
    pub curve: u8,
    pub active: bool,
    pub depth: f32,
    /// Secondary scale source. `0` = `None` = depth unscaled. Rides the
    /// free low-byte bits of the packed `matrix_meta` word (see
    /// [`pack_matrix_meta`]).
    pub scale_src: u8,
}

/// Pack a matrix row's topology into one `u32`:
/// `source<<24 | dest<<16 | curve<<8 | scale_src<<1 | active`.
///
/// `active` is bit 0; the scale source (≤ 11) rides bits 1..=7 of the low byte.
/// A blob with those bits clear decodes to `scale_src = None`.
#[inline]
pub(crate) fn pack_matrix_meta(source: u8, dest: u8, curve: u8, active: bool, scale_src: u8) -> u32 {
    ((source as u32) << 24)
        | ((dest as u32) << 16)
        | ((curve as u32) << 8)
        | (((scale_src & 0x7F) as u32) << 1)
        | (active as u32)
}

impl ParamView for SharedParams {
    #[inline]
    fn get(&self, id: usize) -> f32 {
        SharedParams::get(self, id)
    }

    #[inline]
    fn matrix_row_raw(&self, slot: usize) -> MatrixRowRaw {
        SharedParams::matrix_row_raw(self, slot)
    }

    #[inline]
    fn ks_curve(&self, op: usize, side: usize) -> vxn2_dsp::ks::KsCurve {
        ks_curve_from_bits(SharedParams::ks_curve_raw(self, op, side))
    }

    #[inline]
    fn eg_curve(&self, op: usize) -> EgCurve {
        eg_curve_from_bits(SharedParams::eg_curve_raw(self, op))
    }
}

impl ParamModel for SharedParams {
    fn total(&self) -> usize {
        TOTAL_PARAMS
    }

    fn get_normalised(&self, id: usize) -> f32 {
        SharedParams::get_normalised(self, id)
    }

    /// Serialise the param table as a host-state blob.
    ///
    /// Wire format (little-endian):
    ///
    /// | offset | bytes | content                          |
    /// |-------:|------:|----------------------------------|
    /// |   0    |   4   | magic `b"VXN2"`                  |
    /// |   4    |   2   | version `u16` (= [`BLOB_VERSION`])|
    /// |   6    |   2   | param count `u16` ([`TOTAL_PARAMS`])|
    /// |   8    | 4 × N | raw `f32` bits, indexed by CLAP id|
    ///
    /// No per-id framing, no name strings — this is the binary host blob,
    /// not the user-facing preset format.
    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(
            BLOB_HEADER_LEN
                + TOTAL_PARAMS * 4
                + BLOB_MATRIX_LEN
                + BLOB_KS_CURVE_LEN
                + BLOB_EG_CURVE_LEN,
        );
        buf.extend_from_slice(BLOB_MAGIC);
        buf.extend_from_slice(&BLOB_VERSION.to_le_bytes());
        buf.extend_from_slice(&(TOTAL_PARAMS as u16).to_le_bytes());
        for i in 0..TOTAL_PARAMS {
            let bits = self.values[i].load(Ordering::Relaxed);
            buf.extend_from_slice(&bits.to_le_bytes());
        }
        // Matrix trailer — topology + non-automatable slot depths.
        for s in 0..N_MATRIX_SLOTS {
            let packed = self.matrix_meta[s].load(Ordering::Relaxed);
            buf.extend_from_slice(&packed.to_le_bytes());
        }
        for s in 0..(N_MATRIX_SLOTS - N_MATRIX_CLAP_SLOTS) {
            let bits = self.matrix_extra_depth[s].load(Ordering::Relaxed);
            buf.extend_from_slice(&bits.to_le_bytes());
        }
        // KS-curve trailer — packed per-side KS level-curve selectors.
        buf.extend_from_slice(&self.ks_curve_meta.load(Ordering::Relaxed).to_le_bytes());
        // EG-curve trailer — packed per-op EG level-curve selectors.
        buf.extend_from_slice(&self.eg_curve_meta.load(Ordering::Relaxed).to_le_bytes());
        buf
    }

    /// Inverse of [`snapshot_bytes`]. Validates magic / version / count /
    /// length, then writes value + trailer bits unmodified — no descriptor
    /// clamp — so a snapshot round-trip is bit-identical.
    ///
    /// Single version only: a blob whose version field is not [`BLOB_VERSION`]
    /// is rejected outright.
    fn load_bytes(&self, bytes: &[u8]) -> Result<(), ParamLoadError> {
        let expected = BLOB_HEADER_LEN
            + TOTAL_PARAMS * 4
            + BLOB_MATRIX_LEN
            + BLOB_KS_CURVE_LEN
            + BLOB_EG_CURVE_LEN;
        if bytes.len() < BLOB_HEADER_LEN {
            return Err(ParamLoadError::LengthMismatch {
                expected,
                got: bytes.len(),
            });
        }
        if &bytes[0..4] != BLOB_MAGIC {
            return Err(ParamLoadError::MagicMismatch);
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != BLOB_VERSION {
            return Err(ParamLoadError::UnsupportedVersion(version));
        }
        let count = u16::from_le_bytes([bytes[6], bytes[7]]);
        if count as usize != TOTAL_PARAMS {
            return Err(ParamLoadError::CountMismatch {
                expected: TOTAL_PARAMS as u16,
                got: count,
            });
        }
        if bytes.len() != expected {
            return Err(ParamLoadError::LengthMismatch {
                expected,
                got: bytes.len(),
            });
        }
        // Value block — one `f32` per CLAP id, indexed 1:1.
        for i in 0..TOTAL_PARAMS {
            let off = BLOB_HEADER_LEN + i * 4;
            let bits = u32::from_le_bytes([
                bytes[off],
                bytes[off + 1],
                bytes[off + 2],
                bytes[off + 3],
            ]);
            self.values[i].store(bits, Ordering::Relaxed);
        }
        // Matrix trailer — topology + non-automatable slot depths.
        let mut off = BLOB_HEADER_LEN + TOTAL_PARAMS * 4;
        for s in 0..N_MATRIX_SLOTS {
            let packed = u32::from_le_bytes([
                bytes[off],
                bytes[off + 1],
                bytes[off + 2],
                bytes[off + 3],
            ]);
            self.matrix_meta[s].store(packed, Ordering::Relaxed);
            off += 4;
        }
        for s in 0..(N_MATRIX_SLOTS - N_MATRIX_CLAP_SLOTS) {
            let bits = u32::from_le_bytes([
                bytes[off],
                bytes[off + 1],
                bytes[off + 2],
                bytes[off + 3],
            ]);
            self.matrix_extra_depth[s].store(bits, Ordering::Relaxed);
            off += 4;
        }
        // KS-curve trailer — packed per-side level-curve selectors.
        let packed = u32::from_le_bytes([
            bytes[off],
            bytes[off + 1],
            bytes[off + 2],
            bytes[off + 3],
        ]);
        self.ks_curve_meta.store(packed, Ordering::Relaxed);
        off += 4;
        // EG-curve trailer — packed per-op level-curve selectors.
        let packed = u32::from_le_bytes([
            bytes[off],
            bytes[off + 1],
            bytes[off + 2],
            bytes[off + 3],
        ]);
        self.eg_curve_meta.store(packed, Ordering::Relaxed);
        // Bulk store bypassed `set` / `set_matrix_row_raw`; flip every
        // dirty bit so the next main-thread tick re-broadcasts the full
        // table.
        self.mark_all_dirty();
        // Loading a blob is a whole-patch swap — bump the epoch so the audio
        // engine silences any voice still ringing from the previous preset.
        self.load_epoch.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

// vxn_core_app::ParamModel + Vxn2Params bridges.
//
// The local [`ParamModel`] above is the audio-thread / state-extension
// surface; the bridges below are the controller surface.
// Method shapes differ — `ParamId` newtype, `normalized` spelling,
// gesture flags, `descriptor` returning core-app's `ParamDesc`, and
// `restore_from_bytes` returning `Result<(), String>` rather than the
// engine's typed error.

impl vxn_core_app::ParamModel for SharedParams {
    fn total(&self) -> usize {
        TOTAL_PARAMS
    }

    fn get(&self, id: vxn_core_app::ParamId) -> f32 {
        SharedParams::get(self, id.raw())
    }

    fn set(&self, id: vxn_core_app::ParamId, plain: f32) {
        SharedParams::set(self, id.raw(), plain);
    }

    fn get_normalized(&self, id: vxn_core_app::ParamId) -> f32 {
        SharedParams::get_normalised(self, id.raw())
    }

    fn set_normalized(&self, id: vxn_core_app::ParamId, norm: f32) {
        SharedParams::set_normalised(self, id.raw(), norm);
    }

    fn gesture(&self, id: vxn_core_app::ParamId) -> bool {
        SharedParams::gesture(self, id.raw())
    }

    fn set_gesture(&self, id: vxn_core_app::ParamId, on: bool) {
        SharedParams::set_gesture(self, id.raw(), on);
    }

    fn descriptor(&self, id: vxn_core_app::ParamId) -> Option<&'static vxn_core_app::ParamDesc> {
        core_desc_for_clap_id(id.raw())
    }

    fn snapshot_bytes(&self) -> Vec<u8> {
        ParamModel::snapshot_bytes(self)
    }

    fn restore_from_bytes(&self, blob: &[u8]) -> Result<(), String> {
        ParamModel::load_bytes(self, blob).map_err(|e| e.to_string())
    }
}

impl vxn2_app::Vxn2Params for SharedParams {
    fn matrix_row(&self, slot: u8) -> vxn2_app::MatrixRow {
        let raw = self.matrix_row_raw(slot as usize);
        vxn2_app::MatrixRow {
            source: raw.source,
            dest: raw.dest,
            curve: raw.curve,
            active: raw.active,
            depth: raw.depth,
            scale_src: raw.scale_src,
        }
    }

    fn set_matrix_row(&self, slot: u8, row: vxn2_app::MatrixRow) {
        self.set_matrix_row_raw(
            slot as usize,
            MatrixRowRaw {
                source: row.source,
                dest: row.dest,
                curve: row.curve,
                active: row.active,
                depth: row.depth,
                scale_src: row.scale_src,
            },
        );
    }

    fn ks_curves(&self) -> [[u8; 2]; 6] {
        std::array::from_fn(|op| {
            [
                SharedParams::ks_curve_raw(self, op, 0),
                SharedParams::ks_curve_raw(self, op, 1),
            ]
        })
    }

    fn set_ks_curve(&self, op: u8, side: u8, curve: u8) {
        SharedParams::set_ks_curve_raw(self, op as usize, side as usize, curve);
    }

    fn take_dirty_ks_curve(&self) -> bool {
        SharedParams::take_dirty_ks_curve(self)
    }

    fn eg_curves(&self) -> [u8; 6] {
        std::array::from_fn(|op| SharedParams::eg_curve_raw(self, op))
    }

    fn set_eg_curve(&self, op: u8, curve: u8) {
        SharedParams::set_eg_curve_raw(self, op as usize, curve);
    }

    fn take_dirty_eg_curve(&self) -> bool {
        SharedParams::take_dirty_eg_curve(self)
    }

    fn mark_all_dirty(&self) {
        SharedParams::mark_all_dirty(self);
    }
}

/// Engine-native shape of the optional per-voice filter section. Mirrors the
/// `filter-*` CLAP params decoded into render-ready types.
/// `enable` off ⇒ the engine takes the sample-major path and ignores
/// every other field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilterParams {
    pub enable: bool,
    /// Base cutoff in Hz (matrix `Cutoff` adds octaves on top, per stack).
    pub cutoff_hz: f32,
    /// Base resonance in `[0, 1]` (matrix `Resonance` adds on top, per stack).
    pub resonance: f32,
    pub mode: FilterMode,
    pub slope: FilterSlope,
    pub drive: f32,
    /// Oversample factor: 1, 2, 4 or 8 (decoded from the enum index `1 << idx`).
    pub oversample: usize,
    /// Key-tracking amount in `[0, 1]`. The engine shifts cutoff by
    /// `(note − 12)/12 × keytrack` octaves (centred on C0); 1.0 tracks the
    /// played pitch exactly.
    pub keytrack: f32,
}

impl Default for FilterParams {
    fn default() -> Self {
        Self {
            enable: false,
            cutoff_hz: 12_000.0,
            resonance: 0.0,
            mode: FilterMode::Lp,
            slope: FilterSlope::Pole4,
            drive: 1.0,
            oversample: 4,
            keytrack: 0.0,
        }
    }
}

// The filter's interp+decimate path adds a real group delay
// (`vxn2_dsp::halfband::roundtrip_latency_base_samples`, 24 base samples at the
// fixed 4×), reported to the host via `engine::LATENCY_SAMPLES` (the CLAP latency
// extension). The engine's `SpanDelay` holds the dry/bypass path at the same
// delay, so the group delay is constant regardless of FX state — a single static
// report is correct (CLAP only lets the reported latency move across an `activate`
// boundary, which would force a restart and an audible dropout).

/// Decode the `filter-*` CLAP params out of any [`ParamView`] into the
/// engine-native [`FilterParams`], without building a whole [`EngineParams`]
/// snapshot. Used both by `EngineParams::snapshot_from` (audio thread, per
/// control block) and the CLAP latency extension (main thread, on every host
/// latency query and per-block change check) so the param decode
/// lives in exactly one place.
pub fn filter_params_of<P: ParamView>(p: &P) -> FilterParams {
    let fb = PATCH_BASE + OFF_FILTER;
    FilterParams {
        enable: p.get(fb) >= 0.5,
        cutoff_hz: p.get(fb + 1),
        resonance: p.get(fb + 2),
        mode: filter_mode_from(p.get(fb + 3).round() as i32),
        slope: filter_slope_from(p.get(fb + 4).round() as i32),
        drive: p.get(fb + 5),
        // Oversampling is fixed at 4×: the filter shares one 4× span with the
        // dynamics FX. The `FilterParams.oversample` field lets kernel tests
        // drive the ladder at other factors.
        oversample: 4,
        keytrack: p.get(fb + 6).clamp(0.0, 1.0),
        // `filter-cutoff-tuned` (fb + 7) is UI-only — the stored cutoff is Hz
        // regardless, so the engine never reads it.
    }
}

/// Decode the `filter-mode` enum index into a [`FilterMode`].
fn filter_mode_from(idx: i32) -> FilterMode {
    match idx {
        1 => FilterMode::Hp,
        2 => FilterMode::Bp,
        3 => FilterMode::Notch,
        _ => FilterMode::Lp,
    }
}

/// Decode the `filter-slope` enum index into a [`FilterSlope`].
fn filter_slope_from(idx: i32) -> FilterSlope {
    match idx {
        0 => FilterSlope::Pole2,
        _ => FilterSlope::Pole4,
    }
}

/// Cutoff in Hz at / below which the HP stage is bypassed (its 20 Hz floor =
/// "off", transparent).
pub const HP_OFF_HZ: f32 = 20.0;

/// Engine-native shape of the static high-pass stage (`hp-cutoff`). A one-pole
/// tone-shaping filter ahead of the musical filter — deliberately *not* a
/// mod-matrix dest (fixed tone shaping), so a bare cutoff is all it carries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HpParams {
    /// Base cutoff in Hz. `<= `[`HP_OFF_HZ`] ⇒ the engine bypasses the stage.
    pub cutoff_hz: f32,
}

impl Default for HpParams {
    fn default() -> Self {
        Self {
            cutoff_hz: HP_OFF_HZ,
        }
    }
}

impl HpParams {
    /// True when the stage is engaged (cutoff lifted off its floor).
    #[inline]
    pub fn active(&self) -> bool {
        self.cutoff_hz > HP_OFF_HZ
    }
}

/// Decode the `hp-cutoff` CLAP param out of any [`ParamView`].
pub fn hp_params_of<P: ParamView>(p: &P) -> HpParams {
    HpParams {
        cutoff_hz: p.get(PATCH_BASE + OFF_HP),
    }
}

/// Mirror of [`SharedParams`] in engine-native shapes. Refreshed once per
/// control block by [`Self::snapshot_from`]. The engine never touches the
/// atomic store on the audio thread — only this struct.
#[derive(Clone, Copy, Debug)]
pub struct EngineParams {
    pub patch: Patch,
    pub mod_params: PatchModParams,
    pub alloc: AllocParams,
    pub delay: StereoDelayParams,
    pub reverb: FdnReverbParams,
    pub phaser: PhaserParams,
    pub dynamics: DynamicsParams,
    pub master: MasterParams,
    pub filter: FilterParams,
    pub hp: HpParams,
    /// CLAP-automatable matrix slot depths.
    pub mtx_depths: [f32; N_CLAP_DEPTH_SLOTS],
    /// Mod-matrix slot topology + depth, fanned out from the param store at
    /// snapshot time. The engine's [`crate::matrix::MatrixTable`] is rebuilt
    /// from this array each block in
    /// [`crate::engine::Engine::apply_block_params`] — that's the only path
    /// matrix UI / preset edits reach the audio renderer.
    pub matrix_rows: [MatrixRowRaw; N_MATRIX_SLOTS],
}

impl Default for EngineParams {
    fn default() -> Self {
        Self::from_shared(&SharedParams::new())
    }
}

impl EngineParams {
    /// Build a fresh snapshot from `s`. Equivalent to `Default::default()`
    /// followed by `snapshot_from(s)` but skips one redundant
    /// initialise-then-overwrite pass.
    pub fn from_shared(s: &SharedParams) -> Self {
        Self::from_params(s)
    }

    /// As [`Self::from_shared`] but for any [`ParamView`] — used by
    /// `vxn2-clap::local::LocalParams::write_to` to push its mirror into the
    /// engine without round-tripping through the atomic store.
    pub fn from_params<P: ParamView>(p: &P) -> Self {
        let mut e = Self {
            patch: Patch::default(),
            mod_params: PatchModParams::default(),
            alloc: AllocParams::default(),
            delay: StereoDelayParams::default(),
            reverb: FdnReverbParams::default(),
            phaser: PhaserParams::default(),
            dynamics: DynamicsParams::default(),
            master: MasterParams::default(),
            filter: FilterParams::default(),
            hp: HpParams::default(),
            mtx_depths: [0.0; N_CLAP_DEPTH_SLOTS],
            matrix_rows: [MatrixRowRaw::default(); N_MATRIX_SLOTS],
        };
        e.snapshot_from(p);
        e
    }

    /// Read every CLAP id out of `shared` and fan it into the engine fields.
    /// No allocation; no per-id branching beyond what the section readers
    /// need (enum decode, clamp). Call once per control block.
    pub fn snapshot_from<P: ParamView>(&mut self, shared: &P) {
        self.patch = read_patch(shared);

        // Matrix CLAP-automatable depths.
        for s in 0..N_CLAP_DEPTH_SLOTS {
            self.mtx_depths[s] = shared.get(OFF_MTX + s);
        }

        // Mod-matrix topology. The engine rebuilds its `MatrixTable` from
        // these each block — they're the only path UI / preset matrix edits
        // reach the audio renderer.
        debug_assert_eq!(
            N_MATRIX_SLOTS, N_MATRIX_RUNTIME_SLOTS,
            "shared / matrix slot counts diverged",
        );
        for s in 0..N_MATRIX_SLOTS {
            self.matrix_rows[s] = shared.matrix_row_raw(s);
        }

        // Patch-level block.
        let pb = PATCH_BASE;

        // Sync subdivision is derived from the rate / time fader's own
        // position (the fader *is* the selector) via the same helper the
        // display uses — so dragging the slider while sync is on walks the
        // subdivisions. `sync_index` is unused while sync is off.
        self.mod_params.lfo1 = Lfo1Params {
            shape: lfo_shape_from(shared.get(pb + OFF_LFO1) as i32),
            rate_hz: shared.get(pb + OFF_LFO1 + 1),
            sync: shared.get(pb + OFF_LFO1 + 2) >= 0.5,
            sync_index: crate::sync::sync_index_for(
                pb + OFF_LFO1 + 1,
                shared.get(pb + OFF_LFO1 + 1),
            ),
        };

        self.delay = StereoDelayParams {
            on: shared.get(pb + OFF_DELAY) >= 0.5,
            time_ms: shared.get(pb + OFF_DELAY + 1),
            sync: shared.get(pb + OFF_DELAY + 2) >= 0.5,
            sync_index: crate::sync::sync_index_for(
                pb + OFF_DELAY + 1,
                shared.get(pb + OFF_DELAY + 1),
            ),
            feedback: shared.get(pb + OFF_DELAY + 3),
            mix: shared.get(pb + OFF_DELAY + 4),
            pingpong: shared.get(pb + OFF_DELAY + 5) >= 0.5,
        };

        self.reverb = FdnReverbParams {
            on: shared.get(pb + OFF_REVERB) >= 0.5,
            size: shared.get(pb + OFF_REVERB + 1),
            decay_secs: shared.get(pb + OFF_REVERB + 2),
            damp: shared.get(pb + OFF_REVERB + 3),
            mix: shared.get(pb + OFF_REVERB + 4),
        };

        // Phaser — host-automation only. `set_params` re-clamps; the struct
        // just carries the snapshot.
        self.phaser = PhaserParams {
            on: shared.get(pb + OFF_PHASER) >= 0.5,
            rate_hz: shared.get(pb + OFF_PHASER + 1),
            depth: shared.get(pb + OFF_PHASER + 2),
            feedback: shared.get(pb + OFF_PHASER + 3),
            mix: shared.get(pb + OFF_PHASER + 4),
        };

        // Dynamics — host-automation only. `set_from` re-clamps; the struct
        // just carries the snapshot.
        self.dynamics = DynamicsParams {
            on: shared.get(pb + OFF_DYNAMICS) >= 0.5,
            threshold_db: shared.get(pb + OFF_DYNAMICS + 1),
            ratio: shared.get(pb + OFF_DYNAMICS + 2),
            attack_ms: shared.get(pb + OFF_DYNAMICS + 3),
            release_ms: shared.get(pb + OFF_DYNAMICS + 4),
            makeup_db: shared.get(pb + OFF_DYNAMICS + 5),
            drive_db: shared.get(pb + OFF_DYNAMICS + 6),
            mix: shared.get(pb + OFF_DYNAMICS + 7),
        };

        self.master = MasterParams {
            tune_cents: shared.get(pb + OFF_MASTER),
            volume_db: shared.get(pb + OFF_MASTER + 1),
            // `limiter-on` is appended at the very end of the flat space (past
            // the Filter section) for blob-prefix stability, not adjacent to
            // the other master ids.
            limiter_on: shared.get(pb + OFF_LIMITER) >= 0.5,
        };

        // Filter section (ADR 0004). Decoded by `filter_params_of` so the same
        // path feeds the audio render and the CLAP latency report.
        self.filter = filter_params_of(shared);

        // Static high-pass stage — a single cutoff, applied per stack ahead of
        // the musical filter, at base rate (never oversampled).
        self.hp = hp_params_of(shared);

        // Master tune bakes into the patch's per-op `base_phase_inc` at
        // note-on via `VoiceParams::master_tune_cents`.
        self.patch.voice.master_tune_cents = self.master.tune_cents;

        // Allocator reads the patch-level assignment block.
        self.alloc = read_assign(shared);
    }
}

fn read_patch<P: ParamView>(s: &P) -> Patch {
    let mut ops = [OpParams::default(); 6];
    for i in 0..6 {
        ops[i] = read_op(s, i * N_PER_OP);
    }
    let voice = VoiceParams {
        ops,
        algo: s.get(OFF_ALGO).clamp(1.0, 32.0) as u8,
        feedback: s.get(OFF_FEEDBACK).clamp(0.0, 7.0),
        master_tune_cents: 0.0, // overwritten with patch-level value post-snap
        lfo2: read_lfo2(s, OFF_LFO2),
        pitch_eg: read_peg(s, OFF_PEG),
        peg_depth: s.get(OFF_PEG + 8),
        mod_env: read_mod_env(s, OFF_MOD_ENV),
    };
    let stack = read_stack(s, OFF_STACK);
    Patch { stack, voice }
}

fn read_op<P: ParamView>(s: &P, base: usize) -> OpParams {
    let f = |off| s.get(base + off);
    let i = |off| s.get(base + off).round() as i32;
    // Op blocks are `N_PER_OP`-strided from id 0; recover the op index for the
    // non-CLAP KS-curve lookup (which isn't part of the flat value block).
    let op = base / N_PER_OP;
    OpParams {
        // `op{n}-ratio-mode` is the trailing enum in each op block (index 20):
        // 0 = Ratio, 1 = Fixed. Mirrors `RatioMode`'s discriminant order.
        ratio_mode: if i(20) == 1 {
            RatioMode::Fixed
        } else {
            RatioMode::Ratio
        },
        num: i(0).clamp(1, 32) as u8,
        denom: i(1).clamp(1, 8) as u8,
        fixed_hz: f(2),
        fine: i(3).clamp(-100, 100) as i8,
        detune: i(4).clamp(-100, 100) as i8,
        level: i(5).clamp(0, 99) as u8,
        vel_sens: i(6).clamp(0, 7) as u8,
        eg: vxn2_dsp::eg::EgParams {
            r: [
                i(7).clamp(0, 99) as u8,
                i(8).clamp(0, 99) as u8,
                i(9).clamp(0, 99) as u8,
                i(10).clamp(0, 99) as u8,
            ],
            l: [
                i(11).clamp(0, 99) as u8,
                i(12).clamp(0, 99) as u8,
                i(13).clamp(0, 99) as u8,
                i(14).clamp(0, 99) as u8,
            ],
        },
        // Per-op EG level curve: non-CLAP patch state read through
        // `ParamView::eg_curve`, persisted in the blob trailer + preset table.
        // Default `Exp` (log curve, ADR 0007).
        eg_curve: s.eg_curve(op),
        ks_break_pt: i(15).clamp(0, 127) as u8,
        ks_l_depth: i(16).clamp(0, 99) as u8,
        ks_r_depth: i(17).clamp(0, 99) as u8,
        // Per-side curve selectors: non-CLAP patch state read through
        // `ParamView::ks_curve`, persisted in the blob trailer + preset table.
        // Default left NegLin / right NegExp.
        ks_l_curve: s.ks_curve(op, 0),
        ks_r_curve: s.ks_curve(op, 1),
        ks_rate: i(18).clamp(0, 7) as u8,
        pan: f(19),
        // index 20 is `op{n}-ratio-mode` (read above); phase is the trailing
        // float at index 21.
        phase: f(21),
    }
}

fn read_lfo2<P: ParamView>(s: &P, base: usize) -> Lfo2Params {
    Lfo2Params {
        shape: lfo_shape_from(s.get(base) as i32),
        rate_hz: s.get(base + 1),
        delay_ms: s.get(base + 2),
        fade_ms: s.get(base + 3),
        sync: s.get(base + 4) >= 0.5,
        // Derived from the rate fader's position, same as LFO1 / delay — the
        // slider selects the subdivision while sync is on.
        sync_index: crate::sync::sync_index_for(base + 1, s.get(base + 1)),
    }
}

fn read_peg<P: ParamView>(s: &P, base: usize) -> PitchEgParams {
    let i = |o| s.get(base + o).round() as i32;
    PitchEgParams {
        r: [
            i(0).clamp(0, 99) as u8,
            i(1).clamp(0, 99) as u8,
            i(2).clamp(0, 99) as u8,
            i(3).clamp(0, 99) as u8,
        ],
        l: [
            i(4).clamp(-99, 99) as i8,
            i(5).clamp(-99, 99) as i8,
            i(6).clamp(-99, 99) as i8,
            i(7).clamp(-99, 99) as i8,
        ],
    }
}

fn read_mod_env<P: ParamView>(s: &P, base: usize) -> ModEnvParams {
    ModEnvParams {
        a_ms: s.get(base),
        d_ms: s.get(base + 1),
        s: s.get(base + 2),
        r_ms: s.get(base + 3),
        shape: match s.get(base + 4).round() as i32 {
            0 => AdsrShape::Lin,
            _ => AdsrShape::Exp,
        },
    }
}

fn read_stack<P: ParamView>(s: &P, base: usize) -> StackParams {
    StackParams {
        density: s.get(base).clamp(1.0, 8.0) as u8,
        detune_cents_max: s.get(base + 1),
        spread: s.get(base + 2),
        phase: s.get(base + 3),
        distrib: match s.get(base + 4).round() as i32 {
            0 => StackDistrib::Linear,
            1 => StackDistrib::Geometric,
            _ => StackDistrib::Random,
        },
    }
}

fn read_assign<P: ParamView>(s: &P) -> AllocParams {
    let off = OFF_ASSIGN;
    AllocParams {
        assign_mode: match s.get(off).round() as i32 {
            0 => AssignMode::Poly,
            _ => AssignMode::Solo,
        },
        legato: s.get(off + 1) >= 0.5,
        glide_time_ms: s.get(off + 2),
    }
}

#[inline]
fn lfo_shape_from(i: i32) -> LfoShape {
    match i {
        0 => LfoShape::Sine,
        1 => LfoShape::Triangle,
        2 => LfoShape::SawUp,
        3 => LfoShape::SawDown,
        4 => LfoShape::Pulse,
        _ => LfoShape::SampleHold,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::id_of;

    #[test]
    fn shared_params_seed_from_default_patch() {
        // Every slot starts at the illustrative-patch value, not the bare
        // descriptor default.
        let s = SharedParams::new();
        let expected = crate::default_patch::default_param_values();
        for i in 0..TOTAL_PARAMS {
            assert_eq!(s.get(i), expected[i], "slot {} ({})", i, PARAMS[i].id);
        }
        // Feedback 6.0 comes from the default patch, not the descriptor
        // default (0.0) — proves seeding goes through default_patch.
        assert!((s.get(crate::params::id_of("feedback").unwrap()) - 6.0).abs() < 1e-6);
        assert!(
            (s.get(crate::params::id_of("master-volume").unwrap()) - (-6.0)).abs() < 1e-6
        );
    }

    #[test]
    fn set_then_get_round_trips_with_clamp() {
        let s = SharedParams::new();
        let id = id_of("master-volume").unwrap();
        s.set(id, -3.0);
        assert_eq!(s.get(id), -3.0);
        s.set(id, 999.0);
        assert_eq!(s.get(id), 6.0);
        s.set(id, -999.0);
        assert_eq!(s.get(id), -60.0);
    }

    #[test]
    fn normalised_round_trip_through_store() {
        let s = SharedParams::new();
        let id = id_of("reverb-decay").unwrap();
        s.set_normalised(id, 0.5);
        let v = s.get(id);
        // Exp { mid: 2.0 } → 0.5 lands near 2.0 s.
        assert!((v - 2.0).abs() < 0.1, "got {v}");
    }

    #[test]
    fn snapshot_uses_defaults_correctly() {
        let s = SharedParams::new();
        let e = EngineParams::from_shared(&s);
        assert!((e.master.volume_db - (-6.0)).abs() < 1e-6);
        assert!((e.reverb.size - 0.55).abs() < 1e-6);
        assert!((e.delay.time_ms - 375.0).abs() < 1e-6);
        assert_eq!(e.patch.voice.master_tune_cents, 0.0);
    }

    #[test]
    fn snapshot_master_tune_cascades_into_voice() {
        let s = SharedParams::new();
        s.set(id_of("master-tune").unwrap(), 25.0);
        let e = EngineParams::from_shared(&s);
        assert_eq!(e.patch.voice.master_tune_cents, 25.0);
    }

    #[test]
    fn snapshot_resolves_per_op_block() {
        let s = SharedParams::new();
        s.set(id_of("op3-num").unwrap(), 7.0);
        s.set(id_of("op3-denom").unwrap(), 2.0);
        s.set(id_of("op6-level").unwrap(), 42.0);
        let e = EngineParams::from_shared(&s);
        assert_eq!(e.patch.voice.ops[2].num, 7);
        assert_eq!(e.patch.voice.ops[2].denom, 2);
        assert_eq!(e.patch.voice.ops[5].level, 42);
    }

    #[test]
    fn snapshot_resolves_mtx_depths() {
        let s = SharedParams::new();
        s.set(id_of("mtx1-depth").unwrap(), 0.4);
        s.set(id_of("mtx8-depth").unwrap(), -0.7);
        let e = EngineParams::from_shared(&s);
        assert!((e.mtx_depths[0] - 0.4).abs() < 1e-6);
        assert!((e.mtx_depths[7] - (-0.7)).abs() < 1e-6);
    }

    #[test]
    fn snapshot_bytes_round_trip_is_bit_identical() {
        let src = SharedParams::new();
        // Touch a spread of slots so we exercise per-id positions.
        for (name, v) in [
            ("op1-num", 3.0_f32),
            ("op6-level", 88.0),
            ("op4-pan", -0.7),
            ("master-volume", -3.0),
            ("reverb-decay", 4.5),
            ("delay-time", 250.0),
        ] {
            let id = id_of(name).unwrap();
            src.set(id, v);
        }
        // Stuff a NaN bit pattern into a slot we don't care about — load_bytes
        // must preserve it bit-for-bit so the round-trip is unambiguous.
        let nan_id = id_of("op2-fine").unwrap();
        let pattern = 0x7fc0_dead_u32;
        src.values[nan_id].store(pattern, Ordering::Relaxed);

        let bytes = src.snapshot_bytes();
        assert_eq!(
            bytes.len(),
            BLOB_HEADER_LEN
                + TOTAL_PARAMS * 4
                + BLOB_MATRIX_LEN
                + BLOB_KS_CURVE_LEN
                + BLOB_EG_CURVE_LEN
        );
        assert_eq!(&bytes[0..4], BLOB_MAGIC);
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), BLOB_VERSION);
        assert_eq!(
            u16::from_le_bytes([bytes[6], bytes[7]]) as usize,
            TOTAL_PARAMS
        );

        let dst = SharedParams::new();
        dst.load_bytes(&bytes).unwrap();
        for i in 0..TOTAL_PARAMS {
            assert_eq!(
                src.values[i].load(Ordering::Relaxed),
                dst.values[i].load(Ordering::Relaxed),
                "slot {i} ({}) differs after round-trip",
                PARAMS[i].id
            );
        }
        assert_eq!(dst.values[nan_id].load(Ordering::Relaxed), pattern);
    }

    /// The secondary scale source rides the packed `matrix_meta` word's free
    /// low-byte bits, so it survives a state-blob round-trip unchanged.
    #[test]
    fn matrix_scale_src_survives_blob_round_trip() {
        let src = SharedParams::new();
        src.set_matrix_row_raw(
            2,
            MatrixRowRaw {
                source: 2, // lfo2
                dest: 17, // global-pitch
                curve: 0,
                active: true,
                depth: 0.5,
                scale_src: 5, // mod-wheel
            },
        );
        let bytes = src.snapshot_bytes();
        let dst = SharedParams::new();
        dst.load_bytes(&bytes).unwrap();
        let row = dst.matrix_row_raw(2);
        assert_eq!(row.scale_src, 5);
        assert_eq!(row.source, 2);
        assert!(row.active);
    }

    /// A blob with the scale-src bits clear (only `active` set in the low byte)
    /// decodes to `scale_src = 0` (unscaled). Simulate by clearing the scale
    /// bits in a snapshot's matrix trailer and confirming the row still loads.
    #[test]
    fn pre_e033_blob_decodes_scale_src_none() {
        let src = SharedParams::new();
        src.set_matrix_row_raw(
            0,
            MatrixRowRaw {
                source: 1, // lfo1
                dest: 17,
                curve: 0,
                active: true,
                depth: 0.3,
                scale_src: 7, // stripped below to simulate the bits-clear case
            },
        );
        let mut bytes = src.snapshot_bytes();
        // Matrix trailer starts right after the value block; slot 0's packed
        // u32 is first. Clear bits 1..=7 of its low byte (keep bit 0 = active).
        let off = BLOB_HEADER_LEN + TOTAL_PARAMS * 4;
        bytes[off] &= 0x01;
        let dst = SharedParams::new();
        dst.load_bytes(&bytes).unwrap();
        let row = dst.matrix_row_raw(0);
        assert_eq!(row.scale_src, 0, "bits clear → unscaled");
        assert!(row.active, "active bit preserved");
        assert_eq!(row.source, 1);
    }

    #[test]
    fn load_bytes_rejects_short_buffer() {
        let s = SharedParams::new();
        let err = s.load_bytes(&[0u8; 4]).unwrap_err();
        let expected = BLOB_HEADER_LEN
            + TOTAL_PARAMS * 4
            + BLOB_MATRIX_LEN
            + BLOB_KS_CURVE_LEN
            + BLOB_EG_CURVE_LEN;
        assert_eq!(err, ParamLoadError::LengthMismatch { expected, got: 4 });
    }

    #[test]
    fn load_bytes_rejects_bad_magic() {
        let s = SharedParams::new();
        let mut bytes = s.snapshot_bytes();
        bytes[0] = b'X';
        assert_eq!(s.load_bytes(&bytes).unwrap_err(), ParamLoadError::MagicMismatch);
    }

    #[test]
    fn load_bytes_rejects_future_version() {
        let s = SharedParams::new();
        let mut bytes = s.snapshot_bytes();
        let future = BLOB_VERSION + 1;
        bytes[4..6].copy_from_slice(&future.to_le_bytes());
        assert_eq!(
            s.load_bytes(&bytes).unwrap_err(),
            ParamLoadError::UnsupportedVersion(future)
        );
    }

    /// Any *older* version is rejected too, not just future ones. A blob
    /// stamped with version 15 does not load — re-save from the live build.
    #[test]
    fn load_bytes_rejects_old_version() {
        let s = SharedParams::new();
        let mut bytes = s.snapshot_bytes();
        bytes[4..6].copy_from_slice(&15u16.to_le_bytes());
        assert_eq!(
            s.load_bytes(&bytes).unwrap_err(),
            ParamLoadError::UnsupportedVersion(15)
        );
    }

    #[test]
    fn load_bytes_rejects_wrong_count() {
        let s = SharedParams::new();
        let mut bytes = s.snapshot_bytes();
        let wrong = (TOTAL_PARAMS as u16) - 1;
        bytes[6..8].copy_from_slice(&wrong.to_le_bytes());
        assert_eq!(
            s.load_bytes(&bytes).unwrap_err(),
            ParamLoadError::CountMismatch {
                expected: TOTAL_PARAMS as u16,
                got: wrong,
            }
        );
    }

    #[test]
    fn load_bytes_rejects_truncated_payload() {
        let s = SharedParams::new();
        let mut bytes = s.snapshot_bytes();
        let full_len = bytes.len();
        bytes.truncate(full_len - 4);
        assert_eq!(
            s.load_bytes(&bytes).unwrap_err(),
            ParamLoadError::LengthMismatch {
                expected: full_len,
                got: full_len - 4,
            }
        );
    }

    /// Matrix topology (source / dest / curve / active) round-trips through a
    /// blob save/load just like CLAP-automatable params.
    #[test]
    fn snapshot_bytes_round_trips_matrix_meta() {
        let src = SharedParams::new();
        // Stomp slot 0 (default-seeded) with a fresh row plus a non-CLAP slot
        // (9) whose depth rides matrix_extra_depth.
        src.set_matrix_row_raw(
            0,
            MatrixRowRaw {
                source: 4,    // ModEnv
                dest: 2,      // Op1Level
                curve: 1,     // Exp
                active: true,
                depth: 0.42,
                scale_src: 0,
            },
        );
        src.set_matrix_row_raw(
            9,
            MatrixRowRaw {
                source: 5,    // ModWheel
                dest: 21,     // Lfo2Rate
                curve: 0,
                active: true,
                depth: -0.6,
                scale_src: 0,
            },
        );

        let bytes = src.snapshot_bytes();
        let dst = SharedParams::new();
        dst.load_bytes(&bytes).unwrap();

        let r0 = dst.matrix_row_raw(0);
        assert_eq!(r0.source, 4);
        assert_eq!(r0.dest, 2);
        assert_eq!(r0.curve, 1);
        assert!(r0.active);
        assert!((r0.depth - 0.42).abs() < 1e-6);

        let r9 = dst.matrix_row_raw(9);
        assert_eq!(r9.source, 5);
        assert_eq!(r9.dest, 21);
        assert!(r9.active);
        assert!((r9.depth - (-0.6)).abs() < 1e-6);
    }

    #[test]
    fn ks_curve_default_is_legacy_frozen_shapes() {
        let s = SharedParams::new();
        for op in 0..N_OPS {
            assert_eq!(s.ks_curve_raw(op, 0), 0, "op{op} left default = NegLin");
            assert_eq!(s.ks_curve_raw(op, 1), 2, "op{op} right default = NegExp");
            assert_eq!(
                ParamView::ks_curve(&s, op, 0),
                vxn2_dsp::ks::KsCurve::NegLin
            );
            assert_eq!(
                ParamView::ks_curve(&s, op, 1),
                vxn2_dsp::ks::KsCurve::NegExp
            );
        }
    }

    #[test]
    fn set_ks_curve_raw_is_independent_per_field() {
        let s = SharedParams::new();
        // Flip every field to a distinct value and confirm no field stomps a
        // neighbour (2-bit packing correctness).
        for op in 0..N_OPS {
            s.set_ks_curve_raw(op, 0, (op as u8) % 4);
            s.set_ks_curve_raw(op, 1, ((op as u8) + 1) % 4);
        }
        for op in 0..N_OPS {
            assert_eq!(s.ks_curve_raw(op, 0), (op as u8) % 4);
            assert_eq!(s.ks_curve_raw(op, 1), ((op as u8) + 1) % 4);
        }
    }

    #[test]
    fn snapshot_bytes_round_trips_ks_curves() {
        let src = SharedParams::new();
        src.set_ks_curve_raw(0, 0, 3); // PosExp
        src.set_ks_curve_raw(0, 1, 1); // PosLin
        src.set_ks_curve_raw(5, 1, 3); // PosExp

        let bytes = src.snapshot_bytes();
        let dst = SharedParams::new();
        dst.load_bytes(&bytes).unwrap();

        assert_eq!(dst.ks_curve_raw(0, 0), 3);
        assert_eq!(dst.ks_curve_raw(0, 1), 1);
        assert_eq!(dst.ks_curve_raw(5, 1), 3);
        // Untouched fields keep their default.
        assert_eq!(dst.ks_curve_raw(3, 0), 0);
        assert_eq!(dst.ks_curve_raw(3, 1), 2);
    }

    #[test]
    fn eg_curve_default_is_exp() {
        let s = SharedParams::new();
        for op in 0..N_OPS {
            assert_eq!(s.eg_curve_raw(op), 0, "op{op} default = Exp");
            assert_eq!(ParamView::eg_curve(&s, op), EgCurve::Exp);
        }
    }

    #[test]
    fn set_eg_curve_raw_is_independent_per_op() {
        let s = SharedParams::new();
        // Flip alternate ops to Lin and confirm no op stomps a neighbour
        // (1-bit packing correctness).
        for op in 0..N_OPS {
            s.set_eg_curve_raw(op, (op as u8) % 2);
        }
        for op in 0..N_OPS {
            assert_eq!(s.eg_curve_raw(op), (op as u8) % 2);
        }
    }

    #[test]
    fn snapshot_bytes_round_trips_eg_curves() {
        let src = SharedParams::new();
        src.set_eg_curve_raw(0, 1); // Lin
        src.set_eg_curve_raw(5, 1); // Lin

        let bytes = src.snapshot_bytes();
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), BLOB_VERSION);
        let dst = SharedParams::new();
        dst.load_bytes(&bytes).unwrap();

        assert_eq!(dst.eg_curve_raw(0), 1);
        assert_eq!(dst.eg_curve_raw(5), 1);
        // Untouched ops keep the default (Exp).
        assert_eq!(dst.eg_curve_raw(3), 0);
    }

    /// A live blob round-trips every `dyn-*` slot byte-for-byte — the
    /// engine-side decode is the only consumer between save and load.
    #[test]
    fn snapshot_round_trips_dynamics_params() {
        let src = SharedParams::new();
        let cases: &[(&str, f32)] = &[
            ("dyn-on", 1.0),
            ("dyn-threshold", -24.0),
            ("dyn-ratio", 8.0),
            ("dyn-attack", 3.5),
            ("dyn-release", 150.0),
            ("dyn-makeup", 6.0),
            ("dyn-drive", 18.0),
            ("dyn-mix", 0.75),
        ];
        for &(name, v) in cases {
            src.set(id_of(name).unwrap(), v);
        }

        let bytes = src.snapshot_bytes();
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), BLOB_VERSION);

        let dst = SharedParams::new();
        dst.load_bytes(&bytes).unwrap();
        for &(name, v) in cases {
            assert_eq!(dst.get(id_of(name).unwrap()), v, "{name}");
        }

        // And the engine decode lands the same values.
        let ep = EngineParams::from_shared(&dst);
        assert!(ep.dynamics.on);
        assert_eq!(ep.dynamics.threshold_db, -24.0);
        assert_eq!(ep.dynamics.ratio, 8.0);
        assert_eq!(ep.dynamics.attack_ms, 3.5);
        assert_eq!(ep.dynamics.release_ms, 150.0);
        assert_eq!(ep.dynamics.makeup_db, 6.0);
        assert_eq!(ep.dynamics.drive_db, 18.0);
        assert_eq!(ep.dynamics.mix, 0.75);
    }

    /// A stack-pitch route (dest in the appended 30..=35 band) saves and
    /// reloads through the snapshot blob unchanged.
    #[test]
    fn snapshot_round_trips_stack_pitch_route() {
        use crate::matrix::DestId;
        let dest = DestId::Op3StackPitch as u8; // 32
        let src = SharedParams::new();
        src.set_matrix_row_raw(
            0,
            MatrixRowRaw {
                source: 1, // Lfo1
                dest,
                curve: 0,
                active: true,
                depth: 0.5,
                scale_src: 0,
            },
        );
        let bytes = src.snapshot_bytes();
        // The widened dest space does not change the param count or byte layout.
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), BLOB_VERSION);
        let dst = SharedParams::new();
        dst.load_bytes(&bytes).unwrap();
        let r0 = dst.matrix_row_raw(0);
        assert_eq!(r0.dest, dest);
        assert_eq!(DestId::from_u8(r0.dest), DestId::Op3StackPitch);
        assert!(r0.active);
        assert!((r0.depth - 0.5).abs() < 1e-6);
    }

    #[test]
    fn snapshot_bytes_is_stable_across_saves() {
        let s = SharedParams::new();
        s.set(id_of("master-volume").unwrap(), -3.0);
        s.set(id_of("reverb-decay").unwrap(), 4.5);
        let a = s.snapshot_bytes();
        let b = s.snapshot_bytes();
        assert_eq!(a, b);
    }

    // Dirty bitset (ADR 0003).

    fn drain_total(words: &[u64; N_DIRTY_VALUE_WORDS]) -> u32 {
        words.iter().map(|w| w.count_ones()).sum()
    }

    /// Fresh `SharedParams` carries an all-ones seed (full re-broadcast
    /// on the first tick).
    #[test]
    fn new_seeds_all_dirty_bits_for_full_broadcast() {
        let s = SharedParams::new();
        let values = s.take_dirty_values();
        assert_eq!(drain_total(&values), TOTAL_PARAMS as u32);
        // Last word only carries the in-range bits.
        let last_word = values[N_DIRTY_VALUE_WORDS - 1];
        let expected_last = dirty_values_full_word(N_DIRTY_VALUE_WORDS - 1);
        assert_eq!(last_word, expected_last);
        assert_eq!(s.take_dirty_matrix(), DIRTY_MATRIX_ALL);
    }

    /// Writing one id sets exactly one value bit; other bits stay zero.
    #[test]
    fn set_flips_exactly_one_value_bit() {
        let s = SharedParams::new();
        // Drain the seed so we start from a clean bitset.
        let _ = s.take_dirty_values();
        let _ = s.take_dirty_matrix();

        let id = id_of("master-volume").unwrap();
        s.set(id, -3.0);
        let bits = s.take_dirty_values();
        assert_eq!(drain_total(&bits), 1);
        assert_eq!(bits[id / 64], 1u64 << (id % 64));
        // Matrix bitset untouched by a plain value write.
        assert_eq!(s.take_dirty_matrix(), 0);
    }

    /// Out-of-range `set` does not flip any bit (preserves the original
    /// short-circuit and avoids index-out-of-bounds in the bitset).
    #[test]
    fn set_out_of_range_flips_no_bit() {
        let s = SharedParams::new();
        let _ = s.take_dirty_values();
        s.set(TOTAL_PARAMS + 5, 0.0);
        let bits = s.take_dirty_values();
        assert_eq!(drain_total(&bits), 0);
    }

    /// `set_normalised` routes through `set`, so the bit flips too.
    #[test]
    fn set_normalised_flips_value_bit() {
        let s = SharedParams::new();
        let _ = s.take_dirty_values();
        let id = id_of("reverb-decay").unwrap();
        s.set_normalised(id, 0.5);
        let bits = s.take_dirty_values();
        assert_eq!(drain_total(&bits), 1);
        assert!(bits[id / 64] & (1u64 << (id % 64)) != 0);
    }

    /// Writing one matrix slot sets exactly one matrix bit; other slot
    /// bits stay zero.
    #[test]
    fn set_matrix_row_raw_flips_exactly_one_matrix_bit_for_extra_slot() {
        let s = SharedParams::new();
        let _ = s.take_dirty_values();
        let _ = s.take_dirty_matrix();
        s.set_matrix_row_raw(
            9,
            MatrixRowRaw { source: 4, dest: 2, curve: 0, active: true, depth: 0.3, scale_src: 0 },
        );
        // Slot 9 lives past N_MATRIX_CLAP_SLOTS; depth doesn't touch a
        // CLAP id, so the value bitset stays empty.
        assert_eq!(drain_total(&s.take_dirty_values()), 0);
        assert_eq!(s.take_dirty_matrix(), 1u64 << 9);
    }

    /// For a CLAP-automatable slot (1-8), `set_matrix_row_raw` flips
    /// both the matrix slot bit AND the matching depth value bit so the
    /// fader follows through the standard primitive bind.
    #[test]
    fn set_matrix_row_raw_clap_slot_flips_both_matrix_and_value_bits() {
        let s = SharedParams::new();
        let _ = s.take_dirty_values();
        let _ = s.take_dirty_matrix();
        s.set_matrix_row_raw(
            0,
            MatrixRowRaw { source: 4, dest: 2, curve: 1, active: true, depth: 0.5, scale_src: 0 },
        );
        assert_eq!(s.take_dirty_matrix(), 1u64 << 0);
        let bits = s.take_dirty_values();
        assert_eq!(drain_total(&bits), 1);
        let depth_id = OFF_MTX + 0;
        assert!(bits[depth_id / 64] & (1u64 << (depth_id % 64)) != 0);
    }

    /// `take_dirty_*` clears the bits — a second drain with no
    /// intervening writes returns all zeros.
    #[test]
    fn take_dirty_clears_the_bits() {
        let s = SharedParams::new();
        // First drain pops the all-ones seed.
        let first_values = s.take_dirty_values();
        assert!(drain_total(&first_values) > 0);
        let first_matrix = s.take_dirty_matrix();
        assert_ne!(first_matrix, 0);
        // Second drain with no intervening writes — both bitsets empty.
        let second_values = s.take_dirty_values();
        assert_eq!(drain_total(&second_values), 0);
        assert_eq!(s.take_dirty_matrix(), 0);
    }

    /// `load_bytes` round-trip leaves both bitsets non-zero — state
    /// load is observable to the main-thread tick without any bespoke
    /// push from the caller.
    #[test]
    fn load_bytes_marks_full_table_dirty() {
        let src = SharedParams::new();
        src.set(id_of("master-volume").unwrap(), -3.0);
        src.set_matrix_row_raw(
            0,
            MatrixRowRaw { source: 4, dest: 2, curve: 1, active: true, depth: 0.42, scale_src: 0 },
        );
        let bytes = src.snapshot_bytes();

        let dst = SharedParams::new();
        let _ = dst.take_dirty_values();
        let _ = dst.take_dirty_matrix();
        dst.load_bytes(&bytes).unwrap();

        let values = dst.take_dirty_values();
        assert_eq!(drain_total(&values), TOTAL_PARAMS as u32);
        assert_eq!(dst.take_dirty_matrix(), DIRTY_MATRIX_ALL);
    }

    /// `reset_to_defaults` flips every dirty bit so the next tick
    /// re-broadcasts the full table.
    #[test]
    fn reset_to_defaults_marks_all_dirty() {
        let s = SharedParams::new();
        let _ = s.take_dirty_values();
        let _ = s.take_dirty_matrix();
        s.reset_to_defaults();
        assert_eq!(drain_total(&s.take_dirty_values()), TOTAL_PARAMS as u32);
        assert_eq!(s.take_dirty_matrix(), DIRTY_MATRIX_ALL);
    }

    #[test]
    fn snapshot_uses_assignment_block() {
        let s = SharedParams::new();
        s.set(id_of("assign-mode").unwrap(), 1.0);
        s.set(id_of("legato").unwrap(), 1.0);
        s.set(id_of("glide-time").unwrap(), 200.0);
        let e = EngineParams::from_shared(&s);
        assert_eq!(e.alloc.assign_mode, AssignMode::Solo);
        assert!(e.alloc.legato);
        assert!((e.alloc.glide_time_ms - 200.0).abs() < 1e-5);
    }
}
