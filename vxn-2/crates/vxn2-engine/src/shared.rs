//! Lock-free parameter store + audio-thread snapshot (ticket 0012).
//!
//! Same shape as VXN1's `vxn-engine::SharedParams`: a flat
//! `[AtomicU32; TOTAL_PARAMS]` of plain `f32` values stored as bits, indexed
//! by stable CLAP id. The store is the single source of truth between the
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

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use vxn2_dsp::delay::StereoDelayParams;
use vxn2_dsp::envelope::{AdsrShape, ModEnvParams, PitchEgParams};
use vxn2_dsp::lfo::{Lfo1Params, Lfo2Params, LfoShape};
use vxn2_dsp::op::{OpParams, RatioMode};
use vxn2_dsp::reverb::FdnReverbParams;
use vxn2_dsp::stack::{StackDistrib, StackParams};
use vxn2_dsp::voice::VoiceParams;

use crate::alloc::{AllocParams, AssignMode};
use crate::master::MasterParams;
use crate::matrix::{N_CLAP_DEPTH_SLOTS, N_SLOTS as N_MATRIX_RUNTIME_SLOTS};
use crate::modulation::PatchModParams;
use crate::params::{
    N_PER_OP, OFF_ALGO, OFF_ASSIGN, OFF_DELAY, OFF_FEEDBACK, OFF_LFO1, OFF_LFO2, OFF_MASTER,
    OFF_MOD_ENV, OFF_MTX, OFF_PEG, OFF_REVERB, OFF_STACK, PARAMS, PATCH_BASE, TOTAL_PARAMS,
    core_desc_for_clap_id,
};

// ── Patch ───────────────────────────────────────────────────────────────────

/// A complete patch parameter set. Per [ADR 0002] dual-layer voicing is gone
/// — a patch is one stack + voice pair. The matrix slot table lives next to
/// the engine (one [`crate::matrix::MatrixTable`] per patch).
#[derive(Clone, Copy, Debug, Default)]
pub struct Patch {
    pub stack: StackParams,
    pub voice: VoiceParams,
}

// ── ParamModel trait surface ────────────────────────────────────────────────

/// Errors returned by [`ParamModel::load_bytes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamLoadError {
    /// First 4 bytes were not `b"VXN2"`.
    MagicMismatch,
    /// Version field exceeds the highest supported version (v1 today).
    UnsupportedVersion(u16),
    /// Header count differs from [`TOTAL_PARAMS`]. v1 demands exact match;
    /// future versions may relax this.
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
/// Highest blob version this build can read.
///
/// - v1: header + `f32` values for every CLAP id.
/// - v2: appends mod-matrix topology (16 × packed `u32` meta) and the
///   non-automatable slot depths (8 × `f32`). Older v1 blobs still load and
///   leave the matrix at default-patch topology.
/// - v3: collapses per-op Ratio + Detune mod-matrix dests into a single
///   per-op Pitch dest (was 4 dests per op, now 3). Older v2 blobs migrate
///   on load by rewriting the packed `dest` discriminant — both old Ratio
///   and old Detune routes map to the new Pitch dest.
pub const BLOB_VERSION: u16 = 3;
/// Header byte length: 4 magic + 2 version + 2 count.
pub const BLOB_HEADER_LEN: usize = 8;
/// Trailing matrix-meta byte length appended at v2:
/// `N_MATRIX_SLOTS * 4 + (N_MATRIX_SLOTS - N_MATRIX_CLAP_SLOTS) * 4`.
pub const BLOB_MATRIX_LEN: usize =
    N_MATRIX_SLOTS * 4 + (N_MATRIX_SLOTS - N_MATRIX_CLAP_SLOTS) * 4;

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
}

/// Main-thread parameter-model surface bound by the CLAP params extension
/// (ticket 0015) and the state extension (ticket 0017). The UI epic adds a
/// second implementation backing a view-side mirror with view-event emission;
/// both satisfy this trait so the CLAP shell stays swappable.
pub trait ParamModel: ParamView {
    fn total(&self) -> usize;
    fn get_normalised(&self, id: usize) -> f32;
    fn snapshot_bytes(&self) -> Vec<u8>;
    fn load_bytes(&self, bytes: &[u8]) -> Result<(), ParamLoadError>;
}

// ── SharedParams ────────────────────────────────────────────────────────────

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
/// and drained by the main-thread tick (ADR 0003 / epic E005).
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

/// Lock-free param store. Sized to [`TOTAL_PARAMS`] (180 in v1). Cheap to
/// share via `Arc` — every field is an atomic.
///
/// Beyond the CLAP-automatable `values` array the store also holds the
/// non-CLAP shared state the controller (ticket 0022) needs to read /
/// write: per-param gesture flags, matrix-row topology (source / dest /
/// curve / active), slot 9-16 depths.
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
    /// change channel (ADR 0003). Every `set` / `set_normalised` /
    /// `set_matrix_row_raw` write site flips the matching bit with
    /// `fetch_or(Release)`; the main-thread tick drains via
    /// `take_dirty_values` (`swap(Acquire)`). The Release/Acquire pair
    /// guarantees a reader that observes the bit sees the value the
    /// writer stored before flipping it.
    ///
    /// Seeded with every valid bit set so the first tick after open
    /// broadcasts the whole table — equivalent to the all-NaN
    /// `last_seen` seed in the prior polling pump.
    dirty_values: [AtomicU64; N_DIRTY_VALUE_WORDS],
    /// Dirty bitset for matrix-slot topology (one bit per slot). Any
    /// non-zero word triggers a whole-table `MatrixSnapshot` push on the
    /// next tick. Slot bits cover both meta drift and the slot-9-16
    /// depth side-table; slot 1-8 depth drift also rides
    /// [`dirty_values`] (its CLAP id lives in [`OFF_MTX`]).
    dirty_matrix: AtomicU64,
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
                ))
            }),
            matrix_extra_depth: std::array::from_fn(|s| {
                // Slots past the CLAP-automatable range take depth from this
                // array; the default matrix leaves them inert (zeroed).
                let slot_idx = s + N_MATRIX_CLAP_SLOTS;
                AtomicU32::new(default_matrix.slots[slot_idx].depth.to_bits())
            }),
            // Full-broadcast seed: first tick after open pushes every id
            // + a MatrixSnapshot, hydrating the editor with current
            // state without a bespoke push from the caller.
            dirty_values: std::array::from_fn(|w| AtomicU64::new(dirty_values_full_word(w))),
            dirty_matrix: AtomicU64::new(DIRTY_MATRIX_ALL),
        }
    }

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
            );
            self.matrix_meta[s].store(packed, Ordering::Relaxed);
        }
        for s in 0..(N_MATRIX_SLOTS - N_MATRIX_CLAP_SLOTS) {
            self.matrix_extra_depth[s].store(
                default_matrix.slots[s + N_MATRIX_CLAP_SLOTS].depth.to_bits(),
                Ordering::Relaxed,
            );
        }
        self.mark_all_dirty();
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
    }

    // ── Dirty bitset drain ──────────────────────────────────────────────────

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

    // ── Gesture flags ───────────────────────────────────────────────────────

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

    // ── Matrix-row storage ──────────────────────────────────────────────────

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
        let packed = pack_matrix_meta(row.source, row.dest, row.curve, row.active);
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
}

#[inline]
pub(crate) fn pack_matrix_meta(source: u8, dest: u8, curve: u8, active: bool) -> u32 {
    ((source as u32) << 24) | ((dest as u32) << 16) | ((curve as u32) << 8) | (active as u32)
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
    /// not the user-facing preset format (which lands with the preset epic
    /// and carries the matrix source/dest/curve slots the blob omits).
    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(BLOB_HEADER_LEN + TOTAL_PARAMS * 4 + BLOB_MATRIX_LEN);
        buf.extend_from_slice(BLOB_MAGIC);
        buf.extend_from_slice(&BLOB_VERSION.to_le_bytes());
        buf.extend_from_slice(&(TOTAL_PARAMS as u16).to_le_bytes());
        for i in 0..TOTAL_PARAMS {
            let bits = self.values[i].load(Ordering::Relaxed);
            buf.extend_from_slice(&bits.to_le_bytes());
        }
        // v2 trailer — matrix topology + non-automatable slot depths.
        for s in 0..N_MATRIX_SLOTS {
            let packed = self.matrix_meta[s].load(Ordering::Relaxed);
            buf.extend_from_slice(&packed.to_le_bytes());
        }
        for s in 0..(N_MATRIX_SLOTS - N_MATRIX_CLAP_SLOTS) {
            let bits = self.matrix_extra_depth[s].load(Ordering::Relaxed);
            buf.extend_from_slice(&bits.to_le_bytes());
        }
        buf
    }

    /// Inverse of [`snapshot_bytes`]. Validates magic / version / count /
    /// length, then writes value bits unmodified — no descriptor clamp — so
    /// a snapshot round-trip is bit-identical.
    fn load_bytes(&self, bytes: &[u8]) -> Result<(), ParamLoadError> {
        if bytes.len() < BLOB_HEADER_LEN {
            return Err(ParamLoadError::LengthMismatch {
                expected: BLOB_HEADER_LEN + TOTAL_PARAMS * 4,
                got: bytes.len(),
            });
        }
        if &bytes[0..4] != BLOB_MAGIC {
            return Err(ParamLoadError::MagicMismatch);
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version > BLOB_VERSION {
            return Err(ParamLoadError::UnsupportedVersion(version));
        }
        let count = u16::from_le_bytes([bytes[6], bytes[7]]);
        if count as usize != TOTAL_PARAMS {
            return Err(ParamLoadError::CountMismatch {
                expected: TOTAL_PARAMS as u16,
                got: count,
            });
        }
        let values_len = BLOB_HEADER_LEN + (count as usize) * 4;
        let expected = match version {
            1 => values_len,
            _ => values_len + BLOB_MATRIX_LEN,
        };
        if bytes.len() != expected {
            return Err(ParamLoadError::LengthMismatch {
                expected,
                got: bytes.len(),
            });
        }
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
        if version >= 2 {
            let mut off = values_len;
            for s in 0..N_MATRIX_SLOTS {
                let packed = u32::from_le_bytes([
                    bytes[off],
                    bytes[off + 1],
                    bytes[off + 2],
                    bytes[off + 3],
                ]);
                // v2 → v3: rewrite the `dest` byte through the v2 enum map
                // so old Ratio/Detune routes land on the new Pitch dest.
                let packed = if version < 3 {
                    let old_dest = ((packed >> 16) & 0xFF) as u8;
                    let new_dest = crate::matrix::DestId::from_u8_v2(old_dest) as u8;
                    (packed & 0xFF00_FFFF) | ((new_dest as u32) << 16)
                } else {
                    packed
                };
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
        }
        // Bulk store bypassed `set` / `set_matrix_row_raw`; flip every
        // dirty bit so the next main-thread tick re-broadcasts the full
        // table (ADR 0003). State load no longer needs a bespoke push
        // from the caller.
        self.mark_all_dirty();
        Ok(())
    }
}

// ── vxn_core_app::ParamModel + Vxn2Params bridges ───────────────────────────
//
// The local [`ParamModel`] above is the audio-thread / state-extension
// surface; the bridges below are the controller surface (ticket 0022).
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
            },
        );
    }

    fn mark_all_dirty(&self) {
        SharedParams::mark_all_dirty(self);
    }
}

// ── Engine params (audio-side mirror) ───────────────────────────────────────

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
    pub master: MasterParams,
    /// CLAP-automatable matrix slot depths.
    pub mtx_depths: [f32; N_CLAP_DEPTH_SLOTS],
    /// Patch-level LFO1 depth macro multiplier (matrix multiplies LFO1
    /// output by this at source-eval time).
    pub lfo1_depth: f32,
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
            master: MasterParams::default(),
            mtx_depths: [0.0; N_CLAP_DEPTH_SLOTS],
            lfo1_depth: 0.30,
            matrix_rows: [MatrixRowRaw::default(); N_MATRIX_SLOTS],
        };
        e.snapshot_from(p);
        e
    }

    /// Read every CLAP id out of `shared` and fan it into the engine fields.
    /// No allocation; no per-id branching beyond what the section readers
    /// need (enum decode, clamp). Call once per control block.
    pub fn snapshot_from<P: ParamView>(&mut self, shared: &P) {
        // LFO2 sync_index is patch state (not CLAP), so preserve it across
        // the snapshot like LFO1 / delay do below.
        let prev_lfo2_idx = self.patch.voice.lfo2.sync_index;
        self.patch = read_patch(shared);
        self.patch.voice.lfo2.sync_index = prev_lfo2_idx;

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

        self.mod_params.lfo1 = Lfo1Params {
            shape: lfo_shape_from(shared.get(pb + OFF_LFO1) as i32),
            rate_hz: shared.get(pb + OFF_LFO1 + 1),
            sync: shared.get(pb + OFF_LFO1 + 3) >= 0.5,
            sync_index: self.mod_params.lfo1.sync_index, // patch state (not CLAP)
        };
        self.lfo1_depth = shared.get(pb + OFF_LFO1 + 2);

        self.delay = StereoDelayParams {
            on: shared.get(pb + OFF_DELAY) >= 0.5,
            time_ms: shared.get(pb + OFF_DELAY + 1),
            sync: shared.get(pb + OFF_DELAY + 2) >= 0.5,
            sync_index: self.delay.sync_index, // patch state (not CLAP)
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

        self.master = MasterParams {
            tune_cents: shared.get(pb + OFF_MASTER),
            volume_db: shared.get(pb + OFF_MASTER + 1),
        };

        // Master tune bakes into the patch's per-op `base_phase_inc` at
        // note-on via `VoiceParams::master_tune_cents`.
        self.patch.voice.master_tune_cents = self.master.tune_cents;

        // Allocator reads the patch-level assignment block.
        self.alloc = read_assign(shared);
    }
}

// ── Section readers ────────────────────────────────────────────────────────

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
    OpParams {
        ratio_mode: RatioMode::Ratio, // not CLAP — preset state
        num: i(0).clamp(1, 32) as u8,
        denom: i(1).clamp(1, 8) as u8,
        fixed_hz: f(2),
        fine: i(3).clamp(-100, 100) as i8,
        detune: i(4).clamp(-100, 100) as i8,
        level: i(5).clamp(0, 99) as u8,
        vel_sens: i(6).clamp(0, 7) as u8,
        amp_sens: i(7).clamp(0, 3) as u8,
        eg: vxn2_dsp::eg::EgParams {
            r: [
                i(8).clamp(0, 99) as u8,
                i(9).clamp(0, 99) as u8,
                i(10).clamp(0, 99) as u8,
                i(11).clamp(0, 99) as u8,
            ],
            l: [
                i(12).clamp(0, 99) as u8,
                i(13).clamp(0, 99) as u8,
                i(14).clamp(0, 99) as u8,
                i(15).clamp(0, 99) as u8,
            ],
        },
        ks_break_pt: i(16).clamp(0, 127) as u8,
        ks_l_depth: i(17).clamp(0, 99) as u8,
        ks_r_depth: i(18).clamp(0, 99) as u8,
        ks_l_curve: vxn2_dsp::ks::KsCurve::NegLin, // not CLAP — preset state
        ks_r_curve: vxn2_dsp::ks::KsCurve::NegExp, // not CLAP — preset state
        ks_rate: i(19).clamp(0, 7) as u8,
        pan: f(20),
    }
}

fn read_lfo2<P: ParamView>(s: &P, base: usize) -> Lfo2Params {
    Lfo2Params {
        shape: lfo_shape_from(s.get(base) as i32),
        rate_hz: s.get(base + 1),
        delay_ms: s.get(base + 2),
        fade_ms: s.get(base + 3),
        sync: s.get(base + 4) >= 0.5,
        // Overwritten by the caller from the prior snapshot — see
        // [`EngineParams::snapshot_from`].
        sync_index: 6,
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

// ── Enum decoders ───────────────────────────────────────────────────────────

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
        assert!((s.get(crate::params::id_of("lfo1-rate").unwrap()) - 0.6).abs() < 1e-6);
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
            BLOB_HEADER_LEN + TOTAL_PARAMS * 4 + BLOB_MATRIX_LEN
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

    #[test]
    fn load_bytes_rejects_short_buffer() {
        let s = SharedParams::new();
        let err = s.load_bytes(&[0u8; 4]).unwrap_err();
        assert_eq!(
            err,
            ParamLoadError::LengthMismatch {
                expected: BLOB_HEADER_LEN + TOTAL_PARAMS * 4,
                got: 4,
            }
        );
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

    /// Two consecutive saves with no intervening param changes produce
    /// byte-for-byte identical blobs. Hosts rely on this for change detection.
    /// Matrix topology (source / dest / curve / active) round-trips through a
    /// blob save/load just like CLAP-automatable params. Regression for the
    /// bug where matrix edits silently vanished on patch reload.
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
            },
        );
        src.set_matrix_row_raw(
            9,
            MatrixRowRaw {
                source: 5,    // ModWheel
                dest: 21,     // Lfo2Rate (v3 numbering)
                curve: 0,
                active: true,
                depth: -0.6,
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

    /// A v2 blob's per-op Ratio (= old dest 1) and Detune (= old dest 3)
    /// both map to the new per-op Pitch dest (= new dest 1) on load.
    /// Global-tier dests shift down by 6 (drop the six Detune variants).
    #[test]
    fn load_bytes_migrates_v2_matrix_dests_to_v3() {
        let src = SharedParams::new();
        // Build a v3 blob, then rewrite the dest bytes back to the v2
        // encoding so we can re-load it as v2 and verify the migration.
        src.set_matrix_row_raw(
            0,
            MatrixRowRaw {
                source: 4,
                dest: crate::matrix::DestId::Op1Pitch as u8,
                curve: 0,
                active: true,
                depth: 0.5,
            },
        );
        src.set_matrix_row_raw(
            1,
            MatrixRowRaw {
                source: 4,
                dest: crate::matrix::DestId::Op2Pitch as u8,
                curve: 0,
                active: true,
                depth: 0.25,
            },
        );
        src.set_matrix_row_raw(
            2,
            MatrixRowRaw {
                source: 5,
                dest: crate::matrix::DestId::GlobalPitch as u8,
                curve: 0,
                active: true,
                depth: 0.1,
            },
        );
        let mut bytes = src.snapshot_bytes();
        // Stomp version to v2.
        bytes[4..6].copy_from_slice(&2u16.to_le_bytes());
        // Rewrite dest bytes in the matrix trailer to v2 codes:
        //   slot 0 → 3 (old Op1Detune) — both Ratio/Detune now collapse to Pitch
        //   slot 1 → 5 (old Op2Ratio)
        //   slot 2 → 25 (old GlobalPitch — shifts from 25 down to 19)
        let trailer_off = BLOB_HEADER_LEN + TOTAL_PARAMS * 4;
        // Each meta is 4 bytes; dest is at byte offset 1 (big-endian packing
        // in u32 → little-endian on wire means byte index +1 from start).
        // packed = source<<24 | dest<<16 | curve<<8 | active, stored LE →
        // bytes: [active, curve, dest, source]. Dest at +2.
        bytes[trailer_off + 0 * 4 + 2] = 3; // Op1Detune (v2)
        bytes[trailer_off + 1 * 4 + 2] = 5; // Op2Ratio (v2)
        bytes[trailer_off + 2 * 4 + 2] = 25; // GlobalPitch (v2)

        let dst = SharedParams::new();
        dst.load_bytes(&bytes).unwrap();

        let r0 = dst.matrix_row_raw(0);
        assert_eq!(r0.dest, crate::matrix::DestId::Op1Pitch as u8);
        let r1 = dst.matrix_row_raw(1);
        assert_eq!(r1.dest, crate::matrix::DestId::Op2Pitch as u8);
        let r2 = dst.matrix_row_raw(2);
        assert_eq!(r2.dest, crate::matrix::DestId::GlobalPitch as u8);
    }

    /// A v1 blob (no matrix trailer) still loads cleanly — older project
    /// files just leave the matrix at its current topology.
    #[test]
    fn load_bytes_accepts_legacy_v1_blob() {
        let src = SharedParams::new();
        let mut bytes = src.snapshot_bytes();
        // Strip the v2 trailer and rewrite the version word.
        bytes.truncate(BLOB_HEADER_LEN + TOTAL_PARAMS * 4);
        bytes[4..6].copy_from_slice(&1u16.to_le_bytes());
        let dst = SharedParams::new();
        dst.load_bytes(&bytes).expect("v1 blob loads");
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

    // ── Dirty bitset (ADR 0003 / ticket 0055) ──────────────────────────────

    fn drain_total(words: &[u64; N_DIRTY_VALUE_WORDS]) -> u32 {
        words.iter().map(|w| w.count_ones()).sum()
    }

    /// Fresh `SharedParams` carries an all-ones seed (full re-broadcast
    /// on the first tick). Matches the prior `last_seen = NaN` discipline.
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
            MatrixRowRaw { source: 4, dest: 2, curve: 0, active: true, depth: 0.3 },
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
            MatrixRowRaw { source: 4, dest: 2, curve: 1, active: true, depth: 0.5 },
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
            MatrixRowRaw { source: 4, dest: 2, curve: 1, active: true, depth: 0.42 },
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
