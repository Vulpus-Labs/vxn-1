//! Delay FX cost (ticket 0010).
//!
//! Two scenarios:
//!
//! - `delay_steady` — active delay processing one stereo sample at a time.
//!   Steady-state cost (smoother glide already converged, buffer warm).
//! - `delay_bypassed` — same call site with `on = false`. Pass-through
//!   establishes the bypass overhead floor (should be ~zero — just the
//!   condition + load of the input).
//!
//! Throughput reports stereo *samples per second*, which compared against
//! the sample rate gives an RT factor (`thrpt / SR`).

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use vxn2_dsp::delay::{StereoDelay, StereoDelayParams};

const SR: f32 = 48_000.0;
const BLK: usize = 256;

fn build(on: bool) -> StereoDelay {
    let mut d = StereoDelay::new(SR);
    let p = StereoDelayParams {
        on,
        time_ms: 250.0,
        sync: false,
        sync_index: 0,
        feedback: 0.45,
        mix: 0.25,
        pingpong: false,
    };
    d.set_params(&p, 120.0);
    if on {
        // Warm the buffer + settle the time-smoother past its 100 ms glide.
        for n in 0..(SR as usize) {
            let s = (n as f32 * 0.005).sin();
            let _ = d.process(s, s * 0.5);
        }
    }
    d
}

fn drive(d: &mut StereoDelay, block: &[(f32, f32)]) -> (f32, f32) {
    let mut acc = (0.0_f32, 0.0_f32);
    for &(l, r) in block {
        let (ol, or_) = d.process(l, r);
        acc.0 += ol;
        acc.1 += or_;
    }
    acc
}

fn bench_delay(c: &mut Criterion) {
    let block: Vec<(f32, f32)> = (0..BLK)
        .map(|n| {
            let t = n as f32 / SR;
            (
                (t * 440.0 * std::f32::consts::TAU).sin() * 0.5,
                (t * 220.0 * std::f32::consts::TAU).sin() * 0.5,
            )
        })
        .collect();

    let mut g = c.benchmark_group("delay");
    g.throughput(Throughput::Elements(BLK as u64));

    g.bench_function("delay_steady", |b| {
        let mut d = build(true);
        b.iter(|| black_box(drive(black_box(&mut d), black_box(&block))));
    });

    g.bench_function("delay_bypassed", |b| {
        let mut d = build(false);
        b.iter(|| black_box(drive(black_box(&mut d), black_box(&block))));
    });

    g.finish();
}

criterion_group!(benches, bench_delay);
criterion_main!(benches);
