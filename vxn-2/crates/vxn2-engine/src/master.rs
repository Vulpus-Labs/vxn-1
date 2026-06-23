//! Master section (ticket 0012): patch-level tune offset + final output gain.
//!
//! Per PARAMETERS.md the master is two scalars:
//!
//! - `master_tune` (−100..+100 ct) — additive global tuning offset. Applied
//!   per voice at note-on by writing into [`vxn2_dsp::voice::VoiceParams::master_tune_cents`]
//!   on both Upper and Lower layers, then re-cooking. The DSP path already
//!   bakes `master_tune_cents` into each op's base phase increment; this
//!   module just owns the patch-level mirror.
//! - `master_volume` (−60..+6 dB) — final output gain on the FX-chain sum.
//!   Stored linearly to avoid an `exp10` in the per-sample loop. Applied as
//!   the very last multiplier before the engine returns its stereo pair.
//!
//! The default `master_volume = −6 dB` gives ~6 dB of headroom for typical
//! patches. An optional brickwall safety limiter (VXN1 parity, `limiter_on`,
//! off by default) sits last in the FX chain after this gain; when off,
//! over-cooked patches clip into the host bus as before. The limiter DSP
//! object itself lives on the engine — this module only owns the on/off flag
//! alongside the master scalars.

/// `master_tune` range in cents.
pub const MASTER_TUNE_MIN_CT: f32 = -100.0;
pub const MASTER_TUNE_MAX_CT: f32 = 100.0;

/// `master_volume` range in dB.
pub const MASTER_VOL_MIN_DB: f32 = -60.0;
pub const MASTER_VOL_MAX_DB: f32 = 6.0;

/// Default master volume: −6 dB → ~0.501 linear. Six dB of headroom.
pub const MASTER_VOL_DEFAULT_DB: f32 = -6.0;

/// Per-sample smoothing time for master gain. A slider drag or an automation
/// lane writes a new `volume_db` once per control block; smoothing the linear
/// gain toward it kills the zipper noise of a block-rate step (VXN1 parity,
/// 5 ms). Snapped — not glided — on reset / preset load.
pub const MASTER_VOL_SMOOTH_MS: f32 = 5.0;

/// dB → linear gain (`10^(dB / 20)`).
#[inline]
pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Patch-level master parameters. Owned by the engine.
#[derive(Clone, Copy, Debug)]
pub struct MasterParams {
    pub tune_cents: f32,
    pub volume_db: f32,
    /// Brickwall safety limiter on the master bus (VXN1 parity). Off by
    /// default so an unchanged patch stays bit-identical; the limiter object
    /// lives on the engine and only runs when this is set.
    pub limiter_on: bool,
}

impl Default for MasterParams {
    fn default() -> Self {
        Self {
            tune_cents: 0.0,
            volume_db: MASTER_VOL_DEFAULT_DB,
            limiter_on: false,
        }
    }
}

/// Resolved master state: smoothed linear gain + tune in cents. The gain
/// target is refreshed once per control block from [`MasterParams`]; the
/// linear value glides toward it per sample to avoid zipper noise on slider /
/// automation moves.
#[derive(Clone, Copy, Debug)]
pub struct MasterState {
    /// Smoothed linear gain derived from `volume_db`.
    gain: vxn2_dsp::smoother::Smoothed,
    /// Tune in cents (mirrored from params; the engine writes this into
    /// `VoiceParams::master_tune_cents` on both layers before each note-on).
    pub tune_cents: f32,
}

impl MasterState {
    /// Build with the smoother primed at the default gain. Needs the sample
    /// rate to derive the per-sample smoothing coefficient.
    pub fn new(sample_rate: f32) -> Self {
        Self {
            gain: vxn2_dsp::smoother::Smoothed::new(
                db_to_lin(MASTER_VOL_DEFAULT_DB),
                MASTER_VOL_SMOOTH_MS,
                sample_rate,
            ),
            tune_cents: 0.0,
        }
    }

    /// Linear gain target from `params.volume_db`, clamped to range.
    #[inline]
    fn target_gain(params: &MasterParams) -> f32 {
        db_to_lin(params.volume_db.clamp(MASTER_VOL_MIN_DB, MASTER_VOL_MAX_DB))
    }

    /// Push a fresh gain target and mirror the tune. The gain glides toward
    /// the new value over the next blocks; one `exp` per control block, the
    /// per-sample cost is a single multiply-add in [`Self::apply`].
    #[inline]
    pub fn refresh(&mut self, params: &MasterParams) {
        self.gain.set_target(Self::target_gain(params));
        self.tune_cents = params
            .tune_cents
            .clamp(MASTER_TUNE_MIN_CT, MASTER_TUNE_MAX_CT);
    }

    /// Snap the gain to the param value with no glide. Used on reset / preset
    /// load, where a glide from the previous patch would be wrong.
    #[inline]
    pub fn snap(&mut self, params: &MasterParams) {
        self.gain.snap(Self::target_gain(params));
        self.tune_cents = params
            .tune_cents
            .clamp(MASTER_TUNE_MIN_CT, MASTER_TUNE_MAX_CT);
    }

    /// Apply master gain to a stereo sample, ticking the smoother once per
    /// stereo frame. One smoother tick + two multiplies, no branching.
    #[inline]
    pub fn apply(&mut self, l: f32, r: f32) -> (f32, f32) {
        let g = self.gain.tick();
        (l * g, r * g)
    }

    /// Current (smoothed) linear gain, for tests / introspection.
    #[inline]
    pub fn gain(&self) -> f32 {
        self.gain.current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_to_lin_canonical_points() {
        assert!((db_to_lin(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_lin(-6.0) - 0.501_187_2).abs() < 1e-4);
        assert!((db_to_lin(6.0) - 1.995_262).abs() < 1e-4);
        assert!((db_to_lin(-60.0) - 0.001).abs() < 1e-5);
    }

    #[test]
    fn snap_clamps_out_of_range() {
        let mut s = MasterState::new(48_000.0);
        s.snap(&MasterParams {
            tune_cents: 9999.0,
            volume_db: 100.0,
            limiter_on: false,
        });
        assert!((s.gain() - db_to_lin(MASTER_VOL_MAX_DB)).abs() < 1e-6);
        assert_eq!(s.tune_cents, MASTER_TUNE_MAX_CT);

        s.snap(&MasterParams {
            tune_cents: -9999.0,
            volume_db: -200.0,
            limiter_on: false,
        });
        assert!((s.gain() - db_to_lin(MASTER_VOL_MIN_DB)).abs() < 1e-6);
        assert_eq!(s.tune_cents, MASTER_TUNE_MIN_CT);
    }

    #[test]
    fn snap_then_apply_scales_both_channels() {
        let mut s = MasterState::new(48_000.0);
        s.snap(&MasterParams {
            tune_cents: 0.0,
            volume_db: -6.020_6, // ≈ 0.5 linear
            limiter_on: false,
        });
        let (l, r) = s.apply(1.0, -0.4);
        assert!((l - 0.5).abs() < 1e-3);
        assert!((r + 0.2).abs() < 1e-3);
    }

    #[test]
    fn refresh_glides_no_jump() {
        // After a target change the gain must move toward, not jump to, the
        // new value on the first sample — that is the anti-zipper property.
        let mut s = MasterState::new(48_000.0);
        s.snap(&MasterParams::default()); // start at −6 dB
        let start = s.gain();
        s.refresh(&MasterParams {
            tune_cents: 0.0,
            volume_db: 6.0, // jump up to +6 dB
            limiter_on: false,
        });
        let (l, _) = s.apply(1.0, 1.0);
        let after_one = l; // gain after a single tick
        assert!(after_one > start, "gain should rise toward target");
        assert!(
            after_one < db_to_lin(6.0),
            "gain must not jump straight to target (zipper)"
        );
        // Converges to the target over time.
        for _ in 0..4_800 {
            s.apply(1.0, 1.0);
        }
        assert!((s.gain() - db_to_lin(6.0)).abs() < 1e-3);
    }

    #[test]
    fn new_is_minus_6_db() {
        let s = MasterState::new(48_000.0);
        assert!((s.gain() - db_to_lin(-6.0)).abs() < 1e-6);
    }
}
