//! Single-op cost via the promoted `vxn2_dsp::op` scalar core.
//!
//! Two scenarios:
//! - `op_voice_steady`  — held note in Sustain, EG level cached, no PM. Hot
//!   path is `op_tick` only. Block-rate EG tick included.
//! - `op_voice_attack`  — fresh note-on each block, EG ticks through Attack
//!   into Decay1. Stresses the EG state machine + cook path under transients.
//!
//! Throughput = samples per call (BLOCK × VOICES). This isolates the cost of
//! EG mul + per-op feedback memory + the scalar shape on top of the raw sine
//! reader (`vxn2_dsp::sine::scalar::fast_sine_q32`).

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use vxn2_dsp::op::{OpParams, OpState, op_eg_tick, op_tick};

const BLOCK: usize = 256;
const VOICES: usize = 32;
const SR: f32 = 48_000.0;

fn build_voices() -> [OpState; VOICES] {
    let params = OpParams::default();
    let mut voices = [OpState::default(); VOICES];
    for (i, v) in voices.iter_mut().enumerate() {
        // Spread keys across 4 octaves so phase increments differ per voice
        // and the EG cook isn't a uniform broadcast.
        let key = 48 + (i as u8 * 2) % 48;
        v.cook(&params, key, 100, SR);
        // Mid feedback exercises the FB averaging path. Feedback is now
        // layer-level; the bench harness writes it onto the op directly.
        v.fb_scale = vxn2_dsp::tables::fb_scale(4.0);
        // Decorrelate phase so the optimiser can't collapse the loop.
        v.phase = (i as u32).wrapping_mul(0x1234_5678);
    }
    voices
}

fn render_steady(voices: &mut [OpState; VOICES]) -> f32 {
    let dt_block = BLOCK as f32 / SR;
    let mut acc = 0.0;
    for v in voices.iter_mut() {
        v.force_sustain(0.6 + 0.2 * (v.phase as f32 / u32::MAX as f32));
    }
    op_eg_tick(&mut voices[0], dt_block); // single block-rate tick; sustain is no-op anyway.
    for _ in 0..BLOCK {
        for v in voices.iter_mut() {
            acc += op_tick(v, 0.0);
        }
    }
    acc
}

fn render_attack(voices: &mut [OpState; VOICES]) -> f32 {
    let dt_block = BLOCK as f32 / SR;
    let mut acc = 0.0;
    for v in voices.iter_mut() {
        v.eg.note_on();
    }
    op_eg_tick(&mut voices[0], dt_block);
    for v in voices.iter_mut() {
        op_eg_tick(v, dt_block);
    }
    for _ in 0..BLOCK {
        for v in voices.iter_mut() {
            acc += op_tick(v, 0.0);
        }
    }
    acc
}

fn bench_op_voice(c: &mut Criterion) {
    let mut g = c.benchmark_group("op_voice");
    g.throughput(Throughput::Elements((BLOCK * VOICES) as u64));

    g.bench_function("op_voice_steady", |b| {
        let mut voices = build_voices();
        b.iter(|| black_box(render_steady(black_box(&mut voices))))
    });

    g.bench_function("op_voice_attack", |b| {
        let mut voices = build_voices();
        b.iter(|| black_box(render_attack(black_box(&mut voices))))
    });

    g.finish();
}

criterion_group!(benches, bench_op_voice);
criterion_main!(benches);
