//! Shared test helpers for the DSP crates.
//!
//! Not `#[cfg(test)]`: these are consumed by *other* crates' test modules, so
//! they have to be part of the normal build. They are small and
//! `#[inline]`-free, so the cost of carrying them is a few hundred bytes in a
//! release artefact that never calls them.
//!
//! Canonical copies, gathered by ticket 0226:
//!
//! - `assert_bit_exact_passthrough` / `assert_bit_exact_after_settle` — were in
//!   `vxn2-dsp::test_util`, with a hand-copied duplicate inside `vxn-dsp`'s
//!   `dynamics` test module.
//! - `worst_d4` / `join_d4` — the 4th-difference click detector, from vxn-1's
//!   declick suite. vxn-2's declick harness uses the same probe.

use std::f32::consts::TAU;

/// Assert that a stereo process function is a **bit-exact passthrough** for `n`
/// consecutive samples.
///
/// Bit-exact, not approximate: an effect that has settled to bypass must return
/// the same float bits it was given.
///
/// The input is a fixed two-frequency sine pair (330 Hz L / 110 Hz R at 48 kHz).
/// Any non-trivial deterministic pair establishes the property for ordinary
/// values, and this catches the realistic failures — a residual gain, a wet
/// contribution that never quite reached zero, a denormal-flushed tail.
///
/// **What it does not catch.** These inputs are all ordinary positive-magnitude
/// floats, so an implementation that computes `x * 1.0 + 0.0` instead of
/// returning `x` passes: the two agree bitwise on every value in this set. They
/// diverge only at `-0.0`, where `-0.0 * 1.0 + 0.0` is `+0.0`
/// (`0x00000000` vs `0x80000000`). That gap is why
/// [`crate::declick::WetFade::settled_off`] exists to license skipping the
/// arithmetic *entirely* rather than to bless a cheap-looking equivalent — the
/// contract is "return the input", not "compute something equal to it".
pub fn assert_bit_exact_passthrough(mut process_fn: impl FnMut(f32, f32) -> (f32, f32), n: usize) {
    const SR: f32 = 48_000.0;
    for i in 0..n {
        let x = 0.4 * (TAU * 330.0 * i as f32 / SR).sin();
        let y = -0.3 * (TAU * 110.0 * i as f32 / SR).cos();
        let (l, r) = process_fn(x, y);
        assert_eq!(l.to_bits(), x.to_bits(), "L not bit-exact at i={i}: {l} vs {x}");
        assert_eq!(r.to_bits(), y.to_bits(), "R not bit-exact at i={i}: {r} vs {y}");
    }
}

/// Settle `process_fn` for `settle` samples (arbitrary input), then assert
/// bit-exact passthrough for `n` samples.
///
/// The settle phase is what makes this the *switch-off* check rather than the
/// never-enabled check: it lets a fade-out actually complete first.
pub fn assert_bit_exact_after_settle(
    mut process_fn: impl FnMut(f32, f32) -> (f32, f32),
    settle: usize,
    n: usize,
) {
    for _ in 0..settle {
        process_fn(0.3, 0.3);
    }
    assert_bit_exact_passthrough(process_fn, n);
}

/// Assert an [`FxKernel`](crate::fx::FxKernel)'s `process_block` override is
/// **sample-identical** to looping its `process`.
///
/// The contract every vectorised block path has to keep, and the reason
/// `FxKernel` is a trait at all: written once here rather than in each of
/// 0228–0232. Takes a constructor rather than an instance because it needs two
/// independent kernels in the same state.
pub fn assert_block_matches_sample<K: crate::fx::FxKernel>(
    make: impl Fn() -> K,
    params: &K::Params,
    n: usize,
) {
    const SR: f32 = 48_000.0;
    let mut by_sample = make();
    let mut by_block = make();
    by_sample.set_params(params);
    by_block.set_params(params);

    let input_l: Vec<f32> = (0..n).map(|i| 0.4 * (TAU * 330.0 * i as f32 / SR).sin()).collect();
    let input_r: Vec<f32> = (0..n).map(|i| -0.3 * (TAU * 110.0 * i as f32 / SR).cos()).collect();

    let mut want_l = Vec::with_capacity(n);
    let mut want_r = Vec::with_capacity(n);
    for i in 0..n {
        let (l, r) = by_sample.process(input_l[i], input_r[i]);
        want_l.push(l);
        want_r.push(r);
    }

    let mut got_l = input_l.clone();
    let mut got_r = input_r.clone();
    by_block.process_block(&mut got_l, &mut got_r);

    for i in 0..n {
        assert_eq!(
            got_l[i].to_bits(),
            want_l[i].to_bits(),
            "process_block diverged from process on L at i={i}: {} vs {}",
            got_l[i],
            want_l[i]
        );
        assert_eq!(
            got_r[i].to_bits(),
            want_r[i].to_bits(),
            "process_block diverged from process on R at i={i}: {} vs {}",
            got_r[i],
            want_r[i]
        );
    }
}

/// 4th-difference click detector: max `|b[i+2] − 4b[i+1] + 6b[i] − 4b[i−1] +
/// b[i−2]|` over `range`.
///
/// A click is a discontinuity in the signal or its low derivatives, and d4
/// responds to those far more sharply than amplitude does — a switch-induced
/// step can be tiny in absolute terms and still audible. Caller ensures
/// `2 <= range.start` and `range.end + 2 <= buf.len()`.
pub fn worst_d4(buf: &[f32], range: std::ops::Range<usize>) -> f64 {
    range
        .map(|i| {
            (buf[i + 2] - 4.0 * buf[i + 1] + 6.0 * buf[i] - 4.0 * buf[i - 1] + buf[i - 2]).abs()
                as f64
        })
        .fold(0.0, f64::max)
}

/// `d4` straddling one edge sample — the discontinuity a hard switch introduces
/// and a crossfade removes. Six samples centred on the join cover the kernel's
/// ±2 reach either side.
///
/// Compare against a *steady-state* `worst_d4` over the same signal, never
/// against an absolute threshold: a bright or heavily modulated patch has a
/// high d4 floor of its own, and the question is only whether the edge stands
/// out from it.
pub fn join_d4(buf: &[f32], edge_sample: usize) -> f64 {
    worst_d4(buf, edge_sample - 3..edge_sample + 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_exact_passthrough_accepts_identity() {
        assert_bit_exact_passthrough(|l, r| (l, r), 256);
    }

    /// Documents the helper's blind spot rather than pretending it has none.
    /// `x * 1.0 + 0.0` passes `assert_bit_exact_passthrough` — it agrees on
    /// every ordinary float — and diverges only at negative zero, which the
    /// fixed sine inputs never produce. Asserted directly, not through the
    /// helper, because the helper by construction cannot see it.
    #[test]
    fn multiply_by_one_is_indistinguishable_here_except_at_negative_zero() {
        assert_bit_exact_passthrough(|l, r| (l * 1.0 + 0.0, r * 1.0 + 0.0), 256);
        assert_eq!((-0.0f32 * 1.0 + 0.0).to_bits(), 0x0000_0000, "should be +0.0");
        assert_eq!((-0.0f32).to_bits(), 0x8000_0000, "should be -0.0");
    }

    #[test]
    #[should_panic(expected = "not bit-exact")]
    fn bit_exact_passthrough_rejects_a_tiny_gain_error() {
        assert_bit_exact_passthrough(|l, r| (l * 1.000_001, r), 256);
    }

    #[test]
    fn after_settle_ignores_the_settle_phase() {
        // Non-passthrough for the first 10 samples, identity thereafter.
        let mut n = 0;
        assert_bit_exact_after_settle(
            move |l, r| {
                n += 1;
                if n <= 10 { (l * 9.0, r * 9.0) } else { (l, r) }
            },
            10,
            128,
        );
    }

    /// A step is exactly what d4 is for: a ramp has a low d4, a discontinuity a
    /// high one, and the ratio is the signal the declick suites threshold on.
    #[test]
    fn d4_separates_a_step_from_a_ramp() {
        let ramp: Vec<f32> = (0..64).map(|i| i as f32 * 0.01).collect();
        let mut step = ramp.clone();
        for s in step.iter_mut().skip(32) {
            *s += 0.5;
        }
        let ramp_d4 = worst_d4(&ramp, 2..60);
        let step_d4 = worst_d4(&step, 2..60);
        assert!(ramp_d4 < 1e-6, "a linear ramp should have ~zero d4, got {ramp_d4}");
        assert!(step_d4 > 0.1, "a 0.5 step should show clearly, got {step_d4}");
    }

    #[test]
    fn join_d4_finds_the_edge_a_windowed_scan_would_miss() {
        let mut buf: Vec<f32> = (0..128).map(|i| (i as f32 * 0.05).sin()).collect();
        buf[64] += 0.4; // one-sample spike
        let at_edge = join_d4(&buf, 64);
        let away = join_d4(&buf, 20);
        assert!(at_edge > away * 10.0, "edge {at_edge} vs away {away}");
    }
}
