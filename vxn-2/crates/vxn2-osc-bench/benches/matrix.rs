//! Mod matrix evaluation cost (ticket 0008).
//!
//! Two scenarios at one block (control rate) per iteration, one stack:
//!
//! - `matrix_eval_full` — all 16 slots active, every source / dest distinct,
//!   curve mix across the four kinds. Worst-case per-slot path.
//! - `matrix_eval_empty` — all 16 slots `None`. Slot loop short-circuits at
//!   the first match; only the per-lane accumulator clear runs.
//!
//! Per ticket: empty case should be near-free relative to full.
//!
//! Each iter is: `eval_sources` (fan patch + stack + lane sources into the
//! 8-lane × N_SOURCES lookup) + `eval_dests` (walk slots, accumulate into the
//! 8-lane × N_DESTS accumulator). Throughput = active slot evaluations per
//! call (16 for full, 0 for empty — `Elements` is mostly cosmetic for empty).

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use vxn2_dsp::stack::STACK_LANES;
use vxn2_engine::matrix::{
    CurveKind, DestId, LaneDestVals, LaneSourceVals, LaneSources, MatrixSlot, MatrixTable,
    N_DESTS, N_SOURCES, N_SLOTS, PatchSources, SourceId, StackScalarSources, eval_dests,
    eval_sources,
};

fn build_patch_sources() -> PatchSources {
    PatchSources {
        lfo1: 0.4,
        mod_wheel: 0.6,
        aftertouch: 0.2,
    }
}

fn build_stack_sources() -> StackScalarSources {
    StackScalarSources {
        pitch_eg: 0.8,
        mod_env: 0.5,
        velocity: 0.75,
        key: 0.45,
    }
}

fn build_lane_sources() -> LaneSources {
    let mut lanes = LaneSources::default();
    for k in 0..STACK_LANES {
        lanes.lfo2[k] = -1.0 + (k as f32) * (2.0 / (STACK_LANES as f32 - 1.0));
        lanes.voice_idx[k] = (k as f32) / (STACK_LANES as f32 - 1.0);
        lanes.voice_spread[k] = -1.0 + (k as f32) * (2.0 / (STACK_LANES as f32 - 1.0));
        lanes.voice_rand[k] = (k as f32) * 0.119;
    }
    lanes
}

/// Full table: 16 distinct (source, dest) pairings across every curve kind.
/// Sources and dests cycle so different lanes hit different code paths in
/// each curve arm.
fn full_table() -> MatrixTable {
    let sources = [
        SourceId::Lfo1,
        SourceId::Lfo2,
        SourceId::PitchEg,
        SourceId::ModEnv,
        SourceId::ModWheel,
        SourceId::Aftertouch,
        SourceId::Velocity,
        SourceId::Key,
        SourceId::VoiceIdx,
        SourceId::VoiceSpread,
        SourceId::VoiceRand,
        SourceId::Lfo1,
        SourceId::Lfo2,
        SourceId::PitchEg,
        SourceId::ModEnv,
        SourceId::ModWheel,
    ];
    let dests = [
        DestId::Op1Pitch,
        DestId::Op1Level,
        DestId::Op2Pitch,
        DestId::Op2Level,
        DestId::Op3Pan,
        DestId::Op4Pan,
        DestId::Op5Level,
        DestId::Op6Pitch,
        DestId::GlobalPitch,
        DestId::Lfo1Rate,
        DestId::Lfo2Rate,
        DestId::Lfo2Phase,
        DestId::StackDetune,
        DestId::StackSpread,
        DestId::DelayMix,
        DestId::ReverbMix,
    ];
    let curves = [CurveKind::Lin, CurveKind::Exp, CurveKind::Log, CurveKind::Bipolar];
    let mut table = MatrixTable::default();
    for i in 0..N_SLOTS {
        table.slots[i] = MatrixSlot {
            source: sources[i],
            dest: dests[i],
            depth: 0.5,
            curve: curves[i % 4],
        };
    }
    table
}

fn bench_matrix(c: &mut Criterion) {
    let mut g = c.benchmark_group("matrix");

    let patch = build_patch_sources();
    let stack = build_stack_sources();
    let lanes = build_lane_sources();

    g.throughput(Throughput::Elements(N_SLOTS as u64));
    g.bench_function("matrix_eval_full", |b| {
        let table = full_table();
        let mut src_buf: LaneSourceVals = [[0.0; N_SOURCES]; STACK_LANES];
        let mut dest_buf: LaneDestVals = [[0.0; N_DESTS]; STACK_LANES];
        b.iter(|| {
            eval_sources(
                black_box(&patch),
                black_box(&stack),
                black_box(&lanes),
                &mut src_buf,
            );
            eval_dests(black_box(&table), &src_buf, &mut dest_buf);
            black_box(&dest_buf);
        })
    });

    g.throughput(Throughput::Elements(1));
    g.bench_function("matrix_eval_empty", |b| {
        let table = MatrixTable::default();
        let mut src_buf: LaneSourceVals = [[0.0; N_SOURCES]; STACK_LANES];
        let mut dest_buf: LaneDestVals = [[0.0; N_DESTS]; STACK_LANES];
        b.iter(|| {
            eval_sources(
                black_box(&patch),
                black_box(&stack),
                black_box(&lanes),
                &mut src_buf,
            );
            eval_dests(black_box(&table), &src_buf, &mut dest_buf);
            black_box(&dest_buf);
        })
    });

    g.finish();
}

criterion_group!(benches, bench_matrix);
criterion_main!(benches);
