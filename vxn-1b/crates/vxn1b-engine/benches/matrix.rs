//! Mod matrix evaluation cost (ticket 0329).
//!
//! The vxn-1b twin of `vxn2-osc-bench/benches/matrix.rs`. vxn-1b had no
//! criterion bench for this stage — only `examples/route_profile.rs`, which
//! times a whole render and so prices the routing together with the DSP it
//! drives. E049's evaluator tickets (0334) and its close-out table (0337) need
//! the stage on its own, before and after.
//!
//! Both of vxn-1b's evaluators are measured, because they are what 0334 has to
//! keep bit-exact against each other while replacing them with a shared one:
//!
//! - `matrix_eval_full` / `matrix_eval_scaled` — the **scalar** per-voice path
//!   ([`eval_sources`] + [`eval_dests`]), one voice per iteration. The names
//!   match vxn-2's so the two synths' numbers line up in a close-out table.
//! - `matrix_bank_full` / `matrix_bank_scaled` — the **banked** path
//!   ([`RouteList::compile`] + [`sources_to_soa`] + [`eval_dests_bank`]) over a
//!   whole 8-lane bank, which is what the render loop actually runs.
//!
//! `_scaled` differs from `_full` only in that every slot carries a secondary
//! scale source and a bend, so the delta between the two is the whole cost of
//! the per-route VCA.
//!
//! Throughput is active slot evaluations per call — 16 for the scalar cases,
//! 16 × 8 lanes for the banked ones — so the reported figure is per route
//! evaluation and the scalar and banked numbers are directly comparable.

use criterion::measurement::WallTime;
use criterion::{
    BenchmarkGroup, Criterion, Throughput, black_box, criterion_group, criterion_main,
};
use vxn1b_engine::eval::{
    DestLanesSoa, DestVals, RouteList, SourceLanesSoa, SourceVals, eval_dests, eval_dests_bank,
    eval_sources, sources_to_soa,
};
use vxn1b_engine::matrix::{
    DestId, MatrixSlot, MatrixTable, N_DESTS, N_SLOTS, N_SOURCES, Polarity, Shape, SourceId,
};
use vxn1b_engine::{RenderBank, SourceInputs};

/// Lanes per bank — the width the render loop evaluates the matrix at.
const LANES: usize = RenderBank::LANES;

/// One voice's raw modulation inputs, all off their zero so no source folds to
/// a constant the optimiser could hoist.
fn inputs(lane: usize) -> SourceInputs {
    let f = lane as f32 / LANES as f32;
    SourceInputs {
        env1: 0.8 - 0.3 * f,
        env2: 0.6 + 0.2 * f,
        lfo1: -1.0 + 2.0 * f,
        lfo2: 0.35 - f,
        velocity: 0.75,
        note: 48 + lane as u8,
        mod_wheel: 0.6,
        pitch_wheel: -0.25,
        aftertouch: 0.4,
        note_random: 0.119 * lane as f32 % 1.0,
        spread_pos: -1.0 + 2.0 * f,
        stack_pos: 1.0 - 2.0 * f,
    }
}

/// Full table: all 16 slots active, every source represented, dests spread so
/// several accumulate into the same row and several do not.
fn full_table() -> MatrixTable {
    let sources = [
        SourceId::Env1,
        SourceId::Env2,
        SourceId::Lfo1,
        SourceId::Lfo2,
        SourceId::Velocity,
        SourceId::Key,
        SourceId::ModWheel,
        SourceId::PitchWheel,
        SourceId::Aftertouch,
        SourceId::NoteRandom,
        SourceId::Spread,
        SourceId::StackPos,
        SourceId::Env1,
        SourceId::Lfo1,
        SourceId::Lfo2,
        SourceId::Velocity,
    ];
    let dests = [
        DestId::Pitch,
        DestId::Cutoff,
        DestId::Amp,
        DestId::Pan,
        DestId::Pwm,
        DestId::Resonance,
        DestId::HpfCutoff,
        DestId::CrossModAmount,
        DestId::Osc1Pwm,
        DestId::Osc2Pwm,
        DestId::XModSweep,
        DestId::Lfo1Rate,
        DestId::Cutoff,
        DestId::Pitch,
        DestId::Amp,
        DestId::Pan,
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
            polarity: curves[i % curves.len()].0,
            shape: curves[i % curves.len()].1,
            enabled: true,
            scale_src: SourceId::None,
            scale_shape: Shape::Lin,
        };
    }
    table
}

/// Same table, every slot gated by a secondary scale source with a bend. The
/// scale sources cycle through both polarities (bipolar `lfo1` / `spread` fold,
/// unipolar `velocity` / `mod_wheel` pass through) and all three bends, so no
/// single scale path stays hot across the table.
fn scaled_table() -> MatrixTable {
    let scale_srcs = [
        SourceId::Velocity,
        SourceId::Lfo1,
        SourceId::ModWheel,
        SourceId::Spread,
    ];
    let scale_shapes = [Shape::Lin, Shape::Exp, Shape::Log];
    let mut table = full_table();
    for i in 0..N_SLOTS {
        table.slots[i].scale_src = scale_srcs[i % scale_srcs.len()];
        table.slots[i].scale_shape = scale_shapes[i % scale_shapes.len()];
    }
    table
}

/// The scalar per-voice path: normalise one voice's inputs, then accumulate the
/// table into its dest totals.
fn bench_scalar(
    g: &mut BenchmarkGroup<'_, WallTime>,
    name: &str,
    table: MatrixTable,
) {
    g.throughput(Throughput::Elements(N_SLOTS as u64));
    g.bench_function(name, |b| {
        let inp = inputs(3);
        let mut dest: DestVals = [0.0; N_DESTS];
        b.iter(|| {
            let src: SourceVals = eval_sources(black_box(&inp));
            eval_dests(black_box(&table), &src, &mut dest);
            black_box(&dest);
        })
    });
}

/// The banked path the render loop runs: compile the patch, transpose the
/// bank's per-lane source tables, accumulate dest-major.
fn bench_bank(
    g: &mut BenchmarkGroup<'_, WallTime>,
    name: &str,
    table: MatrixTable,
) {
    g.throughput(Throughput::Elements((N_SLOTS * LANES) as u64));
    g.bench_function(name, |b| {
        let per_lane: [SourceVals; LANES] = std::array::from_fn(|l| eval_sources(&inputs(l)));
        let mut dest: DestLanesSoa<LANES> = [[0.0; LANES]; N_DESTS];
        b.iter(|| {
            // Compiled inside the loop on purpose: the render loop recompiles
            // once per block, so its cost belongs in the block's number.
            let routes = RouteList::compile(black_box(&table));
            let src: SourceLanesSoa<LANES> = sources_to_soa(black_box(&per_lane));
            eval_dests_bank(&routes, &src, &mut dest);
            black_box(&dest);
        })
    });
}

fn bench_matrix(c: &mut Criterion) {
    // The source table is 12 wide and the dest table 16 — assert rather than
    // assume, so a roster change shows up here as a failure rather than as a
    // quietly different number.
    assert_eq!((N_SOURCES, N_DESTS, N_SLOTS), (12, 16, 16));

    let mut g = c.benchmark_group("matrix");
    bench_scalar(&mut g, "matrix_eval_full", full_table());
    bench_scalar(&mut g, "matrix_eval_scaled", scaled_table());
    bench_bank(&mut g, "matrix_bank_full", full_table());
    bench_bank(&mut g, "matrix_bank_scaled", scaled_table());
    g.finish();
}

criterion_group!(benches, bench_matrix);
criterion_main!(benches);
