//! VXN2 sine + operator microbenches.
//!
//! Block: 96 ops × 256 samples per call. The three "sine alone" cases compare
//! readers; the four "op_*" cases add per-op envelope multiplication at varying
//! rates to see whether the NEON sine win survives env mul.
//!
//! At 48 kHz a 256-sample block = 5333 µs of audio — multiply (5333 / block_ns)
//! to get realtime headroom factor.

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use vxn2_osc_bench::*;

const N_OPS: usize = 96;
const BLOCK: usize = 256;
const SUBBLOCK: usize = 64; // 4 sub-blocks per block.

fn make_increments() -> [u32; N_OPS] {
    let mut incs = [0u32; N_OPS];
    let sr = 48_000.0f64;
    for (i, inc) in incs.iter_mut().enumerate() {
        let t = i as f64 / (N_OPS - 1) as f64;
        let hz = 55.0 * 2.0f64.powf(t * 8.0);
        *inc = (hz / sr * 4_294_967_296.0) as u32;
    }
    incs
}

fn init_phases() -> [u32; N_OPS] {
    let mut phases = [0u32; N_OPS];
    for (i, p) in phases.iter_mut().enumerate() {
        *p = (i as u32).wrapping_mul(0x1234_5678);
    }
    phases
}

fn init_adsrs() -> [adsr::Adsr; N_OPS] {
    // All in Sustain — steady-state held note, the common case.
    let mut arr = [adsr::Adsr::new(1e-3, 1e-4, 0.7, 1e-4); N_OPS];
    for (i, a) in arr.iter_mut().enumerate() {
        // Different sustain level per op so the env mul actually matters
        // (otherwise the optimiser may hoist a uniform broadcast).
        let level = 0.3 + 0.6 * (i as f32 / N_OPS as f32);
        a.force_sustain(level);
    }
    arr
}

// ── Sine readers only ──────────────────────────────────────────────────────

fn render_scalar_fast_sine(phases: &mut [u32; N_OPS], incs: &[u32; N_OPS]) -> f32 {
    let mut acc = 0.0f32;
    for _ in 0..BLOCK {
        for i in 0..N_OPS {
            phases[i] = phases[i].wrapping_add(incs[i]);
            acc += scalar::fast_sine_q32(phases[i]);
        }
    }
    acc
}

fn render_scalar_lookup(phases: &mut [u32; N_OPS], incs: &[u32; N_OPS]) -> f32 {
    let table = &SINE_TABLE;
    let mut acc = 0.0f32;
    for _ in 0..BLOCK {
        for i in 0..N_OPS {
            phases[i] = phases[i].wrapping_add(incs[i]);
            acc += scalar::lookup_sine_q32(phases[i], table);
        }
    }
    acc
}

#[cfg(target_arch = "aarch64")]
fn render_neon_fast_sine(phases: &mut [u32; N_OPS], incs: &[u32; N_OPS]) -> f32 {
    use std::arch::aarch64::*;
    const LANES: usize = N_OPS / 4;
    let mut sum = unsafe { vdupq_n_f32(0.0) };
    for _ in 0..BLOCK {
        for l in 0..LANES {
            let off = l * 4;
            unsafe {
                let p = vld1q_u32(phases.as_ptr().add(off));
                let i = vld1q_u32(incs.as_ptr().add(off));
                let next = vaddq_u32(p, i);
                vst1q_u32(phases.as_mut_ptr().add(off), next);
                let s = neon::fast_sine_q32_x4(next);
                sum = vaddq_f32(sum, s);
            }
        }
    }
    let mut out = [0.0f32; 4];
    unsafe { vst1q_f32(out.as_mut_ptr(), sum) };
    out.iter().sum()
}

// ── Full operator: sine × env ─────────────────────────────────────────────

/// Sustain-skip path: env levels constant for the whole block (cached from
/// last block). Lower bound on op cost with env mul present.
#[cfg(target_arch = "aarch64")]
fn render_op_skip_sustain(
    phases: &mut [u32; N_OPS],
    incs: &[u32; N_OPS],
    env_cache: &[f32; N_OPS],
) -> f32 {
    use std::arch::aarch64::*;
    const LANES: usize = N_OPS / 4;
    let mut sum = unsafe { vdupq_n_f32(0.0) };
    // Preload env per group once for the whole block.
    let mut env_vec = [unsafe { vdupq_n_f32(0.0) }; LANES];
    for (l, ev) in env_vec.iter_mut().enumerate() {
        *ev = unsafe { vld1q_f32(env_cache.as_ptr().add(l * 4)) };
    }
    for _ in 0..BLOCK {
        for l in 0..LANES {
            let off = l * 4;
            unsafe {
                let p = vld1q_u32(phases.as_ptr().add(off));
                let i = vld1q_u32(incs.as_ptr().add(off));
                let next = vaddq_u32(p, i);
                vst1q_u32(phases.as_mut_ptr().add(off), next);
                let s = neon::fast_sine_q32_x4(next);
                sum = vfmaq_f32(sum, s, env_vec[l]);
            }
        }
    }
    let mut out = [0.0f32; 4];
    unsafe { vst1q_f32(out.as_mut_ptr(), sum) };
    out.iter().sum()
}

/// Block-rate env: tick all 96 envs once at block top, hold constant inside.
/// Matches the model where env updates are slower than 1 / block_period.
#[cfg(target_arch = "aarch64")]
fn render_op_block_env(
    phases: &mut [u32; N_OPS],
    incs: &[u32; N_OPS],
    adsrs: &mut [adsr::Adsr; N_OPS],
    gates: &[bool; N_OPS],
) -> f32 {
    use std::arch::aarch64::*;
    const LANES: usize = N_OPS / 4;

    // Tick at block top, materialise to packed [f32; N_OPS].
    let mut env = [0.0f32; N_OPS];
    for (i, e) in env.iter_mut().enumerate() {
        *e = adsrs[i].tick(gates[i], false);
    }

    let mut env_vec = [unsafe { vdupq_n_f32(0.0) }; LANES];
    for (l, ev) in env_vec.iter_mut().enumerate() {
        *ev = unsafe { vld1q_f32(env.as_ptr().add(l * 4)) };
    }

    let mut sum = unsafe { vdupq_n_f32(0.0) };
    for _ in 0..BLOCK {
        for l in 0..LANES {
            let off = l * 4;
            unsafe {
                let p = vld1q_u32(phases.as_ptr().add(off));
                let i = vld1q_u32(incs.as_ptr().add(off));
                let next = vaddq_u32(p, i);
                vst1q_u32(phases.as_mut_ptr().add(off), next);
                let s = neon::fast_sine_q32_x4(next);
                sum = vfmaq_f32(sum, s, env_vec[l]);
            }
        }
    }
    let mut out = [0.0f32; 4];
    unsafe { vst1q_f32(out.as_mut_ptr(), sum) };
    out.iter().sum()
}

/// Sub-block-rate env: tick at each 64-sample boundary (4 ticks per block per
/// op), hold constant within sub-block. Recovers attack fidelity at the cost
/// of 4× more env ticks per block.
#[cfg(target_arch = "aarch64")]
fn render_op_subblock_env(
    phases: &mut [u32; N_OPS],
    incs: &[u32; N_OPS],
    adsrs: &mut [adsr::Adsr; N_OPS],
    gates: &[bool; N_OPS],
) -> f32 {
    use std::arch::aarch64::*;
    const LANES: usize = N_OPS / 4;
    const SUBBLOCKS: usize = BLOCK / SUBBLOCK;

    let mut env = [0.0f32; N_OPS];
    let mut env_vec = [unsafe { vdupq_n_f32(0.0) }; LANES];
    let mut sum = unsafe { vdupq_n_f32(0.0) };

    for _sb in 0..SUBBLOCKS {
        for (i, e) in env.iter_mut().enumerate() {
            *e = adsrs[i].tick(gates[i], false);
        }
        for (l, ev) in env_vec.iter_mut().enumerate() {
            *ev = unsafe { vld1q_f32(env.as_ptr().add(l * 4)) };
        }
        for _ in 0..SUBBLOCK {
            for l in 0..LANES {
                let off = l * 4;
                unsafe {
                    let p = vld1q_u32(phases.as_ptr().add(off));
                    let i = vld1q_u32(incs.as_ptr().add(off));
                    let next = vaddq_u32(p, i);
                    vst1q_u32(phases.as_mut_ptr().add(off), next);
                    let s = neon::fast_sine_q32_x4(next);
                    sum = vfmaq_f32(sum, s, env_vec[l]);
                }
            }
        }
    }
    let mut out = [0.0f32; 4];
    unsafe { vst1q_f32(out.as_mut_ptr(), sum) };
    out.iter().sum()
}

/// Per-sample env path — the cliff. Tick all 96 envs every sample, pack into
/// 4-lane vec each sample, then vfma into sine. Predicted to evaporate the
/// SIMD win.
#[cfg(target_arch = "aarch64")]
fn render_op_persample_pack(
    phases: &mut [u32; N_OPS],
    incs: &[u32; N_OPS],
    adsrs: &mut [adsr::Adsr; N_OPS],
    gates: &[bool; N_OPS],
) -> f32 {
    use std::arch::aarch64::*;
    const LANES: usize = N_OPS / 4;
    let mut env = [0.0f32; N_OPS];
    let mut sum = unsafe { vdupq_n_f32(0.0) };
    for _ in 0..BLOCK {
        // Per-sample env tick + pack.
        for (i, e) in env.iter_mut().enumerate() {
            *e = adsrs[i].tick(gates[i], false);
        }
        for l in 0..LANES {
            let off = l * 4;
            unsafe {
                let ev = vld1q_f32(env.as_ptr().add(off));
                let p = vld1q_u32(phases.as_ptr().add(off));
                let i = vld1q_u32(incs.as_ptr().add(off));
                let next = vaddq_u32(p, i);
                vst1q_u32(phases.as_mut_ptr().add(off), next);
                let s = neon::fast_sine_q32_x4(next);
                sum = vfmaq_f32(sum, s, ev);
            }
        }
    }
    let mut out = [0.0f32; 4];
    unsafe { vst1q_f32(out.as_mut_ptr(), sum) };
    out.iter().sum()
}

// ── Bench wiring ──────────────────────────────────────────────────────────

fn bench_all(c: &mut Criterion) {
    let incs = make_increments();
    let gates = [true; N_OPS];

    // ── Sine readers ──
    let mut g = c.benchmark_group("sine_block");
    g.throughput(Throughput::Elements((N_OPS * BLOCK) as u64));

    g.bench_function("scalar_fast_sine_q32", |b| {
        let mut phases = init_phases();
        b.iter(|| {
            black_box(render_scalar_fast_sine(
                black_box(&mut phases),
                black_box(&incs),
            ))
        })
    });

    g.bench_function("scalar_lookup_sine_q32", |b| {
        let mut phases = init_phases();
        b.iter(|| {
            black_box(render_scalar_lookup(
                black_box(&mut phases),
                black_box(&incs),
            ))
        })
    });

    #[cfg(target_arch = "aarch64")]
    g.bench_function("neon_fast_sine_q32_x4", |b| {
        let mut phases = init_phases();
        b.iter(|| {
            black_box(render_neon_fast_sine(
                black_box(&mut phases),
                black_box(&incs),
            ))
        })
    });

    g.finish();

    // ── Full operator (sine × env) ──
    #[cfg(target_arch = "aarch64")]
    {
        let mut g = c.benchmark_group("op_block");
        g.throughput(Throughput::Elements((N_OPS * BLOCK) as u64));

        g.bench_function("op_skip_sustain", |b| {
            let mut phases = init_phases();
            // Cached env values from "previous block" — frozen.
            let mut env_cache = [0.0f32; N_OPS];
            for (i, e) in env_cache.iter_mut().enumerate() {
                *e = 0.3 + 0.6 * (i as f32 / N_OPS as f32);
            }
            b.iter(|| {
                black_box(render_op_skip_sustain(
                    black_box(&mut phases),
                    black_box(&incs),
                    black_box(&env_cache),
                ))
            })
        });

        g.bench_function("op_block_env", |b| {
            let mut phases = init_phases();
            let mut adsrs = init_adsrs();
            b.iter(|| {
                black_box(render_op_block_env(
                    black_box(&mut phases),
                    black_box(&incs),
                    black_box(&mut adsrs),
                    black_box(&gates),
                ))
            })
        });

        g.bench_function("op_subblock_env_64", |b| {
            let mut phases = init_phases();
            let mut adsrs = init_adsrs();
            b.iter(|| {
                black_box(render_op_subblock_env(
                    black_box(&mut phases),
                    black_box(&incs),
                    black_box(&mut adsrs),
                    black_box(&gates),
                ))
            })
        });

        g.bench_function("op_persample_pack", |b| {
            let mut phases = init_phases();
            let mut adsrs = init_adsrs();
            b.iter(|| {
                black_box(render_op_persample_pack(
                    black_box(&mut phases),
                    black_box(&incs),
                    black_box(&mut adsrs),
                    black_box(&gates),
                ))
            })
        });

        g.finish();
    }
}

criterion_group!(benches, bench_all);
criterion_main!(benches);
