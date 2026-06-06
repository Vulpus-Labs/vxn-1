//! Full master chain (ticket 0012).
//!
//! Drives the assembled engine through its steady-state hot path: 16 held
//! voices, density 4 (so 64 op-voice instances in flight), Layer-mode
//! voicing (each note allocates two stacks → 8 notes × 2 layers = 16), full
//! FX (delay + reverb on), master volume + tune applied.
//!
//! Throughput = stereo samples rendered per call. RT factor = `thrpt / SR`.
//!
//! Two scenarios:
//!
//! - `master_chain_full` — every default in place (delay + reverb on).
//! - `master_chain_fx_off` — same voices, FX bypassed. Establishes the
//!   voice-loop floor so the FX overhead is readable directly.

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use vxn2_engine::engine::Engine;
use vxn2_engine::params::id_of;
use vxn2_engine::shared::SharedParams;

const SR: f32 = 48_000.0;
const BLK: usize = 256;
const N_NOTES: usize = 8;

/// 8 notes × Layer-mode = 16 stacks. Density 4 → 16 stacks × 4 lanes = 64
/// op-voice instances in flight.
fn build_engine(fx_on: bool) -> Engine {
    let s = SharedParams::new();
    // Density 4 is the ticket-AC baseline.
    s.set(id_of("upper-stack-density").unwrap(), 4.0);
    s.set(id_of("lower-stack-density").unwrap(), 4.0);
    // Default voicing is Layer; keep it.
    if !fx_on {
        s.set(id_of("delay-on").unwrap(), 0.0);
        s.set(id_of("reverb-on").unwrap(), 0.0);
    }
    let mut e = Engine::new(SR, BLK);
    e.snapshot_params(&s);

    // Hold a sustained 8-note chord across the keyboard.
    let notes: [u8; N_NOTES] = [40, 47, 52, 55, 60, 64, 67, 72];
    for &n in &notes {
        e.note_on(n, 100);
    }

    // Warm the FX chain past delay smoother (~100 ms) + reverb size
    // smoother (~500 ms). Half-second settles both.
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

fn bench_master(c: &mut Criterion) {
    let mut g = c.benchmark_group("master_chain");
    g.throughput(Throughput::Elements(BLK as u64));

    g.bench_function("master_chain_full", |b| {
        let mut e = build_engine(true);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        b.iter(|| black_box(drive(black_box(&mut e), &mut l, &mut r)));
    });

    g.bench_function("master_chain_fx_off", |b| {
        let mut e = build_engine(false);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        b.iter(|| black_box(drive(black_box(&mut e), &mut l, &mut r)));
    });

    g.finish();
}

criterion_group!(benches, bench_master);
criterion_main!(benches);
