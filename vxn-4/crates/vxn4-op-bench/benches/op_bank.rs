//! Criterion side of the vxn-4 operator-block sizing bench.
//!
//! Four groups, one per open question:
//!
//! - `size_os`  — mip-0 table length x oversampling factor.
//! - `layout`   — SIMD across voices vs across operators, at three widths.
//! - `lookup`   — value+slope vs plain two-load, and what the bounds check costs.
//! - `routing`  — dense vs sparse, shared vs per-voice route gains.
//!
//! Throughput is in **voice-samples at 1x**, so `Elements/s ÷ 48000` reads
//! directly as sustainable polyphony on one core. `src/bin/sweep.rs` prints
//! that division already; use criterion when you want the confidence interval
//! and the regression tracking, and the sweep when you want the decision.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

use vxn4_dsp::ops::{OpMajor, VoiceMajor, VoiceMajorPerVoiceGain};
use vxn4_dsp::wavetable::{Lookup, Plain, ValueSlope, ValueSlopeUnchecked};
use vxn4_op_bench::{BLOCK_1X, Fixture, dense_routing, keys, sparse_routing};

fn run_vm<const V: usize, L: Lookup>(fx: &Fixture, b: &mut criterion::Bencher) {
    let mut bank: VoiceMajor<V> = VoiceMajor::new();
    bank.cook(&fx.bank, &fx.cfg, &keys::<V>(), fx.sr_os());
    let (mut l, mut r) = ([0.0f32; BLOCK_1X], [0.0f32; BLOCK_1X]);
    b.iter(|| {
        bank.render::<L>(&fx.bank, &fx.compiled, &fx.bus, fx.os, &mut l, &mut r);
        black_box((&l, &r));
    });
}

fn run_om<const V: usize, L: Lookup>(fx: &Fixture, b: &mut criterion::Bencher) {
    let mut bank: OpMajor<V> = OpMajor::new();
    bank.cook(&fx.bank, &fx.cfg, &keys::<V>(), fx.sr_os());
    let (mut l, mut r) = ([0.0f32; BLOCK_1X], [0.0f32; BLOCK_1X]);
    b.iter(|| {
        bank.render::<L>(&fx.bank, &fx.compiled, &fx.bus, fx.os, &mut l, &mut r);
        black_box((&l, &r));
    });
}

fn run_pvg<const V: usize, L: Lookup>(fx: &Fixture, b: &mut criterion::Bencher) {
    let mut bank: VoiceMajorPerVoiceGain<V> = VoiceMajorPerVoiceGain::new(&fx.routing, 0.2);
    bank.cook(&fx.bank, &fx.cfg, &keys::<V>(), fx.sr_os());
    let (mut l, mut r) = ([0.0f32; BLOCK_1X], [0.0f32; BLOCK_1X]);
    b.iter(|| {
        bank.render::<L>(&fx.bank, &fx.compiled, &fx.bus, fx.os, &mut l, &mut r);
        black_box((&l, &r));
    });
}

/// The primary question: how much does 16x cost over 8x, and does a shorter
/// table pay for it by keeping the gathers in L1?
fn size_os(c: &mut Criterion) {
    let mut g = c.benchmark_group("size_os");
    g.throughput(Throughput::Elements((BLOCK_1X * 8) as u64));
    for base_len in [256usize, 512, 2048] {
        for os in [8usize, 16] {
            let fx = Fixture::new(base_len, os, dense_routing());
            g.bench_with_input(
                BenchmarkId::from_parameter(format!("len{base_len}_os{os}x")),
                &fx,
                |b, fx| run_vm::<8, ValueSlope>(fx, b),
            );
        }
    }
    g.finish();
}

/// Dense routing on both sides, so this isolates layout from sparsity.
fn layout(c: &mut Criterion) {
    let fx = Fixture::new(512, 16, dense_routing());
    let mut g = c.benchmark_group("layout");

    g.throughput(Throughput::Elements((BLOCK_1X * 4) as u64));
    g.bench_function("voice_major_v4", |b| run_vm::<4, ValueSlope>(&fx, b));
    g.bench_function("op_major_v4", |b| run_om::<4, ValueSlope>(&fx, b));

    g.throughput(Throughput::Elements((BLOCK_1X * 8) as u64));
    g.bench_function("voice_major_v8", |b| run_vm::<8, ValueSlope>(&fx, b));
    g.bench_function("op_major_v8", |b| run_om::<8, ValueSlope>(&fx, b));

    g.throughput(Throughput::Elements((BLOCK_1X * 16) as u64));
    g.bench_function("voice_major_v16", |b| run_vm::<16, ValueSlope>(&fx, b));
    g.bench_function("op_major_v16", |b| run_om::<16, ValueSlope>(&fx, b));

    g.finish();
}

/// If `checked` and `unchecked` tie, delete the `unsafe` and keep the safe
/// path — the DSP crates in this workspace carry none, and the only reason to
/// take some on would be a measured win.
fn lookup(c: &mut Criterion) {
    let fx = Fixture::new(512, 16, dense_routing());
    let mut g = c.benchmark_group("lookup");
    g.throughput(Throughput::Elements((BLOCK_1X * 8) as u64));
    g.bench_function("value_slope_checked", |b| run_vm::<8, ValueSlope>(&fx, b));
    g.bench_function("value_slope_unchecked", |b| {
        run_vm::<8, ValueSlopeUnchecked>(&fx, b)
    });
    g.bench_function("plain_two_load", |b| run_vm::<8, Plain>(&fx, b));
    g.finish();
}

/// Dense sets the floor; sparse is what patches will cost. The per-voice-gain
/// arm prices the modulation the brief actually asks for, which the shared-gain
/// kernel does not model.
fn routing(c: &mut Criterion) {
    let dense = Fixture::new(512, 16, dense_routing());
    let sparse = Fixture::new(512, 16, sparse_routing());
    let mut g = c.benchmark_group("routing");
    g.throughput(Throughput::Elements((BLOCK_1X * 8) as u64));
    g.bench_function("dense_shared_gain", |b| run_vm::<8, ValueSlope>(&dense, b));
    g.bench_function("dense_per_voice_gain", |b| {
        run_pvg::<8, ValueSlope>(&dense, b)
    });
    g.bench_function("sparse_shared_gain", |b| run_vm::<8, ValueSlope>(&sparse, b));
    g.bench_function("sparse_per_voice_gain", |b| {
        run_pvg::<8, ValueSlope>(&sparse, b)
    });
    g.finish();
}

criterion_group!(benches, size_os, layout, lookup, routing);
criterion_main!(benches);
