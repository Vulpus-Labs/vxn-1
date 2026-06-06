//! FDN reverb FX cost (ticket 0011).
//!
//! - `reverb_steady` — active reverb processing one stereo sample at a time.
//!   Steady-state cost (size smoother converged, buffer warm).
//! - `reverb_bypassed` — same call site with `on = false`. Pass-through
//!   establishes the bypass overhead floor.
//!
//! Throughput reports stereo *samples per second*, compared against the
//! sample rate this gives an RT factor (`thrpt / SR`).

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use vxn2_dsp::reverb::{FdnReverb, FdnReverbParams};

const SR: f32 = 48_000.0;
const BLK: usize = 256;

fn build(on: bool) -> FdnReverb {
    let mut r = FdnReverb::new(SR);
    let p = FdnReverbParams {
        on,
        size: 0.55,
        decay_secs: 2.4,
        damp: 0.5,
        mix: 0.2,
    };
    r.set_params(&p);
    if on {
        // Warm the ring + settle the size smoother past its 500 ms glide.
        for n in 0..(SR as usize) {
            let s = (n as f32 * 0.005).sin();
            let _ = r.process(s, s * 0.5);
        }
    }
    r
}

fn drive(r: &mut FdnReverb, block: &[(f32, f32)]) -> (f32, f32) {
    let mut acc = (0.0_f32, 0.0_f32);
    for &(l, rr) in block {
        let (ol, or_) = r.process(l, rr);
        acc.0 += ol;
        acc.1 += or_;
    }
    acc
}

fn bench_reverb(c: &mut Criterion) {
    let block: Vec<(f32, f32)> = (0..BLK)
        .map(|n| {
            let t = n as f32 / SR;
            (
                (t * 440.0 * std::f32::consts::TAU).sin() * 0.5,
                (t * 220.0 * std::f32::consts::TAU).sin() * 0.5,
            )
        })
        .collect();

    let mut g = c.benchmark_group("reverb");
    g.throughput(Throughput::Elements(BLK as u64));

    g.bench_function("reverb_steady", |b| {
        let mut r = build(true);
        b.iter(|| black_box(drive(black_box(&mut r), black_box(&block))));
    });

    g.bench_function("reverb_bypassed", |b| {
        let mut r = build(false);
        b.iter(|| black_box(drive(black_box(&mut r), black_box(&block))));
    });

    g.finish();
}

criterion_group!(benches, bench_reverb);
criterion_main!(benches);
