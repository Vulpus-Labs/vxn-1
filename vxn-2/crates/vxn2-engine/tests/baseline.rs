//! Render-hash baseline (ticket 0119). Drives the engine through a fixed,
//! matrix-rich patch and hashes the rendered stereo output bit-for-bit. The
//! `cook_stacks_block` extraction and the `RampState` collapse are both
//! behaviour-preserving refactors of the per-stack mod-matrix loop; this test
//! is the guard that proves it — a stage reorder (one-block-latency bug) or a
//! ramp-index regression (a mis-stepped parallel Vec) perturbs at least one
//! sample and flips the hash.
//!
//! The patch lights up every cook stage at once: per-op level / pan / phase
//! ramps, the stack-pitch scatter, the global-pitch smoother, FX-mix
//! aggregation, per-stack feedback, the deferred LFO1-rate / LFO2-phase /
//! stack-detune routes — across a held chord with spread lanes, driven the way
//! the CLAP shell drives it (re-`apply_block_params` per control block).
//!
//! If an *intentional* DSP change moves the hash, re-capture it: run with
//! `--nocapture`, read the `BASELINE render hash = 0x…` line the test prints,
//! and paste it into `EXPECTED`.

use vxn2_engine::MatrixRowRaw;
use vxn2_engine::engine::Engine;

use std::hash::{Hash, Hasher};

const SR: f32 = 48_000.0;
const BLK: usize = 32;

/// Golden hash of the reference render. Behaviour-preserving refactors must
/// leave this untouched; an intentional DSP change re-captures it (see header).
const EXPECTED: u64 = 0x9460_b892_913e_5b85;

/// Build the reference engine: a matrix-rich, deterministic patch.
fn reference_engine() -> Engine {
    let mut e = Engine::new(SR, BLK);

    // FX on so the FX-mix aggregation stage actually feeds the chain.
    e.params.delay.on = true;
    e.params.delay.mix = 0.3;
    e.params.reverb.on = true;
    e.params.reverb.mix = 0.25;

    // Moving sources for the matrix.
    e.params.mod_params.lfo1.rate_hz = 5.0;
    // LFO2 is a per-voice (per-stack) modulator — its params live on the voice.
    e.params.patch.voice.lfo2.rate_hz = 7.3;
    // Spread lanes so per-lane pan / voice-spread paths carry real motion.
    e.params.patch.stack.density = 4;
    e.params.patch.stack.spread = 0.6;

    // Ten routes, one per cook stage of interest. Dest ids are `DestId as u8`
    // (None = 0): Op1Level = 2, Op2Pan = 6, GlobalPitch = 19, Lfo1Rate = 20,
    // Lfo2Phase = 22, StackDetune = 23, DelayMix = 25, Feedback = 27,
    // Op1StackPitch = 30, Op1Phase = 36. Sources (SourceId as u8): Lfo1 = 1,
    // Lfo2 = 2, ModEnv = 4, ModWheel = 5, Velocity = 7.
    let routes: [(u8, u8, f32); 10] = [
        (1, 2, 1.0),   // Lfo1   → Op1Level     (level ramp)
        (1, 6, 0.8),   // Lfo1   → Op2Pan       (pan ramp)
        (2, 36, 0.5),  // Lfo2   → Op1Phase     (phase ramp, E023)
        (4, 30, 0.7),  // ModEnv → Op1StackPitch(stack-pitch scatter)
        (1, 19, 0.4),  // Lfo1   → GlobalPitch  (pitch smoother)
        (7, 25, 0.6),  // Velocity → DelayMix   (FX aggregation)
        (5, 27, 0.5),  // ModWheel → Feedback   (per-stack feedback mod)
        (1, 20, 0.5),  // Lfo1   → Lfo1Rate     (deferred lfo1-rate)
        (4, 23, 0.5),  // ModEnv → StackDetune  (deferred stack macro)
        (2, 22, 0.5),  // Lfo2   → Lfo2Phase    (deferred lfo2-phase)
    ];
    for (s, &(source, dest, depth)) in routes.iter().enumerate() {
        e.params.matrix_rows[s] = MatrixRowRaw {
            source,
            dest,
            curve: 0,
            active: true,
            depth,
        };
        // Slots < N_CLAP_DEPTH_SLOTS (8) read the CLAP depth; later slots read
        // the row depth. Set both so every route's depth lands regardless.
        if s < 8 {
            e.params.mtx_depths[s] = depth;
        }
    }
    e.apply_block_params();
    e
}

/// Render `blocks` control blocks the CLAP way (re-apply params each block),
/// folding every output sample's bit pattern into the hash.
fn render_hash(e: &mut Engine, blocks: usize, h: &mut impl Hasher) {
    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    for _ in 0..blocks {
        e.apply_block_params();
        e.process_block(&mut l, &mut r);
        for i in 0..BLK {
            l[i].to_bits().hash(h);
            r[i].to_bits().hash(h);
        }
    }
}

#[test]
fn render_hash_unchanged() {
    let mut e = reference_engine();

    // A held chord with motion: mod-wheel + aftertouch keep the ModWheel /
    // (and patch-global) sources off their zero, so their routes contribute.
    e.set_mod_wheel(0.7);
    e.set_aftertouch(0.4);
    for &note in &[48u8, 55, 60, 64] {
        e.note_on(note, 100);
    }

    let mut h = std::collections::hash_map::DefaultHasher::new();
    // ~0.25 s of audio: long enough for EG attack/decay, LFO travel, and the
    // one-block-latency deferred routes to settle and then move.
    render_hash(&mut e, (SR as usize / 4) / BLK, &mut h);
    // Release and let the tails ring so the OFF-path render loop is exercised.
    for &note in &[48u8, 55, 60, 64] {
        e.note_off(note);
    }
    render_hash(&mut e, (SR as usize / 8) / BLK, &mut h);

    let got = h.finish();
    println!("BASELINE render hash = {got:#018x}");
    assert_eq!(
        got, EXPECTED,
        "render hash changed: cook-stage reorder or ramp-index regression \
         (or an intentional DSP change — re-capture EXPECTED; see header)"
    );
}
