//! LFO control-rate cost (ticket 0006).
//!
//! `lfo_block_eval` — worst-case patch-level evaluation: 16 [`Lfo2Stack`]
//! instances (one per polyphony slot) plus 1 [`Lfo1`], each advanced one
//! control block. That's 16 × 8 = 128 LFO2 lane evaluations + 1 LFO1 per
//! `iter`.
//!
//! Throughput = total LFO sub-evaluations per call (129). Reported per-second
//! divided by the block size at 48 kHz gives an instinctive "% of one core"
//! figure (LFOs are control-rate, so the cost is amortised across the block).

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use vxn2_dsp::lfo::{Lfo1, Lfo1Params, Lfo2Params, Lfo2Stack, Lfo2Trig, LfoShape};

const SR: f32 = 48_000.0;
const BLK: usize = 256;
const N_STACKS: usize = 16;

fn build_lfo1() -> (Lfo1, Lfo1Params) {
    let params = Lfo1Params {
        shape: LfoShape::Sine,
        rate_hz: 2.4,
        sync: false,
        sync_index: 0,
    };
    (Lfo1::new(0xCAFE_F00D_1234_5678), params)
}

fn build_lfo2_stacks() -> ([Lfo2Stack; N_STACKS], Lfo2Params) {
    let params = Lfo2Params {
        shape: LfoShape::SawUp,
        rate_hz: 5.1,
        delay_ms: 0.0,
        fade_ms: 0.0,
        trig: Lfo2Trig::Free,
    };
    let mut stacks = [Lfo2Stack::default(); N_STACKS];
    for (i, lfo) in stacks.iter_mut().enumerate() {
        lfo.reseed(0xDEAD_BEEF_DEAD_BEEFu64.wrapping_add(i as u64));
        lfo.note_on(&params);
    }
    (stacks, params)
}

fn render(
    lfo1: &mut Lfo1,
    lfo1_p: &Lfo1Params,
    stacks: &mut [Lfo2Stack; N_STACKS],
    lfo2_p: &Lfo2Params,
    block_secs: f32,
) -> f32 {
    let mut acc = lfo1.eval(lfo1_p, 120.0, block_secs);
    for s in stacks.iter_mut() {
        let lanes = s.eval(lfo2_p, block_secs);
        for v in lanes {
            acc += v;
        }
    }
    acc
}

fn bench_lfo(c: &mut Criterion) {
    let block_secs = BLK as f32 / SR;
    let mut g = c.benchmark_group("lfo");
    // 16 stacks × 8 lanes + 1 LFO1 = 129 LFO sub-evals per iter.
    g.throughput(Throughput::Elements(129));
    g.bench_function("lfo_block_eval", |b| {
        let (mut lfo1, lfo1_p) = build_lfo1();
        let (mut stacks, lfo2_p) = build_lfo2_stacks();
        b.iter(|| {
            black_box(render(
                black_box(&mut lfo1),
                black_box(&lfo1_p),
                black_box(&mut stacks),
                black_box(&lfo2_p),
                black_box(block_secs),
            ))
        })
    });
    g.finish();
}

criterion_group!(benches, bench_lfo);
criterion_main!(benches);
