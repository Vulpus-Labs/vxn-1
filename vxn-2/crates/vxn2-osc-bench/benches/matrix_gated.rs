//! Gated stack-macro / LFO-rate matrix dests.
//!
//! These dests re-cook per-block state (LFO rate offset, per-lane detune) but
//! are *gated*: a block where no slot targets them must pay nothing and stay
//! bit-identical. This bench reads the used-path cost as the delta between an
//! off case (no targeting slot) and an on case, all at density-8 (the heaviest
//! unison stack — the re-cook is per-lane), full 16-note poly, FX on.
//!
//! - `baseline`         — no rate/macro slot; the gated paths are skipped.
//! - `lfo2_rate_on`     — `velocity → lfo2-rate` active: one extra `2^oct` per
//!   active stack per block (the LFO2 tick already runs).
//! - `stack_detune_on`  — `key → stack-detune` active: the heaviest case, a
//!   per-lane detune re-derive folded into the always-present `apply_pitch_mult`.

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use vxn2_engine::engine::Engine;
use vxn2_engine::matrix::{CurveKind, DestId, MatrixSlot, SourceId};
use vxn2_engine::params::id_of;
use vxn2_engine::shared::SharedParams;

const SR: f32 = 48_000.0;
const BLK: usize = 256;
const N_NOTES: usize = 16;

/// Density-8 × 16-note chord, FX on. `slot0` optionally injects a matrix slot
/// targeting one of the gated dests.
fn build_engine(slot0: Option<MatrixSlot>) -> Engine {
    let s = SharedParams::new();
    s.set(id_of("stack-density").unwrap(), 8.0);
    let mut e = Engine::new(SR, BLK);
    e.snapshot_params(&s);
    if let Some(slot) = slot0 {
        // engine.matrix is read directly each block; process_block never
        // re-snapshots, so a post-build write persists for the bench.
        e.matrix.slots[0] = slot;
    }

    let notes: [u8; N_NOTES] = [36, 40, 43, 47, 50, 52, 55, 57, 60, 62, 64, 67, 69, 72, 74, 76];
    for &n in &notes {
        e.note_on(n, 100);
    }
    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    for _ in 0..(SR as usize) / 2 / BLK {
        e.process_block(&mut l, &mut r);
    }
    e
}

fn drive(e: &mut Engine, l: &mut [f32; BLK], r: &mut [f32; BLK]) -> f32 {
    e.process_block(l, r);
    let mut acc = 0.0_f32;
    for i in 0..BLK {
        acc += l[i] + r[i];
    }
    acc
}

fn bench_gated(c: &mut Criterion) {
    let mut g = c.benchmark_group("matrix_gated");
    g.throughput(Throughput::Elements(BLK as u64));

    let lin = CurveKind::Lin;
    let cases: [(&str, Option<MatrixSlot>); 3] = [
        ("baseline", None),
        (
            "lfo2_rate_on",
            Some(MatrixSlot {
                source: SourceId::Velocity,
                dest: DestId::Lfo2Rate,
                depth: 1.0,
                curve: lin,
                scale_src: SourceId::None,
            }),
        ),
        (
            "stack_detune_on",
            Some(MatrixSlot {
                source: SourceId::Key,
                dest: DestId::StackDetune,
                depth: 1.0,
                curve: lin,
                scale_src: SourceId::None,
            }),
        ),
    ];

    for (name, slot) in cases {
        g.bench_function(name, |b| {
            let mut e = build_engine(slot);
            let mut l = [0.0_f32; BLK];
            let mut r = [0.0_f32; BLK];
            b.iter(|| black_box(drive(black_box(&mut e), &mut l, &mut r)));
        });
    }

    g.finish();
}

criterion_group!(benches, bench_gated);
criterion_main!(benches);
