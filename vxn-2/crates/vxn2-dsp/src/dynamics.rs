//! Stereo dynamics block: feed-forward peak compressor followed by a `tanh`
//! saturator, wrapped in a wet/dry smoother so on/off transitions glide
//! instead of click. Mirrors the on/off discipline of phaser / delay / reverb
//! (`vxn-2/crates/vxn2-dsp/src/phaser.rs:347-394`): a steady `on = false`
//! block is a bit-exact passthrough; switching off fades the wet to 0 before
//! reverting to passthrough; switching on glides up from the faded-out 0; the
//! envelope follower resets across a fully-faded inactive interval so
//! re-engaging can't dump a stale gain reduction (same shape as the master
//! limiter's `limiter_was_on` reset at
//! `vxn-2/crates/vxn2-engine/src/engine.rs:1216-1228`).
//!
//! Channel-strip order: **comp → sat**. Compressing first evens the
//! transients the saturator drives into, so the harmonic content is
//! consistent across input dynamics; the reverse would let saturation peaks
//! defeat the comp's detector.
//!
//! Surface params are eight: `on, threshold_db, ratio, attack_ms,
//! release_ms, makeup_db, drive_db, mix`. Knee width, detector mode
//! (peak / RMS), and saturator flavour stay internal defaults — same
//! discipline as the phaser pinning stages / centre / spread.

use crate::math::fast_tanh;
use crate::smoother::{one_pole_coeff, Smoothed};

/// Dry/wet glide. Long enough to mask switch-on / switch-off (no click),
/// short enough to feel instant. Matches phaser / delay / reverb.
const MIX_SMOOTH_MS: f32 = 30.0;

/// Soft-knee width in dB. Internal default (not exposed) — same discipline as
/// the phaser pinning stages / centre / spread.
const KNEE_DB: f32 = 6.0;

/// `20 / log2(10)`: converts a log2 magnitude to dB.
const LOG2_TO_DB: f32 = 6.020_6;
/// `log2(10) / 20`: inverse of [`LOG2_TO_DB`], so `(db * DB_TO_LOG2).exp2()`
/// equals `10^(db / 20)` at the cost of one `exp2` instead of one `powf`.
const DB_TO_LOG2: f32 = 1.0 / LOG2_TO_DB;

// ── Params struct (engine-facing snapshot; mirrors `PhaserParams`) ───────────

/// Block-rate parameter snapshot the engine fans into [`DynamicsBlock`].
/// Host-automation only — not a mod-matrix destination (matches phaser).
#[derive(Clone, Copy, Debug)]
pub struct DynamicsParams {
    pub on: bool,
    /// Compressor threshold, dBFS. Clamped −60..0 by `set_from`.
    pub threshold_db: f32,
    /// Compression ratio. Clamped 1..20 (1 = no compression).
    pub ratio: f32,
    /// Attack time, ms. Clamped 0.1..200.
    pub attack_ms: f32,
    /// Release time, ms. Clamped 5..1000.
    pub release_ms: f32,
    /// Post-comp pre-sat linear makeup gain, dB. Clamped 0..24.
    pub makeup_db: f32,
    /// Saturator input drive, dB. Clamped 0..36. `0` collapses the saturator
    /// to identity (no harmonic content).
    pub drive_db: f32,
    /// Dry/wet on the comp+sat chain. Clamped 0..1.
    pub mix: f32,
}

impl Default for DynamicsParams {
    fn default() -> Self {
        Self {
            on: false,
            threshold_db: -12.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            makeup_db: 0.0,
            drive_db: 0.0,
            mix: 1.0,
        }
    }
}

// ── DynamicsBlock ────────────────────────────────────────────────────────────

/// Stereo dynamics: feed-forward linked-sidechain peak compressor → `tanh`
/// saturator, with a single wet/dry smoother wrapping the pair.
#[derive(Clone)]
pub struct DynamicsBlock {
    sample_rate: f32,
    // Compressor
    /// Peak-envelope follower (linear amplitude).
    env: f32,
    attack_coeff: f32,
    release_coeff: f32,
    threshold_db: f32,
    ratio: f32,
    makeup_lin: f32,
    // Saturator: precomputed at param update so the hot path costs one
    // `fast_tanh` + one multiply per channel.
    drive_lin: f32,
    tanh_drive: f32,
    // Wet/dry — same retarget-on-enable / snap-on-first-set as phaser.
    mix: Smoothed,
    mix_primed: bool,
    enabled: bool,
    /// `enabled || mix.current() > 0` on the previous active call. The
    /// detector is reset on the inactive→active edge so a fully-faded
    /// inactive interval doesn't dump stale gain reduction on re-engage
    /// (mirrors the master limiter's `limiter_was_on` pattern at
    /// `vxn-2/crates/vxn2-engine/src/engine.rs:1216-1228`).
    was_active: bool,
}

impl DynamicsBlock {
    pub fn new(sample_rate: f32) -> Self {
        let p = DynamicsParams::default();
        Self {
            sample_rate,
            env: 0.0,
            attack_coeff: one_pole_coeff(p.attack_ms, sample_rate),
            release_coeff: one_pole_coeff(p.release_ms, sample_rate),
            threshold_db: p.threshold_db,
            ratio: p.ratio,
            makeup_lin: 1.0,
            drive_lin: 0.0,
            tanh_drive: 0.0,
            mix: Smoothed::new(0.0, MIX_SMOOTH_MS, sample_rate),
            mix_primed: false,
            enabled: true,
            was_active: false,
        }
    }

    /// Clear the envelope follower. Smoother target is preserved (matches
    /// `StereoDelay::reset`).
    pub fn clear(&mut self) {
        self.env = 0.0;
        self.was_active = false;
    }

    /// Enable/bypass. Disabling pulls the wet smoother to 0 so the wet fades
    /// out cleanly; `process` reverts to bit-exact passthrough only once the
    /// fade has actually reached 0.
    #[inline]
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.mix.set_target(0.0);
        }
    }

    /// Engine-facing setter: fans a snapshot (incl. on/off) into the DSP.
    /// Coefficients update unconditionally — they're cheap and we want them
    /// in place for the moment the block re-activates. The wet/dry smoother
    /// retargets to the param mix when enabled, 0 when bypassed; the first
    /// call snaps so a patch loaded with dynamics already set doesn't ride
    /// in on a fade.
    pub fn set_from(&mut self, p: &DynamicsParams) {
        self.set_enabled(p.on);
        self.threshold_db = p.threshold_db.clamp(-60.0, 0.0);
        self.ratio = p.ratio.clamp(1.0, 20.0);
        let attack_ms = p.attack_ms.clamp(0.1, 200.0);
        let release_ms = p.release_ms.clamp(5.0, 1000.0);
        self.attack_coeff = one_pole_coeff(attack_ms, self.sample_rate);
        self.release_coeff = one_pole_coeff(release_ms, self.sample_rate);
        self.makeup_lin = (p.makeup_db.clamp(0.0, 24.0) * DB_TO_LOG2).exp2();
        // drive_db → drive_lin via `10^(db/20) − 1`: at 0 dB drive_lin is 0,
        // so the saturator collapses to identity (lim_{k→0} tanh(k·x)/tanh(k)
        // = x). At 36 dB drive_lin ≈ 62, with tanh(62) clamped to 1 so the
        // output peak on a ±1 input is exactly 1 — unity gain at full drive.
        let drive_db = p.drive_db.clamp(0.0, 36.0);
        self.drive_lin = (drive_db * DB_TO_LOG2).exp2() - 1.0;
        self.tanh_drive = fast_tanh(self.drive_lin);

        let target = if self.enabled { p.mix.clamp(0.0, 1.0) } else { 0.0 };
        if self.mix_primed {
            self.mix.set_target(target);
        } else {
            self.mix.snap(target);
            self.mix_primed = true;
        }
    }

    /// One stereo sample in / out. Bit-exact passthrough once a switch-off
    /// fade has fully reached 0; otherwise runs comp → sat → crossfade.
    #[inline]
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        if !self.enabled && self.mix.current() == 0.0 {
            // Steady bypass — zero per-sample work beyond this gate check,
            // and the next set_from(on = true) sees was_active = false and
            // resets the detector before the first active sample.
            self.was_active = false;
            return (in_l, in_r);
        }
        if !self.was_active {
            self.env = 0.0;
            self.was_active = true;
        }

        // Feed-forward peak detector, linked L/R sidechain.
        let peak = in_l.abs().max(in_r.abs());
        let coeff = if peak > self.env {
            self.attack_coeff
        } else {
            self.release_coeff
        };
        self.env += coeff * (peak - self.env);

        // Soft-knee static curve in dB. One log2 + one exp2 per sample (both
        // behind the bypass gate, so a steady off block pays neither).
        let env_safe = self.env.max(1.0e-9);
        let env_db = env_safe.log2() * LOG2_TO_DB;
        let over = env_db - self.threshold_db;
        let slope = 1.0 - 1.0 / self.ratio;
        let gr_db = if 2.0 * over <= -KNEE_DB {
            0.0
        } else if 2.0 * over >= KNEE_DB {
            -over * slope
        } else {
            // Quadratic interp across the knee: gr_db is C¹ at both edges.
            let k = over + KNEE_DB * 0.5;
            -slope * k * k / (2.0 * KNEE_DB)
        };
        let comp_gain = (gr_db * DB_TO_LOG2).exp2() * self.makeup_lin;

        let cl = in_l * comp_gain;
        let cr = in_r * comp_gain;

        // Saturator: `tanh(k·x)/tanh(k)`. At k = 0 (drive_db = 0) the
        // limit is x — early-out so we don't divide by 0.
        let (sl, sr) = if self.drive_lin > 1.0e-6 {
            let inv = 1.0 / self.tanh_drive;
            (
                fast_tanh(self.drive_lin * cl) * inv,
                fast_tanh(self.drive_lin * cr) * inv,
            )
        } else {
            (cl, cr)
        };

        // Linear crossfade — the wet is just a level/shape-modified dry, not
        // a decorrelated tail, so we don't need delay's equal-power sqrt.
        let m = self.mix.tick();
        let dry_gain = 1.0 - m;
        (dry_gain * in_l + m * sl, dry_gain * in_r + m * sr)
    }

    #[cfg(test)]
    pub(crate) fn detector_env(&self) -> f32 {
        self.env
    }

    #[cfg(test)]
    pub(crate) fn mix_current(&self) -> f32 {
        self.mix.current()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const SR: f32 = 48_000.0;

    fn params_off() -> DynamicsParams {
        DynamicsParams {
            on: false,
            ..DynamicsParams::default()
        }
    }

    #[test]
    fn off_from_load_is_bit_exact_from_first_sample() {
        // Engine path: set_from with on = false at load time. First set_from
        // snaps the mix to 0, so process is a bit-exact passthrough from
        // sample 0 — no startup fade-in (gate matches phaser /
        // delay / reverb).
        let mut d = DynamicsBlock::new(SR);
        d.set_from(&DynamicsParams {
            on: false,
            threshold_db: -20.0,
            ratio: 8.0,
            attack_ms: 5.0,
            release_ms: 80.0,
            makeup_db: 6.0,
            drive_db: 18.0,
            mix: 1.0,
        });
        for i in 0..1_000 {
            let phase = (i as f32 * 330.0 / SR).fract();
            let x = 0.4 * (TAU * phase).sin();
            let y = -0.3 * (TAU * (i as f32 * 110.0 / SR).fract()).sin();
            let (l, r) = d.process(x, y);
            assert_eq!(l.to_bits(), x.to_bits(), "L not bit-exact at i={i}: {l} vs {x}");
            assert_eq!(r.to_bits(), y.to_bits(), "R not bit-exact at i={i}: {r} vs {y}");
        }
    }

    #[test]
    fn switch_on_after_load_off_glides_up_from_zero() {
        // Loaded with on = false ⇒ mix snapped to 0. The subsequent
        // on = true must retarget the smoother (set_target, not snap), so
        // the first wet-active call sees mix.current() at the smoother's
        // first-tick value — not the param target — confirming the
        // fade-in is active.
        let mut d = DynamicsBlock::new(SR);
        d.set_from(&params_off());
        assert_eq!(d.mix_current(), 0.0, "load-off should leave mix at 0");

        d.set_from(&DynamicsParams {
            on: true,
            mix: 1.0,
            ..DynamicsParams::default()
        });
        assert_eq!(
            d.mix_current(),
            0.0,
            "set_from after first_set should retarget, not snap"
        );

        // One sample tick: mix should advance a tiny step from 0 toward 1.
        d.process(0.0, 0.0);
        let m = d.mix_current();
        assert!(m > 0.0, "first tick should advance mix from 0 (got {m})");
        assert!(m < 0.01, "first tick should not jump to target (got {m})");
    }

    #[test]
    fn switch_off_fades_then_settles_to_bit_exact() {
        // Switching off mid-render must fade the wet to 0 (no click) and
        // only then revert to bit-exact passthrough.
        let mut d = DynamicsBlock::new(SR);
        let on = DynamicsParams {
            on: true,
            threshold_db: -24.0,
            ratio: 8.0,
            attack_ms: 5.0,
            release_ms: 80.0,
            makeup_db: 0.0,
            drive_db: 18.0,
            mix: 1.0,
        };
        d.set_from(&on);
        // Warm up: drive the comp + sat with a steady-ish signal so the wet
        // is meaningfully different from dry by the time we switch off.
        for i in 0..2_000 {
            let phase = (i as f32 * 440.0 / SR).fract();
            let x = 0.6 * (TAU * phase).sin();
            d.process(x, x);
        }
        // Snapshot the last wet sample.
        let probe_x = 0.6 * (TAU * (2_000.0 * 440.0 / SR).fract()).sin();
        let before = d.process(probe_x, probe_x);
        assert!(
            (before.0 - probe_x).abs() > 1.0e-4,
            "warm-up wet should diverge from dry: before={:?}, dry={probe_x}",
            before
        );

        // Switch off — keep the comp/sat params unchanged so the only thing
        // changing across the edge is the `on` flag. The very next sample is
        // still wet (fade just started), but the L channel must be close to
        // the previous wet output (no click / no snap to dry).
        d.set_from(&DynamicsParams { on: false, ..on });
        let after = d.process(probe_x, probe_x);
        assert!(
            (after.0 - before.0).abs() < 0.05,
            "switch-off jumped: before={:?} after={:?}",
            before,
            after
        );
        assert!(
            (after.0 - probe_x).abs() > 1.0e-4,
            "switch-off was instant (already dry): after={:?}",
            after
        );

        // Settle past the smoother tail (~14·τ ≈ 0.4 s for τ = 30 ms).
        let settle = (SR * 0.6) as usize;
        for _ in 0..settle {
            d.process(0.3, 0.3);
        }
        // Now bit-exact passthrough — assert against arbitrary input.
        for i in 0..1_000 {
            let phase = (i as f32 * 330.0 / SR).fract();
            let x = 0.4 * (TAU * phase).sin();
            let y = -0.3 * (TAU * (i as f32 * 110.0 / SR).fract()).sin();
            let (l, r) = d.process(x, y);
            assert_eq!(l.to_bits(), x.to_bits(), "L not bit-exact: {l} vs {x}");
            assert_eq!(r.to_bits(), y.to_bits(), "R not bit-exact: {r} vs {y}");
        }
    }

    #[test]
    fn gain_reduction_matches_known_threshold_ratio() {
        // Threshold −20 dB, ratio 4, step input at 0 dBFS, mix = 1, makeup
        // = 0 ⇒ steady-state output level should be the compressed level,
        // i.e. gain reduction of (20 dB over) × (1 − 1/4) = 15 dB. Output
        // settles to ≈ 10^(−15/20) = 0.1778.
        let mut d = DynamicsBlock::new(SR);
        d.set_from(&DynamicsParams {
            on: true,
            threshold_db: -20.0,
            ratio: 4.0,
            attack_ms: 5.0,
            release_ms: 50.0,
            makeup_db: 0.0,
            drive_db: 0.0,
            mix: 1.0,
        });
        // Settle: well past the smoother (30 ms) and the attack (5 ms).
        let mut last = 0.0_f32;
        for _ in 0..(SR * 0.2) as usize {
            let (l, _) = d.process(1.0, 1.0);
            last = l;
        }
        let gr_db = 20.0 * last.abs().max(1.0e-9).log10();
        assert!(
            (gr_db - (-15.0)).abs() < 0.5,
            "steady-state gr_db = {gr_db}, expected ≈ −15 ± 0.5"
        );
    }

    #[test]
    fn tanh_drive_flattens_sine() {
        // At drive_db = 24 the saturator (`tanh(k·x)/tanh(k)`, k ≈ 14.85)
        // pushes most of a ±1 sine to ±1 — the waveform is flattened toward
        // a square. Peak stays ≤ 1.0 (unity gain at full drive); the RMS
        // climbs well above a sine's 0.707·peak baseline.
        //
        // Disable the comp by using ratio = 1 and feed mix = 1 so the
        // observed level is the saturator output, not the dry blend.
        let mut d = DynamicsBlock::new(SR);
        d.set_from(&DynamicsParams {
            on: true,
            threshold_db: 0.0,
            ratio: 1.0,
            attack_ms: 1.0,
            release_ms: 50.0,
            makeup_db: 0.0,
            drive_db: 24.0,
            mix: 1.0,
        });
        // Skip the smoother fade-in for the measurement (mix is snapped to
        // 1.0 already, so this is just paranoia / clarity).
        for i in 0..512 {
            let phase = (i as f32 * 440.0 / SR).fract();
            let x = (TAU * phase).sin();
            d.process(x, x);
        }
        let n = (SR * 0.02) as usize; // 20 ms ≈ 9 cycles at 440 Hz
        let mut peak = 0.0_f32;
        let mut sum_sq = 0.0_f32;
        for i in 0..n {
            let phase = ((i + 512) as f32 * 440.0 / SR).fract();
            let x = (TAU * phase).sin();
            let (l, _) = d.process(x, x);
            peak = peak.max(l.abs());
            sum_sq += l * l;
        }
        let rms = (sum_sq / n as f32).sqrt();
        assert!(peak <= 1.001, "peak {peak} exceeds unity-gain ceiling");
        assert!(
            rms > 0.85,
            "rms {rms} should be flattened above sine baseline 0.707"
        );
    }

    #[test]
    fn detector_resets_on_inactive_to_active_edge() {
        // Drive the env up, switch off, hold the env at saturation through
        // the fade (so the inactive transition leaves env ≈ 1.0), wait for
        // the fade to fully settle (process becomes bit-exact passthrough,
        // env frozen at 1.0), then switch on. The very first active sample
        // must see env reset to 0 — confirmed by inspecting the detector.
        let mut d = DynamicsBlock::new(SR);
        d.set_from(&DynamicsParams {
            on: true,
            threshold_db: -30.0,
            ratio: 8.0,
            attack_ms: 0.5,
            release_ms: 1000.0, // slow release so env stays high during fade
            makeup_db: 0.0,
            drive_db: 0.0,
            mix: 1.0,
        });
        // Hammer env up to ≈ 1.0.
        for _ in 0..2_000 {
            d.process(1.0, 1.0);
        }
        assert!(
            d.detector_env() > 0.9,
            "warm-up failed to drive env up: env = {}",
            d.detector_env()
        );

        // Switch off; hold input loud so env keeps tracking near 1.0 across
        // the smoother fade.
        d.set_from(&DynamicsParams {
            on: false,
            threshold_db: -30.0,
            ratio: 8.0,
            attack_ms: 0.5,
            release_ms: 1000.0,
            makeup_db: 0.0,
            drive_db: 0.0,
            mix: 1.0,
        });
        for _ in 0..(SR * 0.6) as usize {
            d.process(1.0, 1.0);
        }
        // Fade has settled — passthrough engaged, env frozen near 1.0.
        assert_eq!(d.mix_current(), 0.0, "mix should have fully faded to 0");
        assert!(
            d.detector_env() > 0.5,
            "env should still be high after fade (frozen during passthrough): env = {}",
            d.detector_env()
        );

        // Switch on again.
        d.set_from(&DynamicsParams {
            on: true,
            threshold_db: -30.0,
            ratio: 8.0,
            attack_ms: 0.5,
            release_ms: 1000.0,
            makeup_db: 0.0,
            drive_db: 0.0,
            mix: 1.0,
        });
        // First active process call: env should be reset to 0 before the
        // peak detector pushes its first sample in. With input 0.0, env
        // remains exactly 0.
        let (_l, _r) = d.process(0.0, 0.0);
        assert_eq!(
            d.detector_env(),
            0.0,
            "detector should have been reset on inactive→active edge"
        );
    }

    #[test]
    fn mix_zero_is_dry() {
        // mix = 0 keeps the dry path even with on = true; useful as a sanity
        // check that the comp + sat don't leak into the output.
        let mut d = DynamicsBlock::new(SR);
        d.set_from(&DynamicsParams {
            on: true,
            threshold_db: -40.0,
            ratio: 20.0,
            attack_ms: 0.5,
            release_ms: 100.0,
            makeup_db: 12.0,
            drive_db: 24.0,
            mix: 0.0,
        });
        for i in 0..1_000 {
            let phase = (i as f32 * 220.0 / SR).fract();
            let x = 0.4 * (TAU * phase).sin();
            let (l, _) = d.process(x, -x);
            assert!((l - x).abs() < 1.0e-6, "L not dry at i={i}: {l} vs {x}");
        }
    }
}
