/// Shared test helpers for vxn2-dsp unit tests. All `pub(crate)`, consumed only
/// by this crate's `#[cfg(test)]` modules.


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

// The bit-exact pair and `sine_rms` are canonical in vxn-core-dsp (0226,
// 0230). Nothing in this crate calls them any more — their users were the
// phaser and the FDN reverb, both of which have moved there — so the
// re-export shim is gone rather than kept as a dangling alias.

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
