//! Control-rate vocabulary: the block size, the update-rate taxonomy, and the
//! sample-rate newtypes.
//!
//! This is naming, not machinery. Its job is to make two classes of bug
//! unrepresentable rather than merely commented:
//!
//! 1. **"Which sample rate is this?"** — see [`BaseRate`] / [`OsRate`] /
//!    [`CtrlRate`].
//! 2. **"How often does this parameter actually move?"** — see [`UpdateRate`].

/// Samples per control block. Parameters are cooked once per block; the audio
/// path then runs `CONTROL_BLOCK` samples with fixed (or linearly ramped)
/// coefficients.
///
/// 32 at 48 kHz is a 1.5 kHz control rate — fast enough that a swept knob does
/// not zipper, coarse enough that per-block cook cost disappears against the
/// per-sample work.
///
/// **The single definition repo-wide** (ticket 0226). It was previously spelled
/// out in `vxn-dsp`, `vxn2-clap` and `vxn2-wasm`; all three now re-export this.
/// Three copies of a constant that must agree is three chances to disagree, and
/// the failure mode — a control block that is 32 on one side of the wire and 64
/// on the other — is silent.
pub const CONTROL_BLOCK: usize = 32;

/// How often a parameter's value is allowed to change in the audio path.
///
/// Naming for a distinction all three synths already make and each spells
/// differently (vxn-1's `Glide`, vxn-1b's Motion/Fx classification, vxn-2's
/// per-param smoother choice). Consolidating the *vocabulary* is cheap and makes
/// the per-synth tables comparable; consolidating the *policy* is deliberately
/// not done — which params belong in which class is a voicing decision, and ADR
/// 0002 §6 keeps it per-synth.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UpdateRate {
    /// Jumps to its new value immediately. Correct for discrete choices (enums,
    /// bools) and for values smoothed further downstream — vxn-1's cutoff snaps
    /// here because `PolyOtaLadder` interpolates the *coefficient* per sample,
    /// so smoothing the value too would be redundant work.
    Snap,
    /// One glide step per control block. The default for gain-like continuous
    /// params: at a 1.5 kHz control rate it takes the edge off an automation
    /// step without per-sample cost.
    Block,
    /// One glide step every `n` samples, for a kernel that internally batches
    /// at some other granularity.
    Quantum(u32),
    /// Glides every sample. Reserved for values consumed in the per-sample
    /// multiply — vxn-1's master volume is the canonical case.
    PerSample,
}

impl UpdateRate {
    /// Samples between glide steps. `Snap` is 0 (no glide at all).
    #[inline]
    pub const fn stride(self) -> u32 {
        match self {
            UpdateRate::Snap => 0,
            UpdateRate::Block => CONTROL_BLOCK as u32,
            UpdateRate::Quantum(n) => n,
            UpdateRate::PerSample => 1,
        }
    }
}

// ---------------------------------------------------------------------------
// Rate newtypes
// ---------------------------------------------------------------------------
//
// Three different sample rates are live at once in these engines, they are all
// `f32`, and passing the wrong one produces something that *works* and is
// subtly mistuned — the worst possible failure. Two hazards this guards, both
// real and both currently held together by comments:
//
//   * `OtaLadderCoeffs::new` takes the OVERSAMPLED rate for its fs-dependent
//     pole detune, while its `k_cap` is in absolute Hz. Hand it the base rate
//     and the filter detunes wrongly at every oversample setting but 1x.
//   * vxn-1's `LfoCore` is constructed at the CONTROL rate (voice.rs:501), not
//     the base rate. Hand it the base rate and every LFO runs 32x slow.
//
// Deliberately `Copy` and `#[inline]`: these vanish at `-O`, so the guard costs
// nothing in the hot path.

/// The plugin's host sample rate — what `activate()` is handed.
#[derive(Clone, Copy, PartialEq, PartialOrd, Debug)]
pub struct BaseRate(pub f32);

/// The oversampled rate inside an oversampled region: `base * factor`.
#[derive(Clone, Copy, PartialEq, PartialOrd, Debug)]
pub struct OsRate(pub f32);

/// The control rate: `base / CONTROL_BLOCK`. What a per-block-ticked smoother
/// or LFO is constructed at.
#[derive(Clone, Copy, PartialEq, PartialOrd, Debug)]
pub struct CtrlRate(pub f32);

impl BaseRate {
    #[inline]
    pub const fn hz(self) -> f32 {
        self.0
    }

    /// The oversampled rate for an integer `factor` (1, 2, 4, 8).
    #[inline]
    pub fn oversampled(self, factor: u32) -> OsRate {
        OsRate(self.0 * factor as f32)
    }

    /// The control rate: one tick per [`CONTROL_BLOCK`] samples.
    #[inline]
    pub fn control(self) -> CtrlRate {
        CtrlRate(self.0 / CONTROL_BLOCK as f32)
    }
}

impl OsRate {
    #[inline]
    pub const fn hz(self) -> f32 {
        self.0
    }
}

impl CtrlRate {
    #[inline]
    pub const fn hz(self) -> f32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_block_is_32() {
        assert_eq!(CONTROL_BLOCK, 32);
    }

    #[test]
    fn update_rate_strides() {
        assert_eq!(UpdateRate::Snap.stride(), 0);
        assert_eq!(UpdateRate::Block.stride(), 32);
        assert_eq!(UpdateRate::Quantum(8).stride(), 8);
        assert_eq!(UpdateRate::PerSample.stride(), 1);
    }

    /// The conversions, at the two rates that actually ship.
    #[test]
    fn rate_conversions() {
        let base = BaseRate(48_000.0);
        assert_eq!(base.hz(), 48_000.0);
        assert_eq!(base.oversampled(1).hz(), 48_000.0);
        assert_eq!(base.oversampled(4).hz(), 192_000.0);
        assert_eq!(base.control().hz(), 1_500.0);

        let base = BaseRate(44_100.0);
        assert_eq!(base.oversampled(8).hz(), 352_800.0);
        assert_eq!(base.control().hz(), 44_100.0 / 32.0);
    }

    /// The whole point: these are distinct types, so the two documented hazards
    /// become compile errors rather than mistuned audio. This test exists to be
    /// read — if someone collapses them back to bare `f32`, it should look wrong.
    #[test]
    fn the_three_rates_are_distinct_types() {
        let base = BaseRate(48_000.0);
        let os = base.oversampled(4);
        let ctrl = base.control();
        // Same underlying f32 kind, wildly different magnitudes — which is
        // exactly why passing one for another is not caught by any range check.
        assert!(os.hz() > base.hz());
        assert!(ctrl.hz() < base.hz());
        // `os.hz()` and `base.hz()` are both f32 and both "the sample rate";
        // only the wrapper distinguishes them.
        assert_eq!(os.hz() / base.hz(), 4.0);
    }
}
