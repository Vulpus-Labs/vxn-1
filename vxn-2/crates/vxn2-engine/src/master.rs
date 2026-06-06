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
//! Per ADR §10 review (and confirmed by the ticket AC) there is no output
//! limiter. The default `master_volume = −6 dB` gives ~6 dB of headroom for
//! typical patches; over-cooked patches clip into the host bus, which is the
//! user's responsibility (and the DAW's chance to limit).

/// `master_tune` range in cents.
pub const MASTER_TUNE_MIN_CT: f32 = -100.0;
pub const MASTER_TUNE_MAX_CT: f32 = 100.0;

/// `master_volume` range in dB.
pub const MASTER_VOL_MIN_DB: f32 = -60.0;
pub const MASTER_VOL_MAX_DB: f32 = 6.0;

/// Default master volume: −6 dB → ~0.501 linear. Six dB of headroom.
pub const MASTER_VOL_DEFAULT_DB: f32 = -6.0;

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
}

impl Default for MasterParams {
    fn default() -> Self {
        Self {
            tune_cents: 0.0,
            volume_db: MASTER_VOL_DEFAULT_DB,
        }
    }
}

/// Resolved master state: linear gain + tune in cents. Refreshed once per
/// control block from [`MasterParams`].
#[derive(Clone, Copy, Debug)]
pub struct MasterState {
    /// Linear gain derived from `volume_db`.
    pub gain: f32,
    /// Tune in cents (mirrored from params; the engine writes this into
    /// `VoiceParams::master_tune_cents` on both layers before each note-on).
    pub tune_cents: f32,
}

impl Default for MasterState {
    fn default() -> Self {
        Self {
            gain: db_to_lin(MASTER_VOL_DEFAULT_DB),
            tune_cents: 0.0,
        }
    }
}

impl MasterState {
    /// Recompute linear gain from `params.volume_db` and mirror the tune.
    /// Cheap — one `exp` per control block, no per-sample cost.
    #[inline]
    pub fn refresh(&mut self, params: &MasterParams) {
        let db = params.volume_db.clamp(MASTER_VOL_MIN_DB, MASTER_VOL_MAX_DB);
        self.gain = db_to_lin(db);
        self.tune_cents = params
            .tune_cents
            .clamp(MASTER_TUNE_MIN_CT, MASTER_TUNE_MAX_CT);
    }

    /// Apply master gain to a stereo sample. The whole point of this module:
    /// one multiply per channel, no branching.
    #[inline]
    pub fn apply(&self, l: f32, r: f32) -> (f32, f32) {
        (l * self.gain, r * self.gain)
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
    fn refresh_clamps_out_of_range() {
        let mut s = MasterState::default();
        s.refresh(&MasterParams {
            tune_cents: 9999.0,
            volume_db: 100.0,
        });
        assert!((s.gain - db_to_lin(MASTER_VOL_MAX_DB)).abs() < 1e-6);
        assert_eq!(s.tune_cents, MASTER_TUNE_MAX_CT);

        s.refresh(&MasterParams {
            tune_cents: -9999.0,
            volume_db: -200.0,
        });
        assert!((s.gain - db_to_lin(MASTER_VOL_MIN_DB)).abs() < 1e-6);
        assert_eq!(s.tune_cents, MASTER_TUNE_MIN_CT);
    }

    #[test]
    fn apply_scales_both_channels() {
        let s = MasterState {
            gain: 0.5,
            tune_cents: 0.0,
        };
        let (l, r) = s.apply(1.0, -0.4);
        assert_eq!(l, 0.5);
        assert_eq!(r, -0.2);
    }

    #[test]
    fn default_is_minus_6_db() {
        let s = MasterState::default();
        assert!((s.gain - db_to_lin(-6.0)).abs() < 1e-6);
    }
}
