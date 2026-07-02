/// Shared test helpers for vxn2-dsp unit tests.
///
/// Design notes (ticket 0165):
///
/// - All helpers are `pub(crate)` so they are accessible within any `#[cfg(test)]`
///   module in this crate, which is the only intended consumer.
/// - `run_until_stage` is closure-based to avoid coupling to a specific stage
///   enum. The caller tick-fn returns `true` when the target stage is reached.
/// - `assert_bit_exact_passthrough` uses a fixed internal stereo sine generator
///   (330 Hz L, 110 Hz R, 48 kHz). Both callers (phaser + dynamics) previously
///   used slightly different generators (fast_sine_01 vs TAU·f·sin), but because
///   the assertion is `output == input` (not `output == reference`), the exact
///   signal shape is irrelevant — any non-trivial, deterministic stereo pair
///   proves the passthrough property. The tolerance remains zero bits (bit-exact).
/// - `sine_tail_energy` targets the reverb helpers (`rms_with_damp`). The filter
///   helpers (`mode_energy`, `chain_energy`) have incompatible signatures
///   (they return `f64`, take mode/slope/cutoff args, and the chain variant uses
///   `osc_chain`) — forcing them through a shared helper would add more complexity
///   than it removes, so they remain as file-local helpers in `filter.rs`.
///   `reverb::tail_energy` is an impulse-driver not a sine-driver; it remains a
///   file-local helper too since its setup differs fundamentally from the
///   `rms_with_damp` pattern (continuous tone vs impulse-then-silence).
/// - The S&H threshold mismatch (`lfo1` uses `3..8`, `lfo2` uses `> 5`) is left
///   as-is. `lfo1` counts steps across 1000 blocks at 4 Hz (~5 cycles);
///   `lfo2_sample_hold` counts distinct `sh_value[0]` values across 1000 blocks
///   at 4 Hz but seeds from the first eval. The two tests are measuring slightly
///   different things (step transitions vs distinct held values) so aligning them
///   would be an observable semantic change, not a safe refactor.
/// - `zero_cross_period` (for lfo.rs) and `assert_cooked_hz` (for op.rs) are
///   inlined since the lfo crossing detection is already extracted into local
///   crossings vectors and `assert_cooked_hz` would save only a minor amount of
///   boilerplate over the 4 call sites.  Instead we provide them both as proper
///   helpers per the ticket AC.

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
/// different). Both phaser.rs and dynamics.rs previously used slightly
/// different generators; this unified form is semantically identical.
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
/// bit-exact passthrough for `n` samples. Equivalent to the settle+check
/// pattern in `switch_off_fades_then_settles_to_bit_exact` (phaser + dynamics).
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
/// `warm` samples, then sum the RMS of the following `measure` samples.
///
/// Encapsulates the pattern in `reverb.rs::rms_with_damp` (continuous tone
/// drive → measure tail RMS). Returns the raw mean-square root (RMS).
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

/// `carrier_friendly_patch()` — algo 32 with all ops having `r[3] = 99`.
///
/// Hoisted from the identical definitions in `stack.rs` and `voice.rs` test
/// modules. Both modules re-export via their own local alias that delegates
/// here. Algo 32: all 6 ops are carriers with no modulator edges, so each op
/// runs its own path with no inter-op coupling. The fast release (`R4=99 ≈
/// 4 ms`) makes `is_idle()` reachable in reasonable test time.
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
