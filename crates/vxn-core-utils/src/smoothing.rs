//! One-pole parameter smoother. Removes zipper noise when a host parameter
//! jumps; the synth updates the target once per control block and lets the
//! smoother glide.

/// Samples for a given duration in milliseconds. Truncates, and **may return
/// zero** — a zero-length window is meaningful here (the limiter's lookahead
/// legitimately goes to 0 at its minimum attack).
///
/// For a *fade* window use [`fade_len_samples`] instead: a fade of length 0 is
/// degenerate, not merely short. The two differ by one sample at 44.1 kHz for
/// a 5 ms window (220 vs 221) — see that function's note.
#[inline]
pub fn ms_to_samples(ms: f32, sample_rate: f32) -> usize {
    (ms * 0.001 * sample_rate).max(0.0) as usize
}

/// Fade-window length in samples: **rounds**, and floors at 1 so the ramp
/// always spans a non-degenerate interval.
///
/// Deliberately a second function rather than a merge with [`ms_to_samples`]
/// (ticket 0225). The two have different contracts and neither can adopt the
/// other's without a caller taking the wrong one: a limiter lookahead of 0 is
/// meaningful, a fade window of 0 divides by zero; and rounding matters where
/// truncation silently shortens a fade. They agree at every rate the plugins
/// support except **44.1 kHz**, where a 5 ms window is 221 here and 220 there —
/// which is why folding them together would have moved vxn-1's oversample-change
/// fade by a sample on a shipped product.
#[inline]
pub fn fade_len_samples(ms: f32, sample_rate: f32) -> usize {
    (ms * 0.001 * sample_rate).round().max(1.0) as usize
}

/// Equal-gain raised-cosine rise `0.5 − 0.5·cos(π·t)` for `t ∈ [0,1]`. Zero
/// slope at *both* endpoints, so neither the start nor the steady handoff leaves
/// a slope corner — a corner reads as a click, which is the exact failure this
/// curve fixes.
///
/// One expression, four former copies (vxn-1's `smoothing`, vxn-1b's
/// `output`, and vxn-2's two inline sites in `engine.rs`). All four were
/// byte-identical, so adopting this is bit-exact.
#[inline]
pub fn raised_cosine_rise(t: f32) -> f32 {
    0.5 - 0.5 * (core::f32::consts::PI * t).cos()
}

/// A deterministic equal-gain raised-cosine crossfade between a stage's *dry*
/// input and its *wet* output, armed on a flag edge. Equal-gain (weights sum to
/// 1) because dry and wet are strongly correlated. Idle (`remaining == 0`) it
/// costs nothing: the caller takes its zero-cost passthrough instead.
///
/// **Scope** ([ADR 0002](../../../adrs/0002-vxn-core-dsp.md) §5): this is the
/// primitive for **whole-span** switches — vxn-1's oversample-change crossfade,
/// vxn-2's span fades. Per-FX enable/disable declick is `WetFade`'s job (E041),
/// not this.
pub struct BypassXfade {
    /// Fade window in samples.
    len: usize,
    /// Samples of fade left; `0` ⇒ idle, `> 0` ⇒ fade in flight.
    remaining: usize,
    /// Direction: `true` = dry→wet (engage), `false` = wet→dry (bypass).
    to_wet: bool,
    /// Last-seen flag, for edge detection.
    on: bool,
}

impl BypassXfade {
    pub fn new(len: usize) -> Self {
        Self { len: len.max(1), remaining: 0, to_wet: false, on: false }
    }

    /// Re-idle the fade (transport reset / sample-rate change): drop any
    /// in-flight fade. The edge memory (`on`) is left to the next
    /// [`Self::prime`], so a still-engaged effect doesn't spuriously re-fade.
    pub fn reset(&mut self) {
        self.remaining = 0;
    }

    /// Adopt `on` as the current flag state with no fade — the first-block seed
    /// after construction or a reset, so an effect that starts engaged is simply
    /// on (no startup ramp) and only a genuine user edge arms a fade.
    pub fn prime(&mut self, on: bool) {
        self.on = on;
        self.remaining = 0;
    }

    /// Arm a fade on a flag edge. No-op if the flag is unchanged. Returns `true`
    /// only on the **off→on** edge, so the caller can reset that stage's DSP
    /// state before the wet fades in from a clean tail.
    pub fn arm(&mut self, now_on: bool) -> bool {
        if now_on == self.on {
            return false;
        }
        self.remaining = self.len;
        self.to_wet = now_on;
        self.on = now_on;
        now_on
    }

    /// Whether a fade is in flight this block.
    #[inline]
    pub fn active(&self) -> bool {
        self.remaining > 0
    }

    /// `(w_dry, w_wet)` for sample `i` within the current block (whose start had
    /// `remaining` samples left). `t` spans `[0,1]` across the window and clamps
    /// past its end, so the last fade sample lands exactly on the target.
    #[inline]
    pub fn weights_at(&self, i: usize) -> (f32, f32) {
        let span = (self.len as f32 - 1.0).max(1.0);
        let start = (self.len - self.remaining) as f32;
        let t = ((start + i as f32) / span).min(1.0);
        let rise = raised_cosine_rise(t);
        if self.to_wet { (1.0 - rise, rise) } else { (rise, 1.0 - rise) }
    }

    /// Consume a processed block of `n` samples.
    #[inline]
    pub fn advance(&mut self, n: usize) {
        self.remaining = self.remaining.saturating_sub(n);
    }
}

/// One-pole smoothing coefficient: `1 - exp(-1 / (ms * 0.001 * sr))`. Applied
/// as `y += coeff * (target - y)`. Larger `ms` → slower glide.
#[inline]
pub fn one_pole_coeff(ms: f32, sample_rate: f32) -> f32 {
    let n = (ms * 0.001 * sample_rate).max(1.0);
    1.0 - (-1.0 / n).exp()
}

/// Distance below which the glide snaps to its target instead of crawling down
/// the one-pole's asymptotic tail forever. Without it the value never reaches
/// the target exactly: a mod-wheel released to 0 leaves a residual that, scaled
/// by a wide pitch depth, is an audible offset that takes a few hundred ms to
/// die. 1e-6 is inaudible for the gain/CC values this smooths.
const SNAP_EPS: f32 = 1.0e-6;

/// A smoothed scalar parameter.
///
/// `Copy + Debug` so vxn-2's mod-matrix can hold smoothers in `Copy` state
/// structs and `#[derive(Debug)]` containers (E027/0117 — both synths now
/// share this one definition).
#[derive(Clone, Copy, Debug)]
pub struct Smoothed {
    current: f32,
    target: f32,
    coeff: f32,
}

impl Smoothed {
    /// Create a smoother with the given glide time. Starts settled at `initial`.
    pub fn new(initial: f32, ms: f32, sample_rate: f32) -> Self {
        Self {
            current: initial,
            target: initial,
            coeff: one_pole_coeff(ms, sample_rate),
        }
    }

    /// Change the glide time.
    pub fn set_time(&mut self, ms: f32, sample_rate: f32) {
        self.coeff = one_pole_coeff(ms, sample_rate);
    }

    /// Set the destination value (call once per control block).
    #[inline]
    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Jump immediately to a value, no glide (e.g. on reset / preset load).
    pub fn snap(&mut self, value: f32) {
        self.current = value;
        self.target = value;
    }

    /// Advance one sample toward the target and return the smoothed value.
    #[inline]
    pub fn tick(&mut self) -> f32 {
        self.current += self.coeff * (self.target - self.current);
        if (self.target - self.current).abs() < SNAP_EPS {
            self.current = self.target;
        }
        self.current
    }

    #[inline]
    pub fn current(&self) -> f32 {
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converges_to_target_within_5_tau() {
        // 5 time-constants is conventional "settled" — within ~0.7% of target.
        // Ticket gates this at 1%.
        let sr = 48_000.0;
        let tau_ms = 5.0;
        let mut s = Smoothed::new(0.0, tau_ms, sr);
        s.set_target(1.0);
        let samples = (5.0 * tau_ms * 0.001 * sr) as usize;
        for _ in 0..samples {
            s.tick();
        }
        assert!((s.current() - 1.0).abs() < 0.01, "got {}", s.current());
    }

    #[test]
    fn snap_is_immediate() {
        let mut s = Smoothed::new(0.0, 100.0, 48_000.0);
        s.snap(0.5);
        assert_eq!(s.tick(), 0.5);
    }

    #[test]
    fn settles_exactly_to_target() {
        // Must reach the target *exactly* in bounded time, not crawl the
        // one-pole tail forever: a residual scaled by a wide pitch depth is an
        // audible offset that lingers after the wheel is released to 0.
        let mut s = Smoothed::new(1.0, 20.0, 1_500.0);
        s.set_target(0.0);
        let mut ticks = 0;
        while s.current() != 0.0 {
            s.tick();
            ticks += 1;
            assert!(ticks < 10_000, "never reached exactly 0.0");
        }
    }

    #[test]
    fn one_pole_coeff_in_unit_range() {
        // coeff = 1 - exp(-1/n) ∈ (0, 1] for n ≥ 1.
        let c = one_pole_coeff(5.0, 48_000.0);
        assert!(c > 0.0 && c < 1.0);
        // Degenerate sub-sample time clamps n to 1, coeff = 1 - exp(-1).
        let c0 = one_pole_coeff(0.0, 48_000.0);
        assert!((c0 - (1.0 - (-1.0_f32).exp())).abs() < 1e-6);
    }

    #[test]
    fn ms_to_samples_basic() {
        assert_eq!(ms_to_samples(10.0, 48_000.0), 480);
        assert_eq!(ms_to_samples(-1.0, 48_000.0), 0);
    }

    /// The two length functions are NOT interchangeable, and this pins the
    /// case that proves it — the reason 0225 kept both instead of merging.
    #[test]
    fn fade_len_and_ms_to_samples_differ_at_44k1() {
        assert_eq!(fade_len_samples(5.0, 44_100.0), 221);
        assert_eq!(ms_to_samples(5.0, 44_100.0), 220);
        // They agree at every other rate the plugins support.
        for sr in [48_000.0, 88_200.0, 96_000.0, 176_400.0, 192_000.0] {
            assert_eq!(fade_len_samples(5.0, sr), ms_to_samples(5.0, sr), "sr {sr}");
            assert_eq!(fade_len_samples(10.0, sr), ms_to_samples(10.0, sr), "sr {sr}");
        }
    }

    #[test]
    fn fade_len_never_degenerate() {
        assert_eq!(fade_len_samples(0.0, 48_000.0), 1);
        assert_eq!(fade_len_samples(-5.0, 48_000.0), 1);
    }

    #[test]
    fn raised_cosine_rise_endpoints_and_midpoint() {
        assert!(raised_cosine_rise(0.0).abs() < 1e-7);
        assert!((raised_cosine_rise(0.5) - 0.5).abs() < 1e-6);
        assert!((raised_cosine_rise(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn xfade_weights_are_equal_gain_and_land_on_target() {
        let mut x = BypassXfade::new(64);
        x.prime(false);
        assert!(x.arm(true), "off->on edge must report true");
        assert!(x.active());
        for i in 0..64 {
            let (d, w) = x.weights_at(i);
            assert!((d + w - 1.0).abs() < 1e-6, "weights not equal-gain at {i}");
        }
        // Last sample of the window is fully wet.
        let (d, w) = x.weights_at(63);
        assert!(d.abs() < 1e-6 && (w - 1.0).abs() < 1e-6);
        x.advance(64);
        assert!(!x.active());
    }

    #[test]
    fn xfade_no_edge_no_fade_and_reset_idles() {
        let mut x = BypassXfade::new(32);
        x.prime(true);
        assert!(!x.arm(true), "unchanged flag must not arm");
        assert!(!x.active());
        assert!(!x.arm(false), "on->off edge returns false but still fades");
        assert!(x.active());
        x.reset();
        assert!(!x.active());
    }
}
