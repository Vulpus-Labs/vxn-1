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
//!
//! `null_test_peak_dbfs` / `assert_null_test` joined them with ticket 0329 —
//! the same genre (compare two renders, say how far apart they are), one
//! tolerance wider. Each synth keeps its own *reference* render, because
//! capturing one needs that synth's engine; only the comparator is shared.

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

// ── null test ───────────────────────────────────────────────────────────────

/// Worst per-sample difference between two renders: `(index, |a − b|)` in
/// linear amplitude.
///
/// The index is half the answer. A null test that only reports a magnitude
/// tells you the render moved but not *where*, and "where" is what separates a
/// one-block transient at a note-on from a drift that grows across the tail —
/// the two failure modes E049 has to tell apart.
///
/// NaN is caught explicitly rather than left to `f64::max`: comparisons against
/// NaN are all false, so a silent `>` test would fold a non-finite render into a
/// zero difference and report a perfect null.
fn null_test_peak(a: &[f32], b: &[f32]) -> (usize, f64) {
    assert_eq!(
        a.len(),
        b.len(),
        "null test needs two renders of the same length, got {} and {}",
        a.len(),
        b.len()
    );
    // Two empty renders agree perfectly, which is the most convincing possible
    // pass and never the answer anyone wanted: a helper whose block count
    // floored to zero would report a flawless null on a synth it never ran.
    assert!(!a.is_empty(), "null test needs a render, got two empty slices");
    let mut worst = 0.0_f64;
    let mut at = 0_usize;
    for (i, (&x, &y)) in a.iter().zip(b.iter()).enumerate() {
        let d = (x as f64 - y as f64).abs();
        if d.is_nan() {
            return (i, f64::INFINITY);
        }
        if d > worst {
            worst = d;
            at = i;
        }
    }
    (at, worst)
}

/// Peak difference between two renders, in dBFS. `-inf` for identical buffers.
///
/// Full scale is `1.0`, so the reading is directly comparable to the numbers
/// E049 §"The bar" quotes: −100 dBFS is beneath the 16-bit noise floor, and a
/// reassociated sum of a handful of `f32` terms lands nearer −140.
///
/// The slices are compared **as given** — for an interleaved stereo render the
/// index [`assert_null_test`] reports counts samples, not frames. Nothing here
/// knows about channels, and a difference is a difference in either of them.
///
/// A zero difference maps to `-inf` rather than to some floor value on purpose:
/// bit-identical is a categorically different answer from "quiet enough", and
/// the two should not be confusable in a log.
pub fn null_test_peak_dbfs(a: &[f32], b: &[f32]) -> f64 {
    let (_, peak) = null_test_peak(a, b);
    20.0 * peak.log10()
}

/// Assert two renders differ by no more than `limit_dbfs`, reporting the
/// measured peak and the sample index where it occurred on failure.
///
/// Both numbers are in the message because neither is enough alone: "exceeded
/// −100 dBFS" does not say whether the change under test sits at −99 dBFS
/// (last-bit reordering that drifted slightly further than expected) or at
/// −12 dBFS (a routing bug), and that difference is the whole judgement. The
/// linear peak rides along so a `-inf`/NaN reading is still legible.
///
/// The comparison is `<=`, so a difference sitting exactly on the limit passes.
pub fn assert_null_test(a: &[f32], b: &[f32], limit_dbfs: f64) {
    let (at, peak) = null_test_peak(a, b);
    let dbfs = 20.0 * peak.log10();
    assert!(
        dbfs <= limit_dbfs,
        "null test failed: peak difference {dbfs:.2} dBFS exceeds the {limit_dbfs:.2} dBFS \
         limit — {peak:.3e} linear at sample {at} of {}",
        a.len()
    );
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

    // ── null test ──────────────────────────────────────────────────────────
    //
    // The "prove it by making it fail" cases. A comparator that passes
    // everything is worse than none at all — it reads as verification in every
    // close-out that quotes it — so the perturbations below land on *known*
    // dBFS values rather than merely somewhere over the limit.

    /// Zeros everywhere except one sample of `b`, so the peak difference is the
    /// perturbation exactly and its dBFS is arithmetic rather than a fit.
    fn one_sample_off(at: usize, by: f32) -> (Vec<f32>, Vec<f32>) {
        let a = vec![0.0_f32; 64];
        let mut b = a.clone();
        b[at] = by;
        (a, b)
    }

    /// The panic payload of a failing `assert_null_test`, as a string.
    fn null_test_failure(a: Vec<f32>, b: Vec<f32>, limit_dbfs: f64) -> String {
        let err = std::panic::catch_unwind(move || assert_null_test(&a, &b, limit_dbfs))
            .expect_err("the difference is over the limit and must fail");
        err.downcast_ref::<String>()
            .cloned()
            .expect("assert! panics with a formatted String")
    }

    #[test]
    fn identical_renders_read_as_negative_infinity() {
        let a: Vec<f32> = (0..256).map(|i| (i as f32 * 0.05).sin()).collect();
        assert_eq!(null_test_peak_dbfs(&a, &a), f64::NEG_INFINITY);
        // …and pass at any limit, including an absurdly strict one.
        assert_null_test(&a, &a, -300.0);
    }

    /// The headline number. `1e-5` of full scale is −100 dBFS, which is E049's
    /// bar, so this pins the reading against the constant every later ticket is
    /// judged on.
    ///
    /// The tolerance is for the *literal*, not the comparator: `1e-5_f32` is
    /// only the nearest `f32` to one part in a hundred thousand, which is a few
    /// times 1e-7 dB off the round decimal. `0.5` is exact and reads exact.
    #[test]
    fn a_known_perturbation_reads_back_at_its_known_level() {
        for (by, want) in [(1e-5_f32, -100.0_f64), (1e-3, -60.0), (0.5, -6.020_599_913_279_624)] {
            let (a, b) = one_sample_off(7, by);
            let got = null_test_peak_dbfs(&a, &b);
            assert!(
                (got - want).abs() < 1e-5,
                "{by} of full scale should read {want} dBFS, got {got}"
            );
        }
    }

    /// A difference sitting exactly on the limit passes — the bar is "at or
    /// below −100 dBFS", not "below".
    #[test]
    fn a_difference_exactly_at_the_limit_passes() {
        let (a, b) = one_sample_off(7, 1e-5);
        assert_null_test(&a, &b, -100.0);
    }

    /// The failure message has to carry both numbers, so assert on both: a
    /// message naming only the limit is the failure mode nobody notices.
    #[test]
    fn a_failure_names_the_measured_peak_and_the_sample() {
        let (a, b) = one_sample_off(41, 1e-3);
        let msg = null_test_failure(a, b, -100.0);
        assert!(msg.contains("-60.00 dBFS"), "message lost the measured peak: {msg}");
        assert!(msg.contains("sample 41"), "message lost the sample index: {msg}");
    }

    /// The peak is the *worst* difference, not the first or the last one.
    #[test]
    fn the_reported_index_is_the_worst_sample_not_the_first() {
        let a = vec![0.0_f32; 64];
        let mut b = a.clone();
        b[5] = 1e-4;
        b[50] = 1e-2;
        b[60] = 1e-3;
        let msg = null_test_failure(a, b, -100.0);
        assert!(msg.contains("sample 50"), "should report the largest difference: {msg}");
    }

    /// A NaN sample must not read as a perfect null. `d > worst` is false for
    /// NaN, so without the explicit check a blown-up render would report −inf —
    /// the most convincing possible pass.
    #[test]
    fn a_nan_render_fails_rather_than_nulling_perfectly() {
        let a = vec![0.0_f32; 8];
        let mut b = a.clone();
        b[3] = f32::NAN;
        assert_eq!(null_test_peak_dbfs(&a, &b), f64::INFINITY);
        let msg = null_test_failure(a, b, -100.0);
        assert!(msg.contains("sample 3"), "{msg}");
    }

    #[test]
    #[should_panic(expected = "same length")]
    fn mismatched_lengths_are_a_harness_error_not_a_null() {
        null_test_peak_dbfs(&[0.0; 8], &[0.0; 9]);
    }

    #[test]
    #[should_panic(expected = "needs a render")]
    fn two_empty_renders_are_a_harness_error_not_a_perfect_null() {
        null_test_peak_dbfs(&[], &[]);
    }
}

/// Drive `process_fn` (stereo in/out) with an `f_hz` sine at 48 kHz for `warm`
/// samples, then return the RMS of the following `measure` samples.
///
/// The shape every "is this filter/damping actually attenuating?" test wants:
/// warm the state up, then measure a steady window. Moved here from `vxn2-dsp`
/// by ticket 0230, with the FDN reverb whose damping test uses it.
pub fn sine_rms(
    mut process_fn: impl FnMut(f32, f32) -> (f32, f32),
    f_hz: f32,
    warm: usize,
    measure: usize,
) -> f32 {
    const SR: f32 = 48_000.0;
    for n in 0..warm {
        let t = n as f32 / SR;
        let s = (t * f_hz * core::f32::consts::TAU).sin();
        let _ = process_fn(s, s);
    }
    let mut e = 0.0_f32;
    for n in 0..measure {
        let t = (warm + n) as f32 / SR;
        let s = (t * f_hz * core::f32::consts::TAU).sin();
        let (l, r) = process_fn(s, s);
        e += l * l + r * r;
    }
    (e / (2.0 * measure as f32)).sqrt()
}
