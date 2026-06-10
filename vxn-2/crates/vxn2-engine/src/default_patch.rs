//! Illustrative default patch (ticket 0018).
//!
//! Hand-tuned values that fill the parameter table and the matrix table so a
//! fresh instance sounds like an intentional voice on its first note rather
//! than a single sine carrier — DX-EP-flavoured electric piano with a slow
//! vibrato breath and a wide, decorrelated stack.
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
    // Op 1 — carrier; bell attack tail shaped by Op 2.
    set(&mut out, "op1-vel-sens", 4.0);
    set(&mut out, "op1-eg-l2", 80.0);
    set(&mut out, "op1-eg-l3", 70.0);
    set(&mut out, "op1-pan", -0.2);
    // Op 2 — bell-attack modulator → Op 1; fast decay, hard vel-sens.
    set(&mut out, "op2-num", 14.0);
    set(&mut out, "op2-level", 72.0);
    set(&mut out, "op2-vel-sens", 6.0);
    set(&mut out, "op2-eg-r2", 80.0);
    set(&mut out, "op2-eg-r3", 20.0);
    set(&mut out, "op2-eg-r4", 70.0);
    set(&mut out, "op2-eg-l2", 50.0);
    set(&mut out, "op2-eg-l3", 0.0);
    // Op 3 — carrier; warmer body, longer sustain.
    set(&mut out, "op3-level", 88.0);
    set(&mut out, "op3-eg-l2", 80.0);
    set(&mut out, "op3-eg-l3", 78.0);
    set(&mut out, "op3-pan", 0.2);
    // Op 4 — modulator → Op 3.
    set(&mut out, "op4-level", 64.0);
    set(&mut out, "op4-vel-sens", 5.0);
    set(&mut out, "op4-eg-r2", 60.0);
    set(&mut out, "op4-eg-r3", 30.0);
    set(&mut out, "op4-eg-l3", 40.0);
    // Op 5 / Op 6 — algo 5's third carrier pair, muted to taste.
    set(&mut out, "op5-level", 0.0);
    set(&mut out, "op6-level", 0.0);
    // LFO 2 (per-voice) — Sine, slow breath; always key-triggered.
    set(&mut out, "lfo2-shape", 0.0); // Sine
    set(&mut out, "lfo2-delay", 240.0);
    // Pitch EG — levels stay zero (descriptor default); rates kept reachable.
    // Mod Env — long-ish bell tail for matrix routing room.
    set(&mut out, "mod-env-d", 480.0);
    set(&mut out, "mod-env-r", 320.0);
    // Assign — Poly default; no glide.
    set(&mut out, "glide-time", 0.0);
    // Stack — density 4, mild detune + spread.
    set(&mut out, "stack-detune", 7.0);
    set(&mut out, "stack-spread", 0.55);
    // Matrix CLAP-automatable depths (slots 1..=6 active per `default_matrix`).
    set(&mut out, "mtx1-depth", 0.03);
    set(&mut out, "mtx2-depth", 1.0);
    set(&mut out, "mtx3-depth", 0.45);
    set(&mut out, "mtx4-depth", 0.6);
    set(&mut out, "mtx5-depth", 1.0);
    set(&mut out, "mtx6-depth", 1.0);

    // ── Patch-level ────────────────────────────────────────────────────────
    set(&mut out, "lfo1-rate", 0.6);
    set(&mut out, "delay-feedback", 0.30);
    set(&mut out, "delay-mix", 0.18);
    set(&mut out, "delay-pingpong", 1.0);
    set(&mut out, "reverb-mix", 0.18);

    out
}

/// Matrix table seeded at engine init. Slots 1..=4 carry the illustrative
/// routings (subtle vibrato, decorrelated stack LFO2, bell-attack velocity
/// boost, mod-wheel-driven vibrato rate); rest stay `None`.
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
        depth: 0.03,
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
        depth: 0.45,
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
