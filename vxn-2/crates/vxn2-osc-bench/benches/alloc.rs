//! Allocator cost (ticket 0004). Two scenarios:
//!
//! - `alloc_held_chord` — 8 notes held, render N audio blocks. Steady-state
//!   cost of the alloc lifecycle (block_tick + per-stack tick) when no event
//!   churn is happening.
//! - `alloc_steal_churn` — note-on/off cycling beyond polyphony cap. Stresses
//!   `pick_slot` (steal heuristic), `most_recent_held` bookkeeping, and the
//!   sequence counter.
//!
//! Density is fixed at 1 in this bench — stack-density scaling lives in the
//! `stack` bench (ticket 0005).
//!
//! Throughput = samples rendered per call (BLOCK × N_BLOCKS).

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use vxn2_dsp::algo::N_OPS;
use vxn2_dsp::op::OpParams;
use vxn2_dsp::stack::{StackDistrib, StackParams, stack_tick_mono};
use vxn2_dsp::voice::VoiceParams;
use vxn2_engine::alloc::{AllocParams, AssignMode, N_STACKS, PolyAlloc};

const BLOCK: usize = 64;
const N_BLOCKS: usize = 32;
const SR: f32 = 48_000.0;
const BLOCK_DT: f32 = BLOCK as f32 / SR;

fn patch(algo: u8) -> VoiceParams {
    let ops = [OpParams::default(); N_OPS];
    VoiceParams {
        ops,
        algo,
        feedback: 2,
        ..VoiceParams::default()
    }
}

fn density1() -> StackParams {
    StackParams {
        density: 1,
        detune_cents_max: 0.0,
        spread: 0.0,
        phase: 0.0,
        distrib: StackDistrib::Linear,
    }
}

fn render_blocks(alloc: &mut PolyAlloc, blocks: usize) -> f32 {
    let mut acc = 0.0;
    for _ in 0..blocks {
        alloc.block_tick(BLOCK_DT);
        for s in &mut alloc.stacks {
            s.eg_tick(BLOCK_DT);
            for _ in 0..BLOCK {
                acc += stack_tick_mono(s);
            }
        }
    }
    acc
}

fn bench_alloc(c: &mut Criterion) {
    let mut g = c.benchmark_group("alloc");
    g.throughput(Throughput::Elements((BLOCK * N_BLOCKS) as u64));

    g.bench_function("alloc_held_chord", |b| {
        let vp = patch(5);
        let sp = density1();
        let params = AllocParams::default();
        let mut alloc = PolyAlloc::new(SR);
        for &n in &[48u8, 50, 52, 55, 57, 60, 62, 64] {
            alloc.note_on(&params, &sp, &vp, n, 100);
        }
        render_blocks(&mut alloc, 4);
        b.iter(|| black_box(render_blocks(black_box(&mut alloc), N_BLOCKS)))
    });

    g.bench_function("alloc_steal_churn", |b| {
        let vp = patch(5);
        let sp = density1();
        let params = AllocParams::default();
        let mut alloc = PolyAlloc::new(SR);
        let mut churn_seq = 0u32;
        b.iter(|| {
            let base = 36 + ((churn_seq as u8) & 0x1F);
            churn_seq = churn_seq.wrapping_add(1);
            for i in 0..(N_STACKS as u8 + 1) {
                alloc.note_on(&params, &sp, &vp, base + i, 100);
            }
            let r = black_box(render_blocks(black_box(&mut alloc), N_BLOCKS));
            for i in 0..(N_STACKS as u8 + 1) {
                alloc.note_off(&params, &sp, &vp, base + i);
            }
            r
        })
    });

    let _ = AssignMode::Poly;
    g.finish();
}

criterion_group!(benches, bench_alloc);
criterion_main!(benches);
