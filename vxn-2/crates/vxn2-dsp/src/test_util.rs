/// Shared test helpers for vxn2-dsp unit tests. All `pub(crate)`, consumed only
/// by this crate's `#[cfg(test)]` modules.

use std::f32::consts::TAU;

/// Tick a closure until it returns `true` (stage reached) or `max` ticks elapse.
/// Returns `true` if the stage was reached.
///
/// # Example
/// ```ignore
/// let reached = run_until_stage(
///     || { eg.tick(dt); eg.stage == EgStage::Sustain },
///     200_000,
/// );
/// assert!(reached, "never reached sustain");
/// ```
pub(crate) fn run_until_stage(mut tick: impl FnMut() -> bool, max: usize) -> bool {
    for _ in 0..max {
        if tick() {
            return true;
        }
    }
    false
}

/// Assert that a stereo process function is a **bit-exact passthrough** for
/// `n` consecutive stereo samples.
///
/// Uses a fixed internal two-frequency sine pair (330 Hz L / 110 Hz R at
/// 48 kHz). Panics on the first non-exact sample with an informative message.
///
/// The specific input waveform is immaterial — any non-trivial deterministic
/// pair establishes the passthrough property (output == input, zero bits
/// different).
pub(crate) fn assert_bit_exact_passthrough(mut process_fn: impl FnMut(f32, f32) -> (f32, f32), n: usize) {
    const SR: f32 = 48_000.0;
    for i in 0..n {
        let x = 0.4 * (TAU * 330.0 * i as f32 / SR).sin();
        let y = -0.3 * (TAU * 110.0 * i as f32 / SR).cos();
        let (l, r) = process_fn(x, y);
        assert_eq!(
            l.to_bits(),
            x.to_bits(),
            "L not bit-exact at i={i}: {l} vs {x}"
        );
        assert_eq!(
            r.to_bits(),
            y.to_bits(),
            "R not bit-exact at i={i}: {r} vs {y}"
        );
    }
}

/// Settle `process_fn` for `settle` samples (arbitrary input), then assert
/// bit-exact passthrough for `n` samples.
pub(crate) fn assert_bit_exact_after_settle(
    mut process_fn: impl FnMut(f32, f32) -> (f32, f32),
    settle: usize,
    n: usize,
) {
    for _ in 0..settle {
        process_fn(0.3, 0.3);
    }
    assert_bit_exact_passthrough(process_fn, n);
}

/// Drive `process_fn` (stereo in/out) with a `f_hz`-Hz sine at 48 kHz for
/// `warm` samples, then return the RMS of the following `measure` samples.
pub(crate) fn sine_rms(
    mut process_fn: impl FnMut(f32, f32) -> (f32, f32),
    f_hz: f32,
    warm: usize,
    measure: usize,
) -> f32 {
    const SR: f32 = 48_000.0;
    for n in 0..warm {
        let t = n as f32 / SR;
        let s = (t * f_hz * TAU).sin();
        let _ = process_fn(s, s);
    }
    let mut e = 0.0_f32;
    for n in 0..measure {
        let t = (warm + n) as f32 / SR;
        let s = (t * f_hz * TAU).sin();
        let (l, r) = process_fn(s, s);
        e += l * l + r * r;
    }
    (e / (2.0 * measure as f32)).sqrt()
}

/// Algo 32 with all ops having `r[3] = 99`: all 6 ops are carriers with no
/// modulator edges, so each op runs its own path with no inter-op coupling.
/// The fast release (`R4=99 ≈ 4 ms`) makes `is_idle()` reachable in reasonable
/// test time.
pub(crate) fn carrier_friendly_patch() -> crate::voice::VoiceParams {
    use crate::algo::N_OPS;
    use crate::op::OpParams;
    let mut ops = [OpParams::default(); N_OPS];
    for op in &mut ops {
        op.eg.r[3] = 99;
    }
    crate::voice::VoiceParams {
        ops,
        algo: 32,
        ..crate::voice::VoiceParams::default()
    }
}

/// Measure the zero-crossing period (in blocks) of a slice of per-block LFO
/// values (positive-going zero crossings, block index units).
///
/// Returns `None` if fewer than 2 crossings are found. Otherwise returns the
/// difference between the first two consecutive positive-going crossings.
///
/// Used for lfo.rs period tests. Blocks are the natural unit because the LFO
/// evaluates once per block.
pub(crate) fn zero_cross_period(samples: &[f32]) -> Option<i32> {
    let mut crossings = Vec::new();
    let mut prev = samples[0];
    for (i, &v) in samples.iter().enumerate().skip(1) {
        if prev < 0.0 && v >= 0.0 {
            crossings.push(i);
        }
        prev = v;
    }
    if crossings.len() >= 2 {
        Some((crossings[1] - crossings[0]) as i32)
    } else {
        None
    }
}

/// Assert that an `OpState` cooked with `params` at A4 (key=69, vel=100, sr=48kHz)
/// yields a `phase_inc` within `tol` ULPs of the increment for `expected_hz`.
pub(crate) fn assert_cooked_hz(params: &crate::op::OpParams, expected_hz: f32, tol: u32) {
    use crate::op::{OpState, PM_SCALE_Q32};
    let mut state = OpState::default();
    state.cook(params, 69, 100, 48_000.0);
    let want = ((expected_hz / 48_000.0) * PM_SCALE_Q32) as u32;
    assert!(
        state.phase_inc.abs_diff(want) <= tol,
        "cooked phase_inc {} vs want {} (expected_hz={expected_hz}, tol={tol})",
        state.phase_inc,
        want,
    );
}
