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
//! [`EngineParams`] is the audio-side mirror: one [`Patch`] (voicing + Upper
//! + Lower), patch-mod params (LFO1), delay + reverb + master + a single
//! [`AllocParams`] derived from Upper's assignment block (see note below),
//! and the per-layer 8-slot CLAP-automatable matrix depths.
//!
//! [`EngineParams::snapshot_from`] walks the flat store once per control
//! block and routes each id into the matching field — straight indexed
//! reads, no allocation.
//!
//! ### Per-layer assignment in v1
//!
//! The param table exposes `upper-assign-mode` / `lower-assign-mode` (and
//! the companion legato + glide-time) per layer for forward compatibility —
//! the UI expects Split-mode bass-mono / lead-poly setups to round-trip.
//! The current [`crate::alloc::PolyAlloc`] takes a single [`AllocParams`]
//! per note-on; v1 reads Upper's assignment block into the engine's live
//! `AllocParams`. Lower's entries remain in the store (visible to host
//! automation) but inert until the allocator is refactored.

use std::sync::atomic::{AtomicU32, Ordering};

use vxn2_dsp::delay::StereoDelayParams;
use vxn2_dsp::envelope::{AdsrShape, ModEnvParams, PitchEgParams};
use vxn2_dsp::lfo::{Lfo1Params, Lfo2Params, Lfo2Trig, LfoShape};
use vxn2_dsp::op::{OpParams, RatioMode};
use vxn2_dsp::reverb::FdnReverbParams;
use vxn2_dsp::stack::{StackDistrib, StackParams};
use vxn2_dsp::voice::VoiceParams;

use crate::alloc::{AllocParams, AssignMode};
use crate::master::MasterParams;
use crate::matrix::N_CLAP_DEPTH_SLOTS;
use crate::modulation::PatchModParams;
use crate::params::{
    LOWER_BASE, N_PER_OP, OFF_ALGO, OFF_ASSIGN, OFF_DELAY, OFF_LFO1, OFF_LFO2, OFF_MASTER,
    OFF_MOD_ENV, OFF_MTX, OFF_PEG, OFF_REVERB, OFF_STACK, OFF_VOICING, PARAMS, PATCH_BASE,
    TOTAL_PARAMS,
};
use crate::voicing::{LayerParams, Patch, VoicingMode, VoicingParams};

// ── ParamModel trait surface ────────────────────────────────────────────────

/// Errors returned by [`ParamModel::load_bytes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamLoadError {
    /// Wire payload length did not match `TOTAL_PARAMS * 4`.
    LengthMismatch { expected: usize, got: usize },
}

impl std::fmt::Display for ParamLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LengthMismatch { expected, got } => {
                write!(f, "param payload length {got} (expected {expected})")
            }
        }
    }
}

impl std::error::Error for ParamLoadError {}

/// Indexed read access into a param store, keyed by CLAP id.
///
/// Internal supertrait of [`ParamModel`]; both the atomic store and the
/// audio-thread mirror in `vxn2-clap::local` implement it so the section
/// readers below (and [`EngineParams::snapshot_from`]) can drive either.
pub trait ParamView {
    fn get(&self, id: usize) -> f32;
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

/// Lock-free param store. Sized to [`TOTAL_PARAMS`] (343 in v1). Cheap to
/// share via `Arc` — every field is an `AtomicU32`.
pub struct SharedParams {
    values: [AtomicU32; TOTAL_PARAMS],
}

impl Default for SharedParams {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedParams {
    /// Initialise every slot to its descriptor default.
    pub fn new() -> Self {
        Self {
            values: std::array::from_fn(|i| AtomicU32::new(PARAMS[i].default.to_bits())),
        }
    }

    #[inline]
    pub fn get(&self, id: usize) -> f32 {
        f32::from_bits(self.values[id].load(Ordering::Relaxed))
    }

    /// Store `value` clamped to the descriptor's plain range.
    #[inline]
    pub fn set(&self, id: usize, value: f32) {
        if id < TOTAL_PARAMS {
            let d = &PARAMS[id];
            self.values[id].store(d.clamp(value).to_bits(), Ordering::Relaxed);
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

    /// Restore every slot to its descriptor default.
    pub fn reset_to_defaults(&self) {
        for i in 0..TOTAL_PARAMS {
            self.values[i].store(PARAMS[i].default.to_bits(), Ordering::Relaxed);
        }
    }
}

impl ParamView for SharedParams {
    #[inline]
    fn get(&self, id: usize) -> f32 {
        SharedParams::get(self, id)
    }
}

impl ParamModel for SharedParams {
    fn total(&self) -> usize {
        TOTAL_PARAMS
    }

    fn get_normalised(&self, id: usize) -> f32 {
        SharedParams::get_normalised(self, id)
    }

    /// Serialise every slot as raw `f32` bits, little-endian. 4 bytes per id;
    /// total `TOTAL_PARAMS * 4`. Allocation is unavoidable (the trait returns
    /// `Vec<u8>`), but the call site is the host's state-save path — main
    /// thread, not audio.
    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(TOTAL_PARAMS * 4);
        for i in 0..TOTAL_PARAMS {
            let bits = self.values[i].load(Ordering::Relaxed);
            buf.extend_from_slice(&bits.to_le_bytes());
        }
        buf
    }

    /// Inverse of [`snapshot_bytes`]. Writes bits unmodified — no descriptor
    /// clamp — so a snapshot round-trip is bit-identical.
    fn load_bytes(&self, bytes: &[u8]) -> Result<(), ParamLoadError> {
        let expected = TOTAL_PARAMS * 4;
        if bytes.len() != expected {
            return Err(ParamLoadError::LengthMismatch {
                expected,
                got: bytes.len(),
            });
        }
        for i in 0..TOTAL_PARAMS {
            let off = i * 4;
            let bits = u32::from_le_bytes([
                bytes[off],
                bytes[off + 1],
                bytes[off + 2],
                bytes[off + 3],
            ]);
            self.values[i].store(bits, Ordering::Relaxed);
        }
        Ok(())
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
    /// Per-layer CLAP-automatable matrix slot depths
    /// (`[upper][slot]`, `[lower][slot]`).
    pub mtx_depths: [[f32; N_CLAP_DEPTH_SLOTS]; 2],
    /// Patch-level LFO1 depth macro multiplier (matrix multiplies LFO1
    /// output by this at source-eval time).
    pub lfo1_depth: f32,
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
            mtx_depths: [[0.0; N_CLAP_DEPTH_SLOTS]; 2],
            lfo1_depth: 0.30,
        };
        e.snapshot_from(p);
        e
    }

    /// Read every CLAP id out of `shared` and fan it into the engine fields.
    /// No allocation; no per-id branching beyond what the section readers
    /// need (enum decode, clamp). Call once per control block.
    pub fn snapshot_from<P: ParamView>(&mut self, shared: &P) {
        // Per-layer blocks.
        self.patch.upper = read_layer(shared, 0);
        self.patch.lower = read_layer(shared, LOWER_BASE);

        // Per-layer matrix CLAP-automatable depths.
        for s in 0..N_CLAP_DEPTH_SLOTS {
            self.mtx_depths[0][s] = shared.get(OFF_MTX + s);
            self.mtx_depths[1][s] = shared.get(LOWER_BASE + OFF_MTX + s);
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

        self.patch.voicing = VoicingParams {
            mode: voicing_mode_from(shared.get(pb + OFF_VOICING) as i32),
            split_point: shared.get(pb + OFF_VOICING + 1).clamp(0.0, 127.0) as u8,
        };

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

        // Master tune cascades into both layers (DSP path bakes
        // `master_tune_cents` into each op's phase_inc at note-on).
        self.patch.upper.voice.master_tune_cents = self.master.tune_cents;
        self.patch.lower.voice.master_tune_cents = self.master.tune_cents;

        // Allocator reads Upper's assignment for v1; see module doc.
        self.alloc = read_assign(shared, 0);
    }
}

// ── Section readers ────────────────────────────────────────────────────────

fn read_layer<P: ParamView>(s: &P, base: usize) -> LayerParams {
    let mut ops = [OpParams::default(); 6];
    for i in 0..6 {
        ops[i] = read_op(s, base + i * N_PER_OP);
    }
    let voice = VoiceParams {
        ops,
        algo: s.get(base + OFF_ALGO).clamp(1.0, 32.0) as u8,
        master_tune_cents: 0.0, // overwritten with patch-level value post-snap
        lfo2: read_lfo2(s, base + OFF_LFO2),
        pitch_eg: read_peg(s, base + OFF_PEG),
        peg_depth: s.get(base + OFF_PEG + 8),
        mod_env: read_mod_env(s, base + OFF_MOD_ENV),
    };
    let stack = read_stack(s, base + OFF_STACK);
    LayerParams { stack, voice }
}

fn read_op<P: ParamView>(s: &P, base: usize) -> OpParams {
    let f = |off| s.get(base + off);
    let i = |off| s.get(base + off).round() as i32;
    OpParams {
        ratio_mode: RatioMode::Ratio, // not CLAP — preset state
        ratio: f(0),
        fixed_hz: f(1),
        fine: f(2),
        detune: i(3).clamp(-7, 7) as i8,
        level: i(4).clamp(0, 99) as u8,
        vel_sens: i(5).clamp(0, 7) as u8,
        amp_sens: i(6).clamp(0, 3) as u8,
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
        ks_break_pt: i(15).clamp(0, 127) as u8,
        ks_l_depth: i(16).clamp(0, 99) as u8,
        ks_r_depth: i(17).clamp(0, 99) as u8,
        ks_l_curve: vxn2_dsp::ks::KsCurve::NegLin, // not CLAP — preset state
        ks_r_curve: vxn2_dsp::ks::KsCurve::NegExp, // not CLAP — preset state
        ks_rate: i(18).clamp(0, 7) as u8,
        pan: f(19),
        feedback: i(20).clamp(0, 7) as u8,
    }
}

fn read_lfo2<P: ParamView>(s: &P, base: usize) -> Lfo2Params {
    Lfo2Params {
        shape: lfo_shape_from(s.get(base) as i32),
        rate_hz: s.get(base + 1),
        delay_ms: s.get(base + 2),
        fade_ms: s.get(base + 3),
        trig: match s.get(base + 4).round() as i32 {
            0 => Lfo2Trig::Free,
            _ => Lfo2Trig::KeySync,
        },
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

fn read_assign<P: ParamView>(s: &P, layer_base: usize) -> AllocParams {
    let off = layer_base + OFF_ASSIGN;
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

#[inline]
fn voicing_mode_from(i: i32) -> VoicingMode {
    match i {
        0 => VoicingMode::Whole,
        1 => VoicingMode::Layer,
        _ => VoicingMode::Split,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::id_of;

    #[test]
    fn shared_params_defaults_match_descriptor_defaults() {
        let s = SharedParams::new();
        for i in 0..TOTAL_PARAMS {
            assert_eq!(
                s.get(i),
                PARAMS[i].default,
                "default mismatch at {}: id={}",
                i,
                PARAMS[i].id
            );
        }
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
        assert_eq!(e.patch.voicing.mode, VoicingMode::Layer);
        assert!((e.reverb.size - 0.55).abs() < 1e-6);
        assert!((e.delay.time_ms - 375.0).abs() < 1e-6);
        assert_eq!(e.patch.upper.voice.master_tune_cents, 0.0);
        assert_eq!(e.patch.lower.voice.master_tune_cents, 0.0);
    }

    #[test]
    fn snapshot_master_tune_cascades_to_both_layers() {
        let s = SharedParams::new();
        s.set(id_of("master-tune").unwrap(), 25.0);
        let e = EngineParams::from_shared(&s);
        assert_eq!(e.patch.upper.voice.master_tune_cents, 25.0);
        assert_eq!(e.patch.lower.voice.master_tune_cents, 25.0);
    }

    #[test]
    fn snapshot_resolves_per_op_block() {
        let s = SharedParams::new();
        s.set(id_of("upper-op3-ratio").unwrap(), 7.5);
        s.set(id_of("lower-op6-level").unwrap(), 42.0);
        let e = EngineParams::from_shared(&s);
        assert!((e.patch.upper.voice.ops[2].ratio - 7.5).abs() < 1e-5);
        assert_eq!(e.patch.lower.voice.ops[5].level, 42);
    }

    #[test]
    fn snapshot_resolves_mtx_depths() {
        let s = SharedParams::new();
        s.set(id_of("upper-mtx1-depth").unwrap(), 0.4);
        s.set(id_of("lower-mtx8-depth").unwrap(), -0.7);
        let e = EngineParams::from_shared(&s);
        assert!((e.mtx_depths[0][0] - 0.4).abs() < 1e-6);
        assert!((e.mtx_depths[1][7] - (-0.7)).abs() < 1e-6);
    }

    #[test]
    fn snapshot_bytes_round_trip_is_bit_identical() {
        let src = SharedParams::new();
        // Touch a spread of slots so we exercise per-id positions.
        for (name, v) in [
            ("upper-op1-ratio", 3.25_f32),
            ("upper-op6-level", 88.0),
            ("lower-op4-pan", -0.7),
            ("master-volume", -3.0),
            ("reverb-decay", 4.5),
            ("delay-time", 250.0),
        ] {
            let id = id_of(name).unwrap();
            src.set(id, v);
        }
        // Stuff a NaN bit pattern into a slot we don't care about — load_bytes
        // must preserve it bit-for-bit so the round-trip is unambiguous.
        let nan_id = id_of("lower-op2-fine").unwrap();
        let pattern = 0x7fc0_dead_u32;
        src.values[nan_id].store(pattern, Ordering::Relaxed);

        let bytes = src.snapshot_bytes();
        assert_eq!(bytes.len(), TOTAL_PARAMS * 4);

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
    fn load_bytes_rejects_wrong_length() {
        let s = SharedParams::new();
        let err = s.load_bytes(&[0u8; 4]).unwrap_err();
        assert_eq!(
            err,
            ParamLoadError::LengthMismatch {
                expected: TOTAL_PARAMS * 4,
                got: 4,
            }
        );
    }

    #[test]
    fn snapshot_uses_upper_assignment() {
        let s = SharedParams::new();
        s.set(id_of("upper-assign-mode").unwrap(), 1.0);
        s.set(id_of("upper-legato").unwrap(), 1.0);
        s.set(id_of("upper-glide-time").unwrap(), 200.0);
        s.set(id_of("lower-assign-mode").unwrap(), 0.0);
        let e = EngineParams::from_shared(&s);
        assert_eq!(e.alloc.assign_mode, AssignMode::Solo);
        assert!(e.alloc.legato);
        assert!((e.alloc.glide_time_ms - 200.0).abs() < 1e-5);
    }
}
