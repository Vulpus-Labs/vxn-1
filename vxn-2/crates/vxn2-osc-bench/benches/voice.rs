//! Per-voice cost (6 ops + algorithm routing + EG advance), ticket 0003.
//!
//! Two scenarios:
//! - `voice_steady`  — held note, all 6 ops in Sustain (level cached). Per-
//!   sample hot path is `voice_tick`: router (carrier sum + 6 mod inputs) +
//!   6× `op_tick`. Block-rate `eg_tick` included.
//! - `voice_release` — note-on then note-off; the release tail (R4=99
//!   default → ~4 ms sweep at L4=0) runs while the EG decays into Idle.
//!   Stresses the EG state machine + post-release routing.
//!
//! Throughput = samples per call (BLOCK × VOICES). Compared against
//! `op_voice_*` (single-op) the multiplier is ≤ 6× since the router and EG
//! are amortised across the 6 ops in one voice.

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use vxn2_dsp::voice::{Voice, VoiceMod, VoiceParams, voice_tick};

const BLOCK: usize = 256;
const VOICES: usize = 16;
const SR: f32 = 48_000.0;

fn build_voices(algo: u8) -> [Voice; VOICES] {
    let mut params = VoiceParams::default();
    params.algo = algo;
    // Pump up modulator levels so the FM index isn't 0 (default OpParams
    // has level=99 on every op, which is carrier-friendly).
    for op in &mut params.ops {
        op.feedback = 2;
    }
    let mut voices = [Voice::default(); VOICES];
    for (i, v) in voices.iter_mut().enumerate() {
        let key = 48 + (i as u8 * 2) % 48;
        v.note_on(&params, key, 100, SR);
        // Decorrelate phase across voices.
        for (j, op) in v.ops.iter_mut().enumerate() {
            op.phase = ((i * 6 + j) as u32).wrapping_mul(0x1234_5678);
        }
    }
    voices
}

fn render_steady(voices: &mut [Voice; VOICES]) -> f32 {
    let dt_block = BLOCK as f32 / SR;
    let modu = VoiceMod::default();
    let mut acc = 0.0;
    for v in voices.iter_mut() {
        for op in &mut v.ops {
            op.force_sustain(0.5);
        }
        v.eg_tick(dt_block);
    }
    for _ in 0..BLOCK {
        for v in voices.iter_mut() {
            acc += voice_tick(v, &modu);
        }
    }
    acc
}

fn render_release(voices: &mut [Voice; VOICES]) -> f32 {
    let dt_block = BLOCK as f32 / SR;
    let modu = VoiceMod::default();
    let mut acc = 0.0;
    for v in voices.iter_mut() {
        for op in &mut v.ops {
            op.force_sustain(0.5);
        }
        v.note_off();
        v.eg_tick(dt_block);
    }
    for _ in 0..BLOCK {
        for v in voices.iter_mut() {
            acc += voice_tick(v, &modu);
        }
    }
    acc
}

fn bench_voice(c: &mut Criterion) {
    let mut g = c.benchmark_group("voice");
    g.throughput(Throughput::Elements((BLOCK * VOICES) as u64));

    // Algo 5 (three parallel 2-stacks) gives a typical edge density: 3
    // modulator edges + 3 carriers. Mirrors the bench host's representative
    // FM patch.
    g.bench_function("voice_steady", |b| {
        let mut voices = build_voices(5);
        b.iter(|| black_box(render_steady(black_box(&mut voices))))
    });

    g.bench_function("voice_release", |b| {
        let mut voices = build_voices(5);
        b.iter(|| black_box(render_release(black_box(&mut voices))))
    });

    g.finish();
}

criterion_group!(benches, bench_voice);
criterion_main!(benches);
