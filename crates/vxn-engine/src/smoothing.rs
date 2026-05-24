//! Engine-side parameter smoothing for *gain-like* parameters.
//!
//! Raw parameter targets (host automation, UI edits, preset loads) arrive in
//! steps. Feeding them straight into per-sample gains produces zipper noise.
//! This layer sets smoothing *targets* once per control block and glides toward
//! them, matching glide granularity to where each parameter is consumed:
//!
//! - **Per-sample** ([`ParamId::MasterVolume`]): the final gain multiply runs
//!   per output sample, so its smoother ticks per sample.
//! - **Block-rate** (oscillator/noise levels, pulse width, mod-matrix depths):
//!   read once per control block into [`crate::voice::BlockCtx`], so one glide
//!   step per block (control rate = sr / `CONTROL_BLOCK` ≈ 1.5 kHz) is enough
//!   to take the audible edge off automation steps.
//! - **Snap** (everything else: enums, bools, ADSR times, pitch, LFO/effect
//!   rates, and crucially cutoff/resonance/drive): discrete, cached, or — for
//!   the filter — smoothed downstream by per-sample *coefficient* interpolation
//!   in [`vxn_dsp::PolyLadder`], which handles automation, LFO and envelope
//!   modulation uniformly. Smoothing the cutoff *value* here too would be
//!   redundant, so these jump.

use crate::{ParamId, ParamValues};
use vxn_dsp::{CONTROL_BLOCK, Smoothed, one_pole_coeff};

/// Glide time for block-rate smoothed params (ms).
const BLOCK_SMOOTH_MS: f32 = 10.0;
/// Glide time for the per-sample master volume (ms).
const VOLUME_SMOOTH_MS: f32 = 5.0;

/// How a parameter is smoothed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Glide {
    /// Jump to target (discrete / cached / smoothed downstream).
    Snap,
    /// One glide step per control block.
    Block,
    /// Glided per output sample by the dedicated volume smoother.
    PerSample,
}

#[inline]
fn glide_of(id: ParamId) -> Glide {
    use ParamId::*;
    // All mod-matrix depth params glide at block rate, wherever they sit.
    if ParamId::is_matrix_param(id.index()) {
        return Glide::Block;
    }
    match id {
        MasterVolume => Glide::PerSample,
        Osc1Level | Osc2Level | NoiseLevel | Osc1PulseWidth | Osc2PulseWidth | CrossMod
        | ModWheelDepth => Glide::Block,
        _ => Glide::Snap,
    }
}

/// Smooths gain-like parameter values between the raw target store and the
/// engine's per-block read. Cutoff/resonance/drive are deliberately *not*
/// handled here — the ladder interpolates their coefficients per sample.
pub struct ParamSmoother {
    /// Smoothed current values for every param. Block-rate params glide here;
    /// snap params mirror their target each block; the per-sample volume value
    /// is taken from [`Self::next_volume`] instead.
    current: ParamValues,
    /// One-pole coefficient at the control rate (block-rate glide).
    block_coeff: f32,
    /// Dedicated per-sample smoother for master volume.
    volume: Smoothed,
}

impl ParamSmoother {
    pub fn new(sample_rate: f32, targets: &ParamValues) -> Self {
        let control_rate = sample_rate / CONTROL_BLOCK as f32;
        Self {
            current: targets.clone(),
            block_coeff: one_pole_coeff(BLOCK_SMOOTH_MS, control_rate),
            volume: Smoothed::new(
                targets.get(ParamId::MasterVolume),
                VOLUME_SMOOTH_MS,
                sample_rate,
            ),
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        let control_rate = sample_rate / CONTROL_BLOCK as f32;
        self.block_coeff = one_pole_coeff(BLOCK_SMOOTH_MS, control_rate);
        self.volume.set_time(VOLUME_SMOOTH_MS, sample_rate);
    }

    /// Jump every smoothed value to its target (reset / sample-rate change).
    pub fn snap_all(&mut self, targets: &ParamValues) {
        self.current = targets.clone();
        self.volume.snap(targets.get(ParamId::MasterVolume));
    }

    /// Advance block-rate smoothers one step toward `targets`, snap the rest,
    /// and arm the per-sample volume target. Call once per control block.
    pub fn tick_block(&mut self, targets: &ParamValues) {
        for id in ParamId::all() {
            match glide_of(id) {
                Glide::Block => {
                    let cur = self.current.get(id);
                    let t = targets.get(id);
                    self.current.set(id, cur + self.block_coeff * (t - cur));
                }
                Glide::Snap => self.current.set(id, targets.get(id)),
                Glide::PerSample => {
                    self.volume.set_target(targets.get(id));
                    self.current.set(id, targets.get(id));
                }
            }
        }
    }

    /// The block-rate-smoothed parameter view the engine reads each block.
    #[inline]
    pub fn values(&self) -> &ParamValues {
        &self.current
    }

    /// Advance and return the per-sample master volume.
    #[inline]
    pub fn next_volume(&mut self) -> f32 {
        self.volume.tick()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets_with(id: ParamId, v: f32) -> ParamValues {
        let mut p = ParamValues::default();
        p.set(id, v);
        p
    }

    #[test]
    fn block_param_glides_toward_target() {
        let mut s = ParamSmoother::new(48_000.0, &ParamValues::default());
        let start = ParamValues::default().get(ParamId::Osc1Level);
        let targets = targets_with(ParamId::Osc1Level, 0.0);
        // After one block it has moved, but not all the way.
        s.tick_block(&targets);
        let after_one = s.values().get(ParamId::Osc1Level);
        assert!(
            after_one < start && after_one > 0.0,
            "no glide: {after_one}"
        );
        // After many blocks it converges.
        for _ in 0..2000 {
            s.tick_block(&targets);
        }
        assert!(s.values().get(ParamId::Osc1Level).abs() < 1e-3);
    }

    #[test]
    fn snap_params_jump_immediately() {
        // Cutoff is snapped here (ladder interpolates its coeffs downstream).
        let mut s = ParamSmoother::new(48_000.0, &ParamValues::default());
        let targets = targets_with(ParamId::Cutoff, 100.0);
        s.tick_block(&targets);
        assert_eq!(s.values().get(ParamId::Cutoff), 100.0);
    }

    #[test]
    fn matrix_depths_are_block_smoothed() {
        assert_eq!(glide_of(ParamId::LfoCutoff), Glide::Block);
        assert_eq!(glide_of(ParamId::Env2Amp), Glide::Block);
    }

    #[test]
    fn volume_glides_per_sample() {
        let mut s = ParamSmoother::new(48_000.0, &ParamValues::default());
        let targets = targets_with(ParamId::MasterVolume, 0.0);
        s.tick_block(&targets);
        let v0 = s.next_volume();
        let v1 = s.next_volume();
        // Per-sample glide downward, not an instant jump to 0.
        assert!(v0 > 0.0 && v1 < v0, "no per-sample glide: {v0} -> {v1}");
    }

    #[test]
    fn snap_all_settles_volume_and_values() {
        let mut s = ParamSmoother::new(48_000.0, &ParamValues::default());
        let targets = targets_with(ParamId::MasterVolume, 0.3);
        s.snap_all(&targets);
        assert_eq!(s.next_volume(), 0.3);
        assert_eq!(s.values().get(ParamId::MasterVolume), 0.3);
    }
}
