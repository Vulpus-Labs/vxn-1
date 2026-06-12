//! Optional per-voice filter render path (E007 / ticket 0087).
//!
//! Same 16-voice × density-4 chord as `master_chain`, FX on. Measures the
//! filter-OFF bypass against the filter-ON stack-major path at each oversample
//! factor, so the per-voice ladder + resampler cost is readable directly.
//!
//! - `filter_off` — the unchanged sample-major loop (bypass; should match the
//!   pre-E007 floor).
//! - `filter_on_1x` — ladder runs, no oversampling (interp/decimate are 1×
//!   passthrough). Isolates the bare ladder cost.
//! - `filter_on_2x/4x/8x` — adds the interpolate → ladder@F → decimate cost.
//!
//! Recorded figures (Apple M-series, 48 kHz, 256-sample block ⇒ a 5.333 ms
//! real-time budget; full poly = 16 voices × density 4 = 64 op-voice instances,
//! FX on — the heaviest steady state). RT-multiple = 5333 µs / measured median,
//! alongside the existing dry/sync/idle numbers:
//!
//! | path                | median   | × real-time |
//! |---------------------|----------|-------------|
//! | `filter_off`        | 286 µs   | 18.6×       |
//! | `filter_on_1x`      | 521 µs   | 10.2×       |
//! | `filter_on_2x`      | 812 µs   |  6.6×       |
//! | `filter_on_4x`      | 1.21 ms  |  4.4×       |
//! | `filter_on_8x`      | 2.20 ms  |  2.4×       |
//!
//! Off-path cost is within noise of the pre-epic full-poly floor; every factor
//! stays real-time at full poly. Quiescence-skip saving (4× setting): a held
//! chord costs ~1.24 ms (4.3×), the same chord released + fully rung out drops
//! to ~12 µs — the skip reclaims essentially the whole filter cost once voices
//! settle.

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use vxn2_engine::engine::Engine;
use vxn2_engine::params::id_of;
use vxn2_engine::shared::SharedParams;

const SR: f32 = 48_000.0;
const BLK: usize = 256;
const N_NOTES: usize = 16;

/// `os_idx`: None ⇒ filter off; Some(0..=3) ⇒ on at 1×/2×/4×/8×.
fn build_engine(os_idx: Option<u32>) -> Engine {
    let s = SharedParams::new();
    s.set(id_of("stack-density").unwrap(), 4.0);
    if let Some(idx) = os_idx {
        s.set(id_of("filter-enable").unwrap(), 1.0);
        s.set(id_of("filter-oversample").unwrap(), idx as f32);
        // A musical, mid-resonance lowpass so the ladder does real work.
        s.set(id_of("filter-cutoff").unwrap(), 2000.0);
        s.set(id_of("filter-resonance").unwrap(), 0.4);
    }
    let mut e = Engine::new(SR, BLK);
    e.snapshot_params(&s);

    let notes: [u8; N_NOTES] = [36, 40, 43, 47, 50, 52, 55, 57, 60, 62, 64, 67, 69, 72, 74, 76];
    for &n in &notes {
        e.note_on(n, 100);
    }
    // Settle FX + filter state.
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

fn bench_filter(c: &mut Criterion) {
    let mut g = c.benchmark_group("filter_path");
    g.throughput(Throughput::Elements(BLK as u64));

    for (name, os) in [
        ("filter_off", None),
        ("filter_on_1x", Some(0)),
        ("filter_on_2x", Some(1)),
        ("filter_on_4x", Some(2)),
        ("filter_on_8x", Some(3)),
    ] {
        g.bench_function(name, |b| {
            let mut e = build_engine(os);
            let mut l = [0.0_f32; BLK];
            let mut r = [0.0_f32; BLK];
            b.iter(|| black_box(drive(black_box(&mut e), &mut l, &mut r)));
        });
    }

    g.finish();
}

/// Quiescence-skip saving (ticket 0085 / 0087). The skip is not a runtime
/// toggle — it engages automatically once a stack goes idle *and* its ladder
/// has rung out — so the saving is read as the delta between two steady states
/// at the same 4× filter setting:
///
/// - `sustaining` — the full 16-note chord held, every stack active, so every
///   voice pays the upsample → ladder@4× → bus cost.
/// - `released_rungout` — the same chord released and left to ring out fully, so
///   every stack is idle + quiescent and skipped. This is the cost the skip
///   reclaims; it should fall back toward the idle floor.
///
/// The gap between the two is the per-block cost the quiescence-skip saves when
/// a held chord is let go.
fn bench_quiescence(c: &mut Criterion) {
    let mut g = c.benchmark_group("filter_quiescence");
    g.throughput(Throughput::Elements(BLK as u64));

    // Sustaining: build_engine already warms 0.5 s with the chord held.
    g.bench_function("sustaining_4x", |b| {
        let mut e = build_engine(Some(2));
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        b.iter(|| black_box(drive(black_box(&mut e), &mut l, &mut r)));
    });

    // Released + rung out: release every note, then render ~2 s so the resonant
    // tails fully decay below the quiescence floor and every stack is skipped.
    g.bench_function("released_rungout_4x", |b| {
        let mut e = build_engine(Some(2));
        let notes: [u8; N_NOTES] = [36, 40, 43, 47, 50, 52, 55, 57, 60, 62, 64, 67, 69, 72, 74, 76];
        for &n in &notes {
            e.note_off(n);
        }
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        for _ in 0..(2 * SR as usize) / BLK {
            e.process_block(&mut l, &mut r);
        }
        b.iter(|| black_box(drive(black_box(&mut e), &mut l, &mut r)));
    });

    g.finish();
}

criterion_group!(benches, bench_filter, bench_quiescence);
criterion_main!(benches);
