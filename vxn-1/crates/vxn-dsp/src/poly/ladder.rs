//! Structure-of-arrays poly OTA ladder kernel for the synthesis hot path.
//!
//! Holds `[f32; CHANNELS_PER_LAYER]` state and processes one layer's channels
//! per sample in a branchless loop the compiler auto-vectorises (NEON is 4-wide
//! f32, so 8 channels = 2 SIMD lanes deep). Filter mode/slope are *per-layer*
//! parameters, hoisted outside the lane loop — the inner loop has no
//! data-dependent branches. A heterogeneous second layer is simply a second
//! kernel instance with its own hoisted globals.
//!
//! Index-based lane loops are intentional: they read/write several parallel
//! `[f32; N]` arrays in lockstep and are what the autovectoriser turns into
//! NEON. Iterator/zip forms here would obscure that, so `needless_range_loop`
//! is allowed module-wide.
#![allow(clippy::needless_range_loop)]

use crate::CHANNELS_PER_LAYER;
use crate::ota_ladder::{FilterMode, FilterSlope, OtaLadderCoeffs};
use super::oscillator::tanh_c;

const N: usize = CHANNELS_PER_LAYER;

// ── Ladder mode/slope as zero-sized markers ───────────────────────────────────

/// Mix the ladder's input node + four stage outputs into the active filter
/// response, as a zero-sized type so the `process` lane loop can be
/// monomorphised per (mode, slope) instead of branching on a `FilterMode` enum
/// inside the loop. Mirrors [`WaveKind`] for the oscillator kernels. Bodies
/// match [`FilterMode::mix`] exactly (that scalar fn stays as the readable
/// reference).
trait LadderMix {
    fn mix(e: f32, y: [f32; 4]) -> f32;
}

struct MLp2;
struct MLp4;
struct MHp2;
struct MHp4;
struct MBp2;
struct MBp4;
struct MNotch;

impl LadderMix for MLp2 {
    #[inline(always)]
    fn mix(_e: f32, y: [f32; 4]) -> f32 {
        y[1]
    }
}
impl LadderMix for MLp4 {
    #[inline(always)]
    fn mix(_e: f32, y: [f32; 4]) -> f32 {
        y[3]
    }
}
impl LadderMix for MHp2 {
    #[inline(always)]
    fn mix(e: f32, y: [f32; 4]) -> f32 {
        e - 2.0 * y[0] + y[1]
    }
}
impl LadderMix for MHp4 {
    #[inline(always)]
    fn mix(e: f32, y: [f32; 4]) -> f32 {
        e - 4.0 * y[0] + 6.0 * y[1] - 4.0 * y[2] + y[3]
    }
}
impl LadderMix for MBp2 {
    #[inline(always)]
    fn mix(_e: f32, y: [f32; 4]) -> f32 {
        2.0 * (y[0] - y[1])
    }
}
impl LadderMix for MBp4 {
    #[inline(always)]
    fn mix(_e: f32, y: [f32; 4]) -> f32 {
        4.0 * y[1] - 8.0 * y[2] + 4.0 * y[3]
    }
}
impl LadderMix for MNotch {
    #[inline(always)]
    fn mix(e: f32, y: [f32; 4]) -> f32 {
        e - 2.0 * y[0] + 2.0 * y[1]
    }
}

/// Resolve a runtime `(FilterMode, FilterSlope)` pair to its marker type once,
/// outside the lane loop, binding it to `$m` for `$body`. Notch's slope switch
/// is a no-op (its 2-pole zero is exact), so both slopes map to the same marker.
macro_rules! with_mix {
    ($mode:expr, $slope:expr, $m:ident => $body:expr) => {
        match ($mode, $slope) {
            (FilterMode::Lp, FilterSlope::Pole2) => {
                type $m = MLp2;
                $body
            }
            (FilterMode::Lp, FilterSlope::Pole4) => {
                type $m = MLp4;
                $body
            }
            (FilterMode::Hp, FilterSlope::Pole2) => {
                type $m = MHp2;
                $body
            }
            (FilterMode::Hp, FilterSlope::Pole4) => {
                type $m = MHp4;
                $body
            }
            (FilterMode::Bp, FilterSlope::Pole2) => {
                type $m = MBp2;
                $body
            }
            (FilterMode::Bp, FilterSlope::Pole4) => {
                type $m = MBp4;
                $body
            }
            (FilterMode::Notch, _) => {
                type $m = MNotch;
                $body
            }
        }
    };
}

// ── PolyOtaLadder ─────────────────────────────────────────────────────────────

/// 16-voice OTA-C ladder lowpass (R3109/IR3109-style, Juno-flavoured). Poly
/// sibling of [`crate::ota_ladder::OtaLadderKernel`].
///
/// Coefficients are *interpolated per sample* across each control block: the
/// engine samples the modulators once per block, calls [`set_coeffs`](Self::set_coeffs)
/// with the block target then [`prepare_ramp`](Self::prepare_ramp), and
/// [`process`](Self::process) linearly ramps `(g, k, drive)` from the previous
/// block's values toward it — turning block-stepped cutoff into a smooth
/// piecewise-linear trajectory (no zipper/staircase).
///
/// The nonlinearity is per-stage `tanh` and there is **no** `scale` term — the
/// OTA design does not thin the bass under resonance. `mode` (LP/BP/HP/Notch,
/// see [`FilterMode`]) is a *layer-wide* parameter, hoisted out of the lane
/// loop; the feedback path is always the 4th stage so resonance is identical in
/// every mode.
#[derive(Clone)]
pub struct PolyOtaLadder {
    // Current (interpolated) coefficients, advanced each sample.
    g: [f32; N],
    k: [f32; N],
    drive: [f32; N],
    // Per-sample increments toward the target (set by `prepare_ramp`).
    dg: [f32; N],
    dk: [f32; N],
    dd: [f32; N],
    // Block target coefficients (set by `set_coeffs`).
    tg: [f32; N],
    tk: [f32; N],
    td: [f32; N],
    s0: [f32; N],
    s1: [f32; N],
    s2: [f32; N],
    s3: [f32; N],
    y4: [f32; N],
    mode: FilterMode,
    slope: FilterSlope,
}

impl Default for PolyOtaLadder {
    fn default() -> Self {
        Self::new()
    }
}

impl PolyOtaLadder {
    pub fn new() -> Self {
        Self {
            g: [0.5; N],
            k: [0.0; N],
            drive: [1.0; N],
            dg: [0.0; N],
            dk: [0.0; N],
            dd: [0.0; N],
            tg: [0.5; N],
            tk: [0.0; N],
            td: [1.0; N],
            s0: [0.0; N],
            s1: [0.0; N],
            s2: [0.0; N],
            s3: [0.0; N],
            y4: [0.0; N],
            mode: FilterMode::Lp,
            slope: FilterSlope::Pole4,
        }
    }

    pub fn reset(&mut self) {
        self.s0 = [0.0; N];
        self.s1 = [0.0; N];
        self.s2 = [0.0; N];
        self.s3 = [0.0; N];
        self.y4 = [0.0; N];
    }

    /// Set the filter response + slope (layer-wide). Feedback path is unchanged.
    #[inline]
    pub fn set_response(&mut self, mode: FilterMode, slope: FilterSlope) {
        self.mode = mode;
        self.slope = slope;
    }

    pub fn mode(&self) -> FilterMode {
        self.mode
    }

    pub fn slope(&self) -> FilterSlope {
        self.slope
    }

    /// Set this block's *target* coefficients for voice `v`.
    #[inline]
    pub fn set_coeffs(&mut self, v: usize, c: OtaLadderCoeffs) {
        self.tg[v] = c.g;
        self.tk[v] = c.k;
        self.td[v] = c.drive;
    }

    /// Compute per-sample increments so the current coefficients reach their
    /// targets after exactly `steps` [`process`] calls. `steps <= 1` snaps.
    #[inline]
    pub fn prepare_ramp(&mut self, steps: usize) {
        if steps <= 1 {
            self.snap_coeffs();
            return;
        }
        let inv = 1.0 / steps as f32;
        for v in 0..N {
            self.dg[v] = (self.tg[v] - self.g[v]) * inv;
            self.dk[v] = (self.tk[v] - self.k[v]) * inv;
            self.dd[v] = (self.td[v] - self.drive[v]) * inv;
        }
    }

    /// Jump current coefficients to the targets with no ramp.
    #[inline]
    pub fn snap_coeffs(&mut self) {
        self.g = self.tg;
        self.k = self.tk;
        self.drive = self.td;
        self.dg = [0.0; N];
        self.dk = [0.0; N];
        self.dd = [0.0; N];
    }

    /// Step the ramped coefficients one base-sample toward the block target.
    /// Hoisted out of [`process`] so it ticks at the base rate (once per base
    /// frame) rather than the oversampled rate (once per OS sample) — the
    /// modulators that move the targets only update at block rate, so per-OS
    /// interpolation was redundant. The caller pairs this with
    /// `prepare_ramp(base_frames)` (not `base_frames * os`) so the per-step
    /// slope matches the new tick rate.
    #[inline]
    pub fn tick_coeffs(&mut self) {
        for v in 0..N {
            self.g[v] += self.dg[v];
            self.k[v] += self.dk[v];
            self.drive[v] += self.dd[v];
        }
    }

    /// One sample per voice: `out[v] = ota_ladder(x[v])`, mixed for the mode/slope.
    /// Coefficients are constant within the call — the caller advances them at
    /// the base rate via [`tick_coeffs`]. Dispatches once to the
    /// monomorphised body so the lane loop is branch-free.
    #[inline]
    pub fn process(&mut self, x: &[f32; N], out: &mut [f32; N]) {
        with_mix!(self.mode, self.slope, M => self.process_w::<M>(x, out));
    }

    /// Monomorphised ladder lane loop. `M` is the mode×slope mix marker.
    #[inline(always)]
    fn process_w<M: LadderMix>(&mut self, x: &[f32; N], out: &mut [f32; N]) {
        for v in 0..N {
            let g = self.g[v];
            let fed = self.drive[v] * x[v] - self.k[v] * self.y4[v];

            let u0 = tanh_c(fed);
            let a0 = (u0 - self.s0[v]) * g;
            let y0 = a0 + self.s0[v];
            self.s0[v] = y0 + a0;

            let u1 = tanh_c(y0);
            let a1 = (u1 - self.s1[v]) * g;
            let y1 = a1 + self.s1[v];
            self.s1[v] = y1 + a1;

            let u2 = tanh_c(y1);
            let a2 = (u2 - self.s2[v]) * g;
            let y2 = a2 + self.s2[v];
            self.s2[v] = y2 + a2;

            let u3 = tanh_c(y2);
            let a3 = (u3 - self.s3[v]) * g;
            let y3 = a3 + self.s3[v];
            self.s3[v] = y3 + a3;

            self.y4[v] = y3;
            out[v] = M::mix(fed, [y0, y1, y2, y3]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_mix_markers_match_scalar_oracle() {
        // The 7 specialised `LadderMix` markers used inside `process_w` must
        // match the scalar `FilterMode::mix` reference exactly across every
        // (mode, slope) pair. Both slopes for Notch share `MNotch` because the
        // 2-pole notch is exact (zero at cutoff regardless of resonance) — the
        // scalar oracle returns the same value for both slopes there too.
        use crate::ota_ladder::{FilterMode, FilterSlope};
        let e = 0.37;
        let y = [0.11, -0.42, 0.83, -0.19];
        for &mode in &FilterMode::ALL {
            for &slope in &[FilterSlope::Pole2, FilterSlope::Pole4] {
                let oracle = mode.mix(slope, e, y);
                let marker = with_mix!(mode, slope, M => M::mix(e, y));
                assert!(
                    (oracle - marker).abs() < 1e-7,
                    "{mode:?}/{slope:?}: oracle {oracle} vs marker {marker}"
                );
            }
        }
    }

    #[test]
    fn poly_ladder_stable_and_lowpass() {
        let sr = 48_000.0;
        let mut lad = PolyOtaLadder::new();
        for v in 0..N {
            lad.set_coeffs(v, OtaLadderCoeffs::new(1000.0, sr, 0.5, 1.0));
        }
        lad.snap_coeffs();
        // Feed Nyquist-ish into all lanes; should be attenuated and finite.
        let mut peak = 0.0f32;
        let mut out = [0.0; N];
        for i in 0..4800 {
            let s = if i % 2 == 0 { 0.1 } else { -0.1 };
            let x = [s; N];
            lad.process(&x, &mut out);
            peak = peak.max(out[0].abs());
            assert!(out.iter().all(|y| y.is_finite()));
        }
        assert!(peak < 0.1, "hf not attenuated: {peak}");
    }

    #[test]
    fn ladder_coeffs_interpolate_across_block() {
        // prepare_ramp must land the current coefficients exactly on target
        // after `steps` tick_coeffs calls, ramping linearly (no jump on
        // sample 0). The caller drives the tick at base rate; `process`
        // itself is constant-coefficient within a tick.
        let sr = 48_000.0;
        let mut lad = PolyOtaLadder::new();
        // Start settled at a low cutoff, then target a high one.
        for v in 0..N {
            lad.set_coeffs(v, OtaLadderCoeffs::new(200.0, sr, 0.0, 1.0));
        }
        lad.snap_coeffs();
        let g_start = lad.g[0];
        let target = OtaLadderCoeffs::new(8000.0, sr, 0.0, 1.0);
        for v in 0..N {
            lad.set_coeffs(v, target);
        }
        let steps = 32;
        lad.prepare_ramp(steps);
        // After one tick the coefficient has moved only a fraction of the way.
        lad.tick_coeffs();
        let after_one = lad.g[0];
        assert!(
            after_one > g_start && after_one < target.g,
            "no mid-ramp value: start {g_start}, after1 {after_one}, target {}",
            target.g
        );
        // Remaining ticks land on (≈) the target.
        for _ in 1..steps {
            lad.tick_coeffs();
        }
        assert!(
            (lad.g[0] - target.g).abs() < 1e-5,
            "ramp missed target: {} vs {}",
            lad.g[0],
            target.g
        );
    }
}
