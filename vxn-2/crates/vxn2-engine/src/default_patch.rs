//! Default patch (ticket 0018): DX7 ROM1A "E.PIANO 1" transcription.
//!
//! Values decoded from the ROM1A 32-voice bulk sysex (voice 11), mapped onto
//! this engine's parameter semantics:
//!
//! - **Levels** (output levels and EG levels): the DX7 level scale is
//!   ~0.75 dB/step; our [`vxn2_dsp::eg::level_to_amp`] is quadratic
//!   `(v/99)²`. Converted via `v = 99 · 2^(0.75·(L−99)/12)` so the resulting
//!   amplitude / modulation index matches the original
//!   (e.g. OL 58 → 17, 89 → 64, 79 → 42; EG L 95 → 83, 75 → 35).
//! - **Rates**, **vel-sens** (KVS), **KS rate scaling**, **algorithm**,
//!   **feedback**: same 0-based scales, copied verbatim.
//! - **Detune**: DX7 steps (±7) ≈ ±5 cents here (op1 +3 → +2 ct,
//!   op5 −7 → −5 ct, op6 +7 → +5 ct).
//! - **KS break point**: DX7 BP 41 = MIDI 62 (op6); right-curve −LIN is
//!   approximated by the engine's fixed NegExp right curve.
//! - DX7 PMD is 0 (vibrato comes from the wheel on hardware), so the
//!   LFO2 → pitch matrix slot ships at depth 0 — wired, ready to dial in.
//! - No stack, no FX: the DX7 is a single dry voice (density 1, delay and
//!   reverb bypassed).
//!
//! [`default_param_values`] is the source of truth for [`crate::SharedParams::new`]
//! and the future preset epic (it will load this same patch from disk as
//! `Init.toml` in the factory bank). [`default_matrix`] seeds the matrix
//! table at engine init.
//!
//! Deterministic and side-effect-free: no randomness, no time-of-day inputs.
//! Stack `voice_rand` (used by Mtx slot 2 to decorrelate per-lane LFO2 phase)
//! is sampled in the allocator at note-on, independent of these defaults.

use crate::matrix::{CurveKind, DestId, MatrixSlot, MatrixTable, SourceId};
use crate::params::{PARAMS, TOTAL_PARAMS, id_of};

/// Plain-units default value for every CLAP id. Replaces a per-descriptor
/// default seed; the descriptor table is still authoritative for ranges /
/// tapers / display.
pub fn default_param_values() -> [f32; TOTAL_PARAMS] {
    let mut out = [0.0_f32; TOTAL_PARAMS];
    for i in 0..TOTAL_PARAMS {
        out[i] = PARAMS[i].default;
    }
    let set = |out: &mut [f32; TOTAL_PARAMS], name: &str, v: f32| {
        let id = id_of(name).unwrap_or_else(|| panic!("unknown id {name}"));
        out[id] = PARAMS[id].clamp(v);
    };

    // ── Per-op ─────────────────────────────────────────────────────────────
    // ROM1A E.PIANO 1, algo 5: three 2-stacks (2→1), (4→3), (6→5), fb op6.
    // DX7 source values quoted in trailing comments where converted.
    // Op 1 — carrier (pair A body).
    set(&mut out, "op1-detune", 2.0); // DX7 +3 steps
    set(&mut out, "op1-vel-sens", 2.0);
    set(&mut out, "op1-eg-r1", 96.0);
    set(&mut out, "op1-eg-r2", 25.0);
    set(&mut out, "op1-eg-r3", 25.0);
    set(&mut out, "op1-eg-r4", 67.0);
    set(&mut out, "op1-eg-l2", 35.0); // DX7 L2 75
    set(&mut out, "op1-eg-l3", 0.0);
    set(&mut out, "op1-ks-r-depth", 0.0);
    set(&mut out, "op1-ks-rate", 3.0);
    // Op 2 — tine modulator → Op 1; ratio 14, hard vel-sens.
    set(&mut out, "op2-num", 14.0);
    set(&mut out, "op2-level", 17.0); // DX7 OL 58
    set(&mut out, "op2-vel-sens", 7.0);
    set(&mut out, "op2-eg-r1", 95.0);
    set(&mut out, "op2-eg-r4", 78.0);
    set(&mut out, "op2-eg-l2", 35.0); // DX7 L2 75
    set(&mut out, "op2-eg-l3", 0.0);
    set(&mut out, "op2-ks-r-depth", 0.0);
    set(&mut out, "op2-ks-rate", 3.0);
    // Op 3 — carrier (pair B body).
    set(&mut out, "op3-vel-sens", 2.0);
    set(&mut out, "op3-eg-r1", 95.0);
    set(&mut out, "op3-eg-r2", 20.0);
    set(&mut out, "op3-eg-r3", 20.0);
    set(&mut out, "op3-eg-r4", 50.0);
    set(&mut out, "op3-eg-l2", 83.0); // DX7 L2 95
    set(&mut out, "op3-eg-l3", 0.0);
    set(&mut out, "op3-ks-r-depth", 0.0);
    set(&mut out, "op3-ks-rate", 3.0);
    // Op 4 — modulator → Op 3.
    set(&mut out, "op4-level", 64.0); // DX7 OL 89
    set(&mut out, "op4-vel-sens", 6.0);
    set(&mut out, "op4-eg-r1", 95.0);
    set(&mut out, "op4-eg-r2", 29.0);
    set(&mut out, "op4-eg-r3", 20.0);
    set(&mut out, "op4-eg-r4", 50.0);
    set(&mut out, "op4-eg-l2", 83.0); // DX7 L2 95
    set(&mut out, "op4-eg-l3", 0.0);
    set(&mut out, "op4-ks-r-depth", 0.0);
    set(&mut out, "op4-ks-rate", 3.0);
    // Op 5 — carrier (pair C body), detuned against Op 1 for chorusing.
    set(&mut out, "op5-detune", -5.0); // DX7 −7 steps
    set(&mut out, "op5-vel-sens", 0.0);
    set(&mut out, "op5-eg-r1", 95.0);
    set(&mut out, "op5-eg-r2", 20.0);
    set(&mut out, "op5-eg-r3", 20.0);
    set(&mut out, "op5-eg-r4", 50.0);
    set(&mut out, "op5-eg-l2", 83.0); // DX7 L2 95
    set(&mut out, "op5-eg-l3", 0.0);
    set(&mut out, "op5-ks-r-depth", 0.0);
    set(&mut out, "op5-ks-rate", 3.0);
    // Op 6 — modulator → Op 5, carries the structural feedback loop;
    // level fades above D4 (KS break 62, right depth 19).
    set(&mut out, "op6-detune", 5.0); // DX7 +7 steps
    set(&mut out, "op6-level", 42.0); // DX7 OL 79
    set(&mut out, "op6-vel-sens", 6.0);
    set(&mut out, "op6-eg-r1", 95.0);
    set(&mut out, "op6-eg-r2", 29.0);
    set(&mut out, "op6-eg-r3", 20.0);
    set(&mut out, "op6-eg-r4", 50.0);
    set(&mut out, "op6-eg-l2", 83.0); // DX7 L2 95
    set(&mut out, "op6-eg-l3", 0.0);
    set(&mut out, "op6-ks-break-pt", 62.0); // DX7 BP 41 = D4
    set(&mut out, "op6-ks-r-depth", 19.0);
    set(&mut out, "op6-ks-rate", 3.0);

    // ── Patch-level ────────────────────────────────────────────────────────
    // Algo 5 is the descriptor default; feedback 6 on op6 per the ROM voice.
    set(&mut out, "feedback", 6.0);
    // LFO 2 (per-voice) — DX7 LFO: sine, speed 34 (~5.6 Hz), delay 33,
    // PMD/AMD 0. Pitch depth lives in Mtx slot 1, shipped at 0.
    set(&mut out, "lfo2-shape", 0.0); // Sine
    set(&mut out, "lfo2-rate", 5.6);
    set(&mut out, "lfo2-delay", 400.0);
    // Pitch EG — DX7 rates 94/67/95/60; levels all 50 (center) → 0 here.
    set(&mut out, "peg-r1", 94.0);
    set(&mut out, "peg-r2", 67.0);
    set(&mut out, "peg-r3", 95.0);
    set(&mut out, "peg-r4", 60.0);
    // Assign — Poly default; no glide.
    set(&mut out, "glide-time", 0.0);
    // Stack — single dry voice like the hardware.
    set(&mut out, "stack-density", 1.0);
    // Matrix CLAP-automatable depths (slots 1..=6 active per `default_matrix`;
    // slots 1 and 3 ship at 0 — see `default_matrix` docs).
    set(&mut out, "mtx2-depth", 1.0);
    set(&mut out, "mtx4-depth", 0.6);
    set(&mut out, "mtx5-depth", 1.0);
    set(&mut out, "mtx6-depth", 1.0);
    // FX — bypassed; the DX7 voice is dry.
    set(&mut out, "delay-on", 0.0);
    set(&mut out, "reverb-on", 0.0);

    out
}

/// Matrix table seeded at engine init. Slots 1..=6 keep the standard
/// routings wired but two ship at depth 0 to match the DX7 voice:
///
/// - Slot 1 (LFO2 → pitch): the ROM voice has PMD 0 — vibrato came from the
///   mod wheel on hardware. Depth 0; dial up for vibrato.
/// - Slot 3 (velocity → Op 2 level): the tine's velocity response is fully
///   carried by op2-vel-sens 7. Depth 0; dial up for extra bite.
///
/// Rest stay `None`.
///
/// Slot depths are also stored in the param table — [`crate::engine::Engine::apply_block_params`]
/// overwrites slots 1..=8 depth from the CLAP-automatable mtx params each
/// block. The depth set here is a redundant book-keeping value; the patch
/// table is what actually feeds the engine.
pub fn default_matrix() -> MatrixTable {
    let mut t = MatrixTable::default();
    t.slots[0] = MatrixSlot {
        source: SourceId::Lfo2,
        dest: DestId::GlobalPitch,
        depth: 0.0,
        curve: CurveKind::Lin,
    };
    t.slots[1] = MatrixSlot {
        source: SourceId::VoiceRand,
        dest: DestId::Lfo2Phase,
        depth: 1.0,
        curve: CurveKind::Lin,
    };
    t.slots[2] = MatrixSlot {
        source: SourceId::Velocity,
        dest: DestId::Op2Level,
        depth: 0.0,
        curve: CurveKind::Exp,
    };
    t.slots[3] = MatrixSlot {
        source: SourceId::ModWheel,
        dest: DestId::Lfo1Rate,
        depth: 0.6,
        curve: CurveKind::Lin,
    };
    // Lane spread → per-carrier pan. The auto pan-spread path was removed —
    // wire it explicitly through the matrix so the macro is one of many
    // possible spread → pan curves users can dial in. Depth 1.0 + Lin
    // reproduces the old `spread * voice_spread[k]` behaviour exactly.
    t.slots[4] = MatrixSlot {
        source: SourceId::VoiceSpread,
        dest: DestId::Op1Pan,
        depth: 1.0,
        curve: CurveKind::Lin,
    };
    t.slots[5] = MatrixSlot {
        source: SourceId::VoiceSpread,
        dest: DestId::Op3Pan,
        depth: 1.0,
        curve: CurveKind::Lin,
    };
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_in_descriptor_range() {
        let v = default_param_values();
        for i in 0..TOTAL_PARAMS {
            let d = &PARAMS[i];
            assert!(
                v[i] >= d.min && v[i] <= d.max,
                "{}: {} out of [{}, {}]",
                d.id,
                v[i],
                d.min,
                d.max
            );
        }
    }

    #[test]
    fn values_are_deterministic() {
        assert_eq!(default_param_values(), default_param_values());
    }

    #[test]
    fn matrix_seeds_six_active_slots() {
        let m = default_matrix();
        assert_eq!(m.slots[0].source, SourceId::Lfo2);
        assert_eq!(m.slots[0].dest, DestId::GlobalPitch);
        assert_eq!(m.slots[1].source, SourceId::VoiceRand);
        assert_eq!(m.slots[1].dest, DestId::Lfo2Phase);
        assert_eq!(m.slots[2].source, SourceId::Velocity);
        assert_eq!(m.slots[2].dest, DestId::Op2Level);
        assert_eq!(m.slots[2].curve, CurveKind::Exp);
        assert_eq!(m.slots[3].source, SourceId::ModWheel);
        assert_eq!(m.slots[3].dest, DestId::Lfo1Rate);
        // Slots 5/6: VoiceSpread → carrier-op pan, the explicit replacement
        // for the dropped auto pan-spread path.
        assert_eq!(m.slots[4].source, SourceId::VoiceSpread);
        assert_eq!(m.slots[4].dest, DestId::Op1Pan);
        assert_eq!(m.slots[5].source, SourceId::VoiceSpread);
        assert_eq!(m.slots[5].dest, DestId::Op3Pan);
        for slot in &m.slots[6..] {
            assert_eq!(slot.source, SourceId::None);
            assert_eq!(slot.dest, DestId::None);
        }
    }

    #[test]
    fn matrix_depths_match_param_table_defaults() {
        // The CLAP-automatable depth defaults must agree with the in-engine
        // MatrixTable depths — `apply_block_params` overwrites the latter from
        // the former each block, so a disagreement here is a silent footgun.
        let v = default_param_values();
        let m = default_matrix();
        assert_eq!(v[id_of("mtx1-depth").unwrap()], m.slots[0].depth);
        assert_eq!(v[id_of("mtx2-depth").unwrap()], m.slots[1].depth);
        assert_eq!(v[id_of("mtx3-depth").unwrap()], m.slots[2].depth);
        assert_eq!(v[id_of("mtx4-depth").unwrap()], m.slots[3].depth);
        assert_eq!(v[id_of("mtx5-depth").unwrap()], m.slots[4].depth);
        assert_eq!(v[id_of("mtx6-depth").unwrap()], m.slots[5].depth);
    }
}
