//! 3-op FM algorithm with OP3 self-feedback (DX7 2-sample averaging trick).
//!
//! Topology:
//!     OP3 (self-FB) → OP2 → OP1 → output
//!
//! Per voice the chain is serial within a sample (OP3 must finish before OP2
//! reads it). SIMD across voices (SoA): lane = voice. 32 voices = 8 groups
//! of 4 lanes.
//!
//! Feedback: OP3 reads the average of its last two outputs (per-voice state,
//! SoA arrays). Linear-blends the loop down, matches the DX7 behaviour that
//! tames otherwise-chaotic FB.
//!
//! Env: one ADSR per op per voice (96 envs total). Block-rate broadcast —
//! tick at block top, hold across the block.
//!
//! Block: 256 samples × 32 voices × 3 ops = 24,576 sine evals per call.

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use vxn2_osc_bench::*;

const VOICES: usize = 32;
const OPS: usize = 3;
const BLOCK: usize = 256;
const LANES: usize = VOICES / 4; // 8 groups of 4

/// Map an f32 modulator in roughly [-1, +1] to a Q32 phase offset. Chosen so
/// that a unit modulator at mod index = 1.0 sweeps one full cycle (i.e. 2π
/// radians of phase). Real synths scale this by op-to-op send levels and a
/// global index parameter.
const PM_SCALE_F32: f32 = 4_294_967_296.0; // 2^32

/// Self-feedback gain — controls how loud the averaged feedback signal is
/// pushed back into OP3's phase. Below ~1.0 the loop is "warm sawtooth",
/// above ~1.0 it heads toward DX-style noise. Pick a moderate setting so
/// the loop is doing real work (not collapsed to zero) during the bench.
const SELF_FB_GAIN: f32 = 0.6;

struct OpState {
    phase: [u32; VOICES],
    inc: [u32; VOICES],
    env_cache: [f32; VOICES],
    adsr: [adsr::Adsr; VOICES],
}

impl OpState {
    fn new(hz_base: f64, ratio: f64, sustain_base: f32) -> Self {
        let sr = 48_000.0f64;
        let mut phase = [0u32; VOICES];
        let mut inc = [0u32; VOICES];
        let mut env_cache = [0.0f32; VOICES];
        let mut adsr = [adsr::Adsr::new(1e-3, 1e-4, sustain_base, 1e-4); VOICES];
        for v in 0..VOICES {
            phase[v] = (v as u32).wrapping_mul(0x1234_5678);
            // Each voice gets a slightly different fundamental (spread across
            // 1 octave) so the bench actually exercises distinct phases.
            let note_hz = hz_base * 2.0f64.powf(v as f64 / VOICES as f64);
            inc[v] = ((note_hz * ratio) / sr * 4_294_967_296.0) as u32;
            // Stagger sustain levels per voice so env_x4 mul isn't a uniform
            // broadcast the optimiser could hoist away.
            let level = sustain_base + 0.2 * (v as f32 / VOICES as f32);
            adsr[v].force_sustain(level);
            env_cache[v] = level;
        }
        Self {
            phase,
            inc,
            env_cache,
            adsr,
        }
    }
}

#[cfg(target_arch = "aarch64")]
mod algo {
    use super::*;
    use std::arch::aarch64::*;

    /// OP3 self-feedback state (last two output samples per voice).
    pub struct FeedbackState {
        pub prev1: [f32; VOICES],
        pub prev2: [f32; VOICES],
    }

    impl FeedbackState {
        pub fn new() -> Self {
            Self {
                prev1: [0.0; VOICES],
                prev2: [0.0; VOICES],
            }
        }
    }

    /// Convert an f32 modulator (~[-1, +1]) to a Q32 phase offset by the
    /// route's scale, on 4 lanes.
    #[inline]
    #[target_feature(enable = "neon")]
    unsafe fn pm_offset(modu: float32x4_t, scale: float32x4_t) -> uint32x4_t {
        unsafe {
            // f32 -> i32 saturating; reinterpret as u32 (wrap on add is what we want).
            let scaled = vmulq_f32(modu, scale);
            let asint = vcvtq_s32_f32(scaled);
            vreinterpretq_u32_s32(asint)
        }
    }

    /// One block of the 3-op chain. Returns a tap so the result can't be DCEd.
    #[target_feature(enable = "neon")]
    pub unsafe fn render_block(
        op3: &mut OpState,
        op2: &mut OpState,
        op1: &mut OpState,
        fb: &mut FeedbackState,
        env3: &[float32x4_t; LANES],
        env2_v: &[float32x4_t; LANES],
        env1_v: &[float32x4_t; LANES],
        self_fb_gain: float32x4_t,
        mod_3_to_2: float32x4_t,
        mod_2_to_1: float32x4_t,
        pm_scale: float32x4_t,
    ) -> float32x4_t {
        let mut sum = vdupq_n_f32(0.0);
            let half = vdupq_n_f32(0.5);

            for _ in 0..BLOCK {
                for l in 0..LANES {
                    let off = l * 4;

                    // ── OP3: self-feedback PM ───────────────────────────
                    let p1 = vld1q_f32(fb.prev1.as_ptr().add(off));
                    let p2 = vld1q_f32(fb.prev2.as_ptr().add(off));
                    let avg = vmulq_f32(vaddq_f32(p1, p2), half);
                    let mod3 = vmulq_f32(avg, self_fb_gain);

                    let ph3 = vld1q_u32(op3.phase.as_ptr().add(off));
                    let in3 = vld1q_u32(op3.inc.as_ptr().add(off));
                    let ph3n = vaddq_u32(ph3, in3);
                    vst1q_u32(op3.phase.as_mut_ptr().add(off), ph3n);

                    let p3_mod = vaddq_u32(ph3n, pm_offset(mod3, pm_scale));
                    let out3 = neon::fast_sine_q32_x4(p3_mod);
                    let out3_env = vmulq_f32(out3, env3[l]);

                    vst1q_f32(fb.prev2.as_mut_ptr().add(off), p1);
                    vst1q_f32(fb.prev1.as_mut_ptr().add(off), out3_env);

                    // ── OP2: phase-modulated by OP3 ─────────────────────
                    let ph2 = vld1q_u32(op2.phase.as_ptr().add(off));
                    let in2 = vld1q_u32(op2.inc.as_ptr().add(off));
                    let ph2n = vaddq_u32(ph2, in2);
                    vst1q_u32(op2.phase.as_mut_ptr().add(off), ph2n);

                    let p2_mod = vaddq_u32(
                        ph2n,
                        pm_offset(vmulq_f32(out3_env, mod_3_to_2), pm_scale),
                    );
                    let out2 = neon::fast_sine_q32_x4(p2_mod);
                    let out2_env = vmulq_f32(out2, env2_v[l]);

                    // ── OP1: carrier, modulated by OP2, → output ────────
                    let ph1 = vld1q_u32(op1.phase.as_ptr().add(off));
                    let in1 = vld1q_u32(op1.inc.as_ptr().add(off));
                    let ph1n = vaddq_u32(ph1, in1);
                    vst1q_u32(op1.phase.as_mut_ptr().add(off), ph1n);

                    let p1_mod = vaddq_u32(
                        ph1n,
                        pm_offset(vmulq_f32(out2_env, mod_2_to_1), pm_scale),
                    );
                    let out1 = neon::fast_sine_q32_x4(p1_mod);
                    sum = vfmaq_f32(sum, out1, env1_v[l]);
                }
            }
            sum
    }
}

#[cfg(target_arch = "aarch64")]
fn run_block_env(
    op3: &mut OpState,
    op2: &mut OpState,
    op1: &mut OpState,
    fb: &mut algo::FeedbackState,
    gates: &[bool; VOICES],
) -> f32 {
    use std::arch::aarch64::*;
    unsafe {
        let pm_scale = vdupq_n_f32(PM_SCALE_F32);
        let self_fb_gain = vdupq_n_f32(SELF_FB_GAIN);
        // Modest mod indices so the FM is in "warm" territory, not chaos.
        let mod_3_to_2 = vdupq_n_f32(0.25);
        let mod_2_to_1 = vdupq_n_f32(0.40);

        // Block-top env refresh (block-rate ADSR for every op).
        let env3 = {
            for v in 0..VOICES {
                op3.env_cache[v] = op3.adsr[v].tick(gates[v], false);
            }
            let mut e = [vdupq_n_f32(0.0); LANES];
            for (l, ev) in e.iter_mut().enumerate() {
                *ev = vld1q_f32(op3.env_cache.as_ptr().add(l * 4));
            }
            e
        };
        let env2_v = {
            for v in 0..VOICES {
                op2.env_cache[v] = op2.adsr[v].tick(gates[v], false);
            }
            let mut e = [vdupq_n_f32(0.0); LANES];
            for (l, ev) in e.iter_mut().enumerate() {
                *ev = vld1q_f32(op2.env_cache.as_ptr().add(l * 4));
            }
            e
        };
        let env1_v = {
            for v in 0..VOICES {
                op1.env_cache[v] = op1.adsr[v].tick(gates[v], false);
            }
            let mut e = [vdupq_n_f32(0.0); LANES];
            for (l, ev) in e.iter_mut().enumerate() {
                *ev = vld1q_f32(op1.env_cache.as_ptr().add(l * 4));
            }
            e
        };

        let sum = algo::render_block(
            op3,
            op2,
            op1,
            fb,
            &env3,
            &env2_v,
            &env1_v,
            self_fb_gain,
            mod_3_to_2,
            mod_2_to_1,
            pm_scale,
        );
        let mut out = [0.0f32; 4];
        vst1q_f32(out.as_mut_ptr(), sum);
        out.iter().sum()
    }
}

#[cfg(target_arch = "aarch64")]
fn run_skip_sustain(
    op3: &mut OpState,
    op2: &mut OpState,
    op1: &mut OpState,
    fb: &mut algo::FeedbackState,
) -> f32 {
    use std::arch::aarch64::*;
    unsafe {
        let pm_scale = vdupq_n_f32(PM_SCALE_F32);
        let self_fb_gain = vdupq_n_f32(SELF_FB_GAIN);
        let mod_3_to_2 = vdupq_n_f32(0.25);
        let mod_2_to_1 = vdupq_n_f32(0.40);

        // Sustain skip — use cached env values, no tick.
        let env3 = {
            let mut e = [vdupq_n_f32(0.0); LANES];
            for (l, ev) in e.iter_mut().enumerate() {
                *ev = vld1q_f32(op3.env_cache.as_ptr().add(l * 4));
            }
            e
        };
        let env2_v = {
            let mut e = [vdupq_n_f32(0.0); LANES];
            for (l, ev) in e.iter_mut().enumerate() {
                *ev = vld1q_f32(op2.env_cache.as_ptr().add(l * 4));
            }
            e
        };
        let env1_v = {
            let mut e = [vdupq_n_f32(0.0); LANES];
            for (l, ev) in e.iter_mut().enumerate() {
                *ev = vld1q_f32(op1.env_cache.as_ptr().add(l * 4));
            }
            e
        };

        let sum = algo::render_block(
            op3,
            op2,
            op1,
            fb,
            &env3,
            &env2_v,
            &env1_v,
            self_fb_gain,
            mod_3_to_2,
            mod_2_to_1,
            pm_scale,
        );
        let mut out = [0.0f32; 4];
        vst1q_f32(out.as_mut_ptr(), sum);
        out.iter().sum()
    }
}

fn bench_algo3(c: &mut Criterion) {
    let gates = [true; VOICES];
    let mut g = c.benchmark_group("algo3_3op_fb");
    // Throughput = total sine evals (3 ops × voices × block).
    g.throughput(Throughput::Elements((OPS * VOICES * BLOCK) as u64));

    #[cfg(target_arch = "aarch64")]
    {
        g.bench_function("op_skip_sustain", |b| {
            // Operator role frequencies: OP1 carrier (1×), OP2 (3.01× detuned),
            // OP3 (5.02× detuned). Realistic bell/EP territory.
            let mut op1 = OpState::new(220.0, 1.00, 0.7);
            let mut op2 = OpState::new(220.0, 3.01, 0.6);
            let mut op3 = OpState::new(220.0, 5.02, 0.5);
            let mut fb = algo::FeedbackState::new();
            b.iter(|| {
                black_box(run_skip_sustain(
                    black_box(&mut op3),
                    black_box(&mut op2),
                    black_box(&mut op1),
                    black_box(&mut fb),
                ))
            })
        });

        g.bench_function("op_block_env", |b| {
            let mut op1 = OpState::new(220.0, 1.00, 0.7);
            let mut op2 = OpState::new(220.0, 3.01, 0.6);
            let mut op3 = OpState::new(220.0, 5.02, 0.5);
            let mut fb = algo::FeedbackState::new();
            b.iter(|| {
                black_box(run_block_env(
                    black_box(&mut op3),
                    black_box(&mut op2),
                    black_box(&mut op1),
                    black_box(&mut fb),
                    black_box(&gates),
                ))
            })
        });
    }

    g.finish();
}

criterion_group!(benches, bench_algo3);
criterion_main!(benches);
