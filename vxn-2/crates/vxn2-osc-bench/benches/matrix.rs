//! Mod matrix evaluation cost.
//!
//! Two scenarios at one block (control rate) per iteration, one stack:
//!
//! - `matrix_eval_full` — all 16 slots active, every source / dest distinct,
//!   curve mix across the four kinds. Worst-case per-slot path.
//! - `matrix_eval_scaled` — same 16 slots, but every one carries a secondary
//!   scale source and a scale bend, so the per-lane VCA loop runs too. The
//!   delta against `matrix_eval_full` is the whole cost of the scale path.
//! - `matrix_eval_empty` — all 16 slots `None`. Slot loop short-circuits at
//!   the first match; only the per-lane accumulator clear runs.
//!
//! Empty case should be near-free relative to full. Throughput = active slot
//! evaluations per call (16 for full, 0 for empty — `Elements` is mostly
//! cosmetic for empty).

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use vxn2_dsp::stack::STACK_LANES;
use vxn2_engine::matrix::{
    DestId, Polarity, Shape, LaneDestVals, LaneSourceVals, LaneSources, MatrixSlot, MatrixTable,
    N_DESTS, N_SOURCES, N_SLOTS, PatchSources, RouteList, SourceId, StackScalarSources, eval_dests,
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
    // One of each polarity plus the shape roster — keeps the bench walking
    // every dispatch arm rather than a single hot one.
    let curves = [
        (Polarity::Direct, Shape::Lin),
        (Polarity::Direct, Shape::Exp),
        (Polarity::Abs, Shape::Log),
        (Polarity::Bipolar, Shape::Lin),
    ];
    let mut table = MatrixTable::default();
    for i in 0..N_SLOTS {
        table.slots[i] = MatrixSlot {
            source: sources[i],
            dest: dests[i],
            depth: 0.5,
            polarity: curves[i % 4].0,
            shape: curves[i % 4].1,
            scale_src: SourceId::None,
            scale_shape: Shape::Lin,
            enabled: true,
        };
    }
    table
}

/// Same table, but every slot gated by a secondary scale source with a bend.
/// Scale sources cycle through both polarities (bipolar `lfo1` / `pitch_eg`
/// fold, unipolar `velocity` / `mod_wheel` pass through) and all three bends,
/// so no single scale path stays hot across the table.
fn scaled_table() -> MatrixTable {
    let scale_srcs = [
        SourceId::Velocity,
        SourceId::Lfo1,
        SourceId::ModWheel,
        SourceId::PitchEg,
    ];
    let scale_shapes = [Shape::Lin, Shape::Exp, Shape::Log];
    let mut table = full_table();
    for i in 0..N_SLOTS {
        table.slots[i].scale_src = scale_srcs[i % scale_srcs.len()];
        table.slots[i].scale_shape = scale_shapes[i % scale_shapes.len()];
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
        let mut src_buf: LaneSourceVals = [[0.0; STACK_LANES]; N_SOURCES];
        let mut dest_buf: LaneDestVals = [[0.0; STACK_LANES]; N_DESTS];
        b.iter(|| {
            eval_sources(
                black_box(&patch),
                black_box(&stack),
                black_box(&lanes),
                &mut src_buf,
            );
            eval_dests(
                &RouteList::compile(black_box(&table)),
                &src_buf,
                &mut dest_buf,
            );
            black_box(&dest_buf);
        })
    });

    g.throughput(Throughput::Elements(N_SLOTS as u64));
    g.bench_function("matrix_eval_scaled", |b| {
        let table = scaled_table();
        let mut src_buf: LaneSourceVals = [[0.0; STACK_LANES]; N_SOURCES];
        let mut dest_buf: LaneDestVals = [[0.0; STACK_LANES]; N_DESTS];
        b.iter(|| {
            eval_sources(
                black_box(&patch),
                black_box(&stack),
                black_box(&lanes),
                &mut src_buf,
            );
            eval_dests(
                &RouteList::compile(black_box(&table)),
                &src_buf,
                &mut dest_buf,
            );
            black_box(&dest_buf);
        })
    });

    g.throughput(Throughput::Elements(1));
    g.bench_function("matrix_eval_empty", |b| {
        let table = MatrixTable::default();
        let mut src_buf: LaneSourceVals = [[0.0; STACK_LANES]; N_SOURCES];
        let mut dest_buf: LaneDestVals = [[0.0; STACK_LANES]; N_DESTS];
        b.iter(|| {
            eval_sources(
                black_box(&patch),
                black_box(&stack),
                black_box(&lanes),
                &mut src_buf,
            );
            eval_dests(
                &RouteList::compile(black_box(&table)),
                &src_buf,
                &mut dest_buf,
            );
            black_box(&dest_buf);
        })
    });

    g.finish();
}

criterion_group!(benches, bench_matrix);
criterion_main!(benches);
