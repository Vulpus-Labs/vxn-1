//! Pre-FX cleanup-filter cost (ticket 0021).
//!
//! `cleanup_steady` — sustained 4 s sine burst through the stereo HPF+LPF
//! chain. Expected ~negligible: four mul-add per sample stereo.
//!
//! Throughput reports stereo *samples per second*; divide by the sample
//! rate for the RT factor.

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use vxn2_dsp::cleanup::CleanupFilter;

const SR: f32 = 48_000.0;
const BLK: usize = 256;

fn drive(f: &mut CleanupFilter, block: &[(f32, f32)]) -> (f32, f32) {
    let mut acc = (0.0_f32, 0.0_f32);
    for &(l, r) in block {
        let (ol, or_) = f.process(l, r);
        acc.0 += ol;
        acc.1 += or_;
    }
    acc
}

fn bench_cleanup(c: &mut Criterion) {
    // 4 s of stereo sine — slow enough that HPF/LPF transients have settled
    // and we're measuring steady-state cost only.
    let n = (SR as usize) * 4;
    let signal: Vec<(f32, f32)> = (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            (
                (t * 440.0 * std::f32::consts::TAU).sin() * 0.5,
                (t * 220.0 * std::f32::consts::TAU).sin() * 0.5,
            )
        })
        .collect();

    let mut g = c.benchmark_group("cleanup");
    g.throughput(Throughput::Elements(BLK as u64));

    g.bench_function("cleanup_steady", |b| {
        let mut f = CleanupFilter::new(SR);
        // Warm past HPF transient.
        for &(l, r) in signal.iter().take((SR * 0.1) as usize) {
            let _ = f.process(l, r);
        }
        let block: Vec<(f32, f32)> = signal[..BLK].to_vec();
        b.iter(|| black_box(drive(black_box(&mut f), black_box(&block))));
    });

    g.finish();
}

criterion_group!(benches, bench_cleanup);
criterion_main!(benches);
