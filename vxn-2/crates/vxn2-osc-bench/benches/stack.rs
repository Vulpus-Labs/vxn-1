//! Stack hot-path cost (ticket 0005).
//!
//! Three scenarios — same 4-note chord, sustained, three stack densities:
//!
//! - `stack_d1` — density 1: one op-voice per note. Equivalent op count to
//!   the existing `voice_steady` bench but exercising the lane-packed code
//!   path with 7 silent lanes (overhead of always-on SoA).
//! - `stack_d4` — density 4: 16 op-voice instances per note (4 lanes × 4
//!   notes). Typical thick-pad use.
//! - `stack_d8` — density 8: full lane width, 32 op-voice instances per
//!   note. Worst-case SIMD-fully-loaded path.
//!
//! Throughput = samples × stack-lanes rendered per call. Per ticket 0005
//! acceptance, `d4` should be < 4× `d1` cost and `d8` < 8× — i.e. sub-linear
//! scaling thanks to lane packing.

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use vxn2_dsp::algo::N_OPS;
use vxn2_dsp::op::OpParams;
use vxn2_dsp::stack::{
    STACK_LANES, Stack, StackDistrib, StackParams, stack_tick_stereo,
};
use vxn2_dsp::voice::VoiceParams;

const BLOCK: usize = 256;
const N_NOTES: usize = 4;
const SR: f32 = 48_000.0;

fn fm_patch(algo: u8) -> VoiceParams {
    let mut ops = [OpParams::default(); N_OPS];
    for op in &mut ops {
        op.feedback = 2;
    }
    VoiceParams {
        ops,
        algo,
        ..VoiceParams::default()
    }
}

fn build_stacks(density: u8, algo: u8) -> [Stack; N_NOTES] {
    let vp = fm_patch(algo);
    let sp = StackParams {
        density,
        detune_cents_max: 8.0,
        spread: 0.60,
        phase: 0.50,
        distrib: StackDistrib::Linear,
    };
    let mut stacks = [Stack::default(); N_NOTES];
    for (i, s) in stacks.iter_mut().enumerate() {
        let key = 48 + (i as u8 * 5);
        s.note_on(&sp, &vp, key, 100, SR, i as u64);
        s.force_sustain(0.4);
    }
    stacks
}

fn render(stacks: &mut [Stack; N_NOTES]) -> f32 {
    let dt_block = BLOCK as f32 / SR;
    for s in stacks.iter_mut() {
        s.eg_tick(dt_block);
    }
    let mut acc = 0.0;
    for _ in 0..BLOCK {
        for s in stacks.iter_mut() {
            let (l, r) = stack_tick_stereo(s);
            acc += l + r;
        }
    }
    acc
}

fn bench_stack(c: &mut Criterion) {
    let mut g = c.benchmark_group("stack");
    // Algo 5 (three 2-stacks, 3 carriers, 3 mod edges) — representative.
    let algo = 5;

    for &density in &[1u8, 4, 8] {
        let throughput = (BLOCK * N_NOTES * density as usize) as u64;
        let name = format!("stack_d{density}");
        g.throughput(Throughput::Elements(throughput));
        g.bench_function(&name, |b| {
            let mut stacks = build_stacks(density, algo);
            b.iter(|| black_box(render(black_box(&mut stacks))))
        });
    }

    let _ = STACK_LANES;
    g.finish();
}

criterion_group!(benches, bench_stack);
criterion_main!(benches);
