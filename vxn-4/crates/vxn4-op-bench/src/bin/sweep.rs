//! Sizing sweep for the vxn-4 operator block.
//!
//! Criterion answers "how long does this take"; this binary answers the
//! question the brief actually poses — **how many voices fit in one core** —
//! and prints it as a table you can read a decision off.
//!
//! ```text
//! cargo run --release -p vxn4-op-bench --bin sweep
//! ```
//!
//! Polyphony is quoted at 100% of one core, which no plugin may actually use.
//! Divide by whatever headroom factor the host budget assumes; the comparison
//! between configurations is what matters here, not the absolute.
//!
//! ## Measurement
//!
//! Best-of-[`REPS`] per point, after a [`spin_up`] busy loop and a per-point
//! warmup. Both are load-bearing on this platform: without the spin-up the
//! first point measured came in ~25% low against an identical later point,
//! because the core had not clocked up yet — which read as "a 256-entry table
//! is slower than a 512-entry one" until it was controlled for. Best-of rather
//! than mean because the noise here is all one-sided (interrupts, migration,
//! thermal), so the fastest run is the closest to the real cost.

use std::hint::black_box;
use std::time::Instant;

use vxn4_dsp::ops::{OpMajor, VoiceMajor, VoiceMajorPerVoiceGain};
use vxn4_dsp::wavetable::{
    Lookup, Plain, PlainUnchecked, ValueSlope, ValueSlopeUnchecked, WaveBank, Waveform,
};
use vxn4_op_bench::{BLOCK_1X, Fixture, SR, dense_routing, keys, sparse_routing};

/// 1x samples rendered per timed run. ~0.7s of audio per run.
const SAMPLES: usize = 32_768;

/// Discarded before timing starts, to settle caches.
const WARMUP: usize = 8_192;

/// Timed runs per point; the fastest is reported.
const REPS: usize = 5;

/// Busy-wait until the core has clocked up. See the module docs.
fn spin_up() {
    let t0 = Instant::now();
    let mut x = 1.0f32;
    while t0.elapsed().as_millis() < 400 {
        for _ in 0..200_000 {
            x = black_box(x * 1.000_000_1 + 1.0);
        }
    }
    black_box(x);
}

/// Best-of-`REPS` voice-samples per second at 1x.
fn best_rate<F: FnMut()>(voices_per_bank: usize, mut render_block: F) -> f64 {
    let blocks = SAMPLES / BLOCK_1X;
    for _ in 0..(WARMUP / BLOCK_1X) {
        render_block();
    }
    let mut best = 0.0f64;
    for _ in 0..REPS {
        let t0 = Instant::now();
        for _ in 0..blocks {
            render_block();
        }
        let secs = t0.elapsed().as_secs_f64();
        best = best.max((blocks * BLOCK_1X * voices_per_bank) as f64 / secs);
    }
    best
}

fn measure_vm<const V: usize, L: Lookup>(fx: &Fixture) -> f64 {
    let mut bank: VoiceMajor<V> = VoiceMajor::new();
    bank.cook(&fx.bank, &fx.cfg, &keys::<V>(), fx.sr_os());
    let (mut l, mut r) = ([0.0f32; BLOCK_1X], [0.0f32; BLOCK_1X]);
    best_rate(V, || {
        bank.render::<L>(&fx.bank, &fx.compiled, &fx.bus, fx.os, &mut l, &mut r);
        black_box((&l, &r));
    })
}

fn measure_om<const V: usize, L: Lookup>(fx: &Fixture) -> f64 {
    let mut bank: OpMajor<V> = OpMajor::new();
    bank.cook(&fx.bank, &fx.cfg, &keys::<V>(), fx.sr_os());
    let (mut l, mut r) = ([0.0f32; BLOCK_1X], [0.0f32; BLOCK_1X]);
    best_rate(V, || {
        bank.render::<L>(&fx.bank, &fx.compiled, &fx.bus, fx.os, &mut l, &mut r);
        black_box((&l, &r));
    })
}

fn measure_pvg<const V: usize, L: Lookup>(fx: &Fixture) -> f64 {
    let mut bank: VoiceMajorPerVoiceGain<V> = VoiceMajorPerVoiceGain::new(&fx.routing, 0.2);
    bank.cook(&fx.bank, &fx.cfg, &keys::<V>(), fx.sr_os());
    let (mut l, mut r) = ([0.0f32; BLOCK_1X], [0.0f32; BLOCK_1X]);
    best_rate(V, || {
        bank.render::<L>(&fx.bank, &fx.compiled, &fx.bus, fx.os, &mut l, &mut r);
        black_box((&l, &r));
    })
}

/// Scattered phases, precomputed so the timed loop does no address arithmetic.
///
/// 4096 entries is 16 KiB — big enough that the table index genuinely scatters,
/// small enough that streaming the phases themselves is not what gets measured.
fn scatter_phases() -> Vec<u32> {
    let mut p = 0x1234_5678u32;
    (0..4096)
        .map(|_| {
            p = p.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            p
        })
        .collect()
}

/// Lookups per second in isolation — no phase modulation, no matrix, no sum bus.
///
/// The full-block numbers fold the lookup into everything else, so a gap
/// between two lookup strategies there is a claim about the whole kernel, not
/// about the lookup. This measures the lookup alone so the two can be told
/// apart.
///
/// Two things had to be controlled for before this measured the lookup at all,
/// and both produced *identical numbers to four significant figures* across
/// strategies that cannot possibly cost the same — which is the tell:
///
/// 1. **A constant phase stride** makes the index sequence affine, so LLVM
///    vectorises the whole loop and every strategy collapses to L1 bandwidth.
///    Scattered phases from an array model the real case, where the index
///    depends on a modulation total not known until the previous tick.
/// 2. **A single accumulator** serialises the loop on `f32` add latency (~3
///    cycles), which is slower than any of the lookups, so all three measure
///    the adder. Eight independent accumulators keep enough in flight that the
///    lookup is what binds.
fn measure_lookup<L: Lookup>(bank: &WaveBank, mip: usize, phases: &[u32]) -> f64 {
    const PASSES: usize = 16;
    const ACC: usize = 8;
    let table = bank.table(Waveform::Saw);
    let chunks = phases.len() / ACC;
    let n = chunks * ACC * PASSES;

    for &p in phases {
        black_box(L::read(table, mip, p));
    }

    let mut best = 0.0f64;
    for _ in 0..REPS {
        let t0 = Instant::now();
        let mut acc = [0.0f32; ACC];
        for _ in 0..PASSES {
            for chunk in phases.chunks_exact(ACC) {
                for (a, &p) in acc.iter_mut().zip(chunk) {
                    *a += L::read(table, mip, p);
                }
            }
        }
        black_box(acc);
        let secs = t0.elapsed().as_secs_f64();
        best = best.max(n as f64 / secs);
    }
    best
}

/// Voice-samples per second → voices sustainable at 48 kHz.
fn voices(rate: f64) -> f64 {
    rate / SR as f64
}

fn rule(width: usize) {
    println!("{}", "-".repeat(width));
}

fn main() {
    println!("vxn-4 operator block — sizing sweep");
    println!("8 operators, {BLOCK_1X}-sample 1x blocks, {SAMPLES} samples x best-of-{REPS}.");
    println!(
        "Polyphony = voices sustainable at {} kHz on one core.\n",
        SR / 1000.0
    );

    spin_up();

    // ---------------------------------------------------------------- table 1
    println!("Table length x oversampling  (voice-major V=8, value+slope, dense 64/64)");
    rule(78);
    println!(
        "{:>9}  {:>4}  {:>12}  {:>10}  {:>12}  {:>10}",
        "mip-0 len", "os", "work set/vc", "polyphony", "voice-samp/s", "bank KiB"
    );
    rule(78);
    for base_len in [256usize, 512, 2048] {
        for os in [8usize, 16] {
            let fx = Fixture::new(base_len, os, dense_routing());
            let rate = measure_vm::<8, ValueSlope>(&fx);
            println!(
                "{:>9}  {:>3}x  {:>10} B  {:>10.1}  {:>12.3e}  {:>10.1}",
                base_len,
                os,
                fx.voice_working_set(),
                voices(rate),
                rate,
                fx.bank.tap_bytes() as f64 / 1024.0,
            );
        }
    }

    let fx = Fixture::new(512, 16, dense_routing());

    // ---------------------------------------------------------------- table 2
    println!("\nLane layout  (512-entry, 16x, dense 64/64 — identical work both sides)");
    rule(52);
    println!(
        "{:>14}  {:>6}  {:>10}  {:>12}",
        "layout", "voices", "polyphony", "voice-samp/s"
    );
    rule(52);
    for (label, v, rate) in [
        ("voice-major", 4, measure_vm::<4, ValueSlope>(&fx)),
        ("voice-major", 8, measure_vm::<8, ValueSlope>(&fx)),
        ("voice-major", 16, measure_vm::<16, ValueSlope>(&fx)),
        ("op-major", 4, measure_om::<4, ValueSlope>(&fx)),
        ("op-major", 8, measure_om::<8, ValueSlope>(&fx)),
        ("op-major", 16, measure_om::<16, ValueSlope>(&fx)),
    ] {
        println!("{label:>14}  {v:>6}  {:>10.1}  {rate:>12.3e}", voices(rate));
    }

    // ---------------------------------------------------------------- table 3
    println!("\nLookup in isolation  (512-entry saw, scattered phase, no PM, no matrix)");
    rule(58);
    println!("{:>26}  {:>12}  {:>14}", "lookup", "mip 0 M/s", "mip 4 M/s");
    rule(58);
    let ph = scatter_phases();
    for (name, m0, m4) in [
        (
            "value+slope, checked",
            measure_lookup::<ValueSlope>(&fx.bank, 0, &ph),
            measure_lookup::<ValueSlope>(&fx.bank, 4, &ph),
        ),
        (
            "value+slope, unchecked",
            measure_lookup::<ValueSlopeUnchecked>(&fx.bank, 0, &ph),
            measure_lookup::<ValueSlopeUnchecked>(&fx.bank, 4, &ph),
        ),
        (
            "plain f32, checked",
            measure_lookup::<Plain>(&fx.bank, 0, &ph),
            measure_lookup::<Plain>(&fx.bank, 4, &ph),
        ),
        (
            "plain f32, unchecked",
            measure_lookup::<PlainUnchecked>(&fx.bank, 0, &ph),
            measure_lookup::<PlainUnchecked>(&fx.bank, 4, &ph),
        ),
    ] {
        println!("{name:>26}  {:>12.1}  {:>14.1}", m0 / 1e6, m4 / 1e6);
    }

    // ---------------------------------------------------------------- table 4
    println!("\nLookup in the full block  (512-entry, 16x, dense, voice-major V=8)");
    rule(52);
    println!("{:>26}  {:>10}  {:>12}", "lookup", "polyphony", "voice-samp/s");
    rule(52);
    for (name, rate) in [
        ("value+slope, checked", measure_vm::<8, ValueSlope>(&fx)),
        (
            "value+slope, unchecked",
            measure_vm::<8, ValueSlopeUnchecked>(&fx),
        ),
        ("plain f32, checked", measure_vm::<8, Plain>(&fx)),
        ("plain f32, unchecked", measure_vm::<8, PlainUnchecked>(&fx)),
    ] {
        println!("{name:>26}  {:>10.1}  {rate:>12.3e}", voices(rate));
    }

    // ---------------------------------------------------------------- table 5
    println!("\nRoute density and per-voice gains  (512-entry, 16x, voice-major V=8)");
    rule(64);
    println!("{:>38}  {:>10}  {:>10}", "scenario", "routes", "polyphony");
    rule(64);
    let sparse = Fixture::new(512, 16, sparse_routing());
    let n_dense = fx.routing.density();
    let n_sparse = sparse.routing.density();
    for (name, n, rate) in [
        (
            "dense, shared route gains",
            n_dense,
            measure_vm::<8, ValueSlope>(&fx),
        ),
        (
            "dense, per-voice route gains",
            n_dense,
            measure_pvg::<8, ValueSlope>(&fx),
        ),
        (
            "sparse (DX7-shaped), shared gains",
            n_sparse,
            measure_vm::<8, ValueSlope>(&sparse),
        ),
        (
            "sparse, per-voice gains",
            n_sparse,
            measure_pvg::<8, ValueSlope>(&sparse),
        ),
    ] {
        println!("{name:>38}  {n:>10}  {:>10.1}", voices(rate));
    }

    println!(
        "\nNote: the decimator is not in these numbers. It runs on the stereo sum\n\
         bus, not per voice, so it is a fixed cost that does not scale with\n\
         polyphony. The `render` boxcar is a placeholder for it."
    );
}
