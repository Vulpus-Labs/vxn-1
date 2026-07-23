//! Patch-level modulation sources resolved once per control block.
//!
//! The mod matrix routes from a fixed set of sources to destinations; LFO1 is
//! one such source. This module owns the patch-global LFO1 state and produces
//! a `ModBlock` snapshot the matrix can read.
//!
//! Per-voice sources (LFO2, Pitch EG, Mod Env, `voice_idx`, `voice_spread`,
//! `voice_rand`) are resolved inside each [`vxn2_dsp::stack::Stack`] — they
//! diverge per stack and don't fit in a patch-shared snapshot.
//!
//! LFO1 supports BPM sync. The engine forwards the host's current tempo and
//! the control-block duration; `Lfo1` snaps to the subdivision table when
//! sync is on. On a host transport restart the engine calls
//! [`PatchMod::on_transport_restart`] to realign LFO1's phase to the bar
//! grid.

use vxn2_dsp::lfo::{Lfo1, Lfo1Params};

/// Patch-level modulation engine state. Owned by the top-level engine; one
/// instance per patch.
#[derive(Clone, Copy, Debug)]
pub struct PatchMod {
    pub lfo1: Lfo1,
}

impl Default for PatchMod {
    fn default() -> Self {
        Self {
            lfo1: Lfo1::default(),
        }
    }
}

impl PatchMod {
    pub fn new(seed: u64) -> Self {
        Self {
            lfo1: Lfo1::new(seed),
        }
    }

    /// Realign LFO1 phase to the bar grid (phase 0) on host transport restart
    /// so a synced shape anchors to the beat. Gating on sync is the caller's
    /// job.
    pub fn on_transport_restart(&mut self) {
        self.lfo1.reset_phase();
    }

    /// Resolve the patch-global modulation snapshot for the next control
    /// block. `tempo_bpm` is the host transport tempo (or a sane default);
    /// `block_secs` is the block duration in seconds.
    #[inline]
    pub fn eval_block(
        &mut self,
        params: &PatchModParams,
        tempo_bpm: f32,
        block_secs: f32,
    ) -> ModBlock {
        ModBlock {
            lfo1: self.lfo1.eval(&params.lfo1, tempo_bpm, block_secs),
        }
    }
}

/// Patch-level modulation parameters (driven by the parameter table). Kept
/// separate from `PatchMod` so the host can rebuild parameters cheaply
/// without resetting LFO phase.
#[derive(Clone, Copy, Debug, Default)]
pub struct PatchModParams {
    pub lfo1: Lfo1Params,
}

/// Patch-global modulation snapshot for one control block. The mod matrix
/// reads from this. LFO1 output is bipolar `[-1, +1]` and enters the matrix
/// at full scale; per-route send level is the slot depth column's job.
#[derive(Clone, Copy, Debug, Default)]
pub struct ModBlock {
    pub lfo1: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use vxn2_dsp::lfo::{LfoShape, SUBDIVISIONS};

    const BLK: f32 = 64.0 / 48_000.0;

    #[test]
    fn eval_block_returns_finite_lfo1() {
        let mut pm = PatchMod::default();
        let params = PatchModParams::default();
        for _ in 0..1000 {
            let mb = pm.eval_block(&params, 120.0, BLK);
            assert!(mb.lfo1.is_finite() && mb.lfo1.abs() <= 1.001);
        }
    }

    #[test]
    fn eval_block_sync_overrides_free_rate() {
        let mut pm = PatchMod::default();
        let q_idx = SUBDIVISIONS.iter().position(|s| s.label == "1/4").unwrap();
        let params = PatchModParams {
            lfo1: Lfo1Params {
                shape: LfoShape::Sine,
                rate_hz: 99.0,
                sync: true,
                sync_index: q_idx,
            },
        };
        // 2 Hz at 64-sample blocks @ 48 kHz = 375 blocks per cycle.
        let mut samples = Vec::with_capacity(1500);
        for _ in 0..1500 {
            samples.push(pm.eval_block(&params, 120.0, BLK));
        }
        let mut crossings = vec![];
        let mut prev = samples[0].lfo1;
        for (i, mb) in samples.iter().enumerate().skip(1) {
            if prev < 0.0 && mb.lfo1 >= 0.0 {
                crossings.push(i);
            }
            prev = mb.lfo1;
        }
        let period = (crossings[1] - crossings[0]) as i32;
        assert!((period - 375).abs() <= 3, "synced period {period}");
    }

    #[test]
    fn transport_restart_resets_lfo1_phase() {
        let mut pm = PatchMod::default();
        let params = PatchModParams::default();
        for _ in 0..100 {
            pm.eval_block(&params, 120.0, BLK);
        }
        pm.on_transport_restart();
        assert_eq!(pm.lfo1.phase, 0);
    }
}
