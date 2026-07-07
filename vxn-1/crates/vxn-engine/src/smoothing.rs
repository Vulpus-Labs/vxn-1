//! Engine-side parameter smoothing for *gain-like* parameters.
//!
//! Raw parameter targets (host automation, UI edits, preset loads) arrive in
//! steps. Feeding them straight into per-sample gains produces zipper noise.
//! This layer sets smoothing *targets* once per control block and glides toward
//! them, matching glide granularity to where each parameter is consumed:
//!
//! - **Per-sample** ([`GlobalParam::MasterVolume`]): the final gain multiply
//!   runs per output sample, so its smoother ticks per sample.
//! - **Block-rate** (oscillator levels, pulse width, fixed-route depths):
//!   read once per control block into [`crate::voice::BlockCtx`], so one glide
//!   step per block (control rate = sr / `CONTROL_BLOCK` ≈ 1.5 kHz) is enough
//!   to take the audible edge off automation steps.
//! - **Snap** (everything else: enums, bools, ADSR times, pitch, LFO/effect
//!   rates, and crucially cutoff/resonance/drive): discrete, cached, or — for
//!   the filter — smoothed downstream by per-sample *coefficient* interpolation
//!   in [`vxn_dsp::PolyOtaLadder`], which handles automation, LFO and envelope
//!   modulation uniformly. Smoothing the cutoff *value* here too would be
//!   redundant, so these jump.
//!
//! Glide classification is per per-patch / global param and applied to **both
//! layers** (ADR 0003): a layer is a complete patch, so each gets the same
//! smoothing treatment.

use crate::params::{GlobalParam, Layer, ParamValues, PatchParam};
use vxn_dsp::{CONTROL_BLOCK, Smoothed, one_pole_coeff};

/// Glide time for block-rate smoothed params (ms).
const BLOCK_SMOOTH_MS: f32 = 10.0;
/// Glide time for the per-sample master volume (ms).
const VOLUME_SMOOTH_MS: f32 = 5.0;
/// Distance below which a block-rate glide snaps to its target instead of
/// crawling down the one-pole's asymptotic tail. Without this the smoothed
/// value never reaches the target exactly, so a `!= 0.0` gate driven off it
/// (ring mod, cross-mod amount) stays armed indefinitely after the param is
/// dialled to zero — the expensive path keeps running and CPU never recovers.
/// 1e-6 is ≈ −120 dB, inaudible for the gain/depth params this governs.
const GLIDE_SNAP_EPS: f32 = 1.0e-6;

/// Equal-gain raised-cosine rise `0.5 − 0.5·cos(π·t)` for `t ∈ [0,1]`. Zero
/// slope at *both* endpoints, so neither the start nor the steady handoff leaves
/// a slope corner (a corner reads as a click — the exact failure this curve
/// fixes). Shared by the FX bypass crossfade ([`BypassXfade`]) and the
/// oversampling-change fade-in (0191); matches the law vxn-2 documents in
/// `render_block_filter_xfade`.
#[inline]
pub(crate) fn raised_cosine_rise(t: f32) -> f32 {
    0.5 - 0.5 * (core::f32::consts::PI * t).cos()
}

/// Fade window length in samples from a millisecond time at `sample_rate`
/// (at least 1, so the ramp always spans a non-degenerate interval).
#[inline]
pub(crate) fn ms_to_samples(ms: f32, sample_rate: f32) -> usize {
    (ms * 0.001 * sample_rate).round().max(1.0) as usize
}

/// A deterministic equal-gain raised-cosine crossfade between a stage's *dry*
/// input and its *wet* output, armed on a bypass-flag edge. Equal-gain (weights
/// sum to 1) because dry and wet are strongly correlated — same rationale as
/// vxn-2's `FILTER_XFADE_MS` toggle. Idle (`remaining == 0`) it costs nothing:
/// the caller takes its zero-cost passthrough instead.
pub(crate) struct BypassXfade {
    /// Fade window in samples (`~FX_XFADE_MS` at the base rate).
    len: usize,
    /// Samples of fade left; `0` ⇒ idle, `> 0` ⇒ fade in flight.
    remaining: usize,
    /// Direction: `true` = dry→wet (engage), `false` = wet→dry (bypass).
    to_wet: bool,
    /// Last-seen flag, for edge detection.
    on: bool,
}

impl BypassXfade {
    pub(crate) fn new(len: usize) -> Self {
        Self {
            len: len.max(1),
            remaining: 0,
            to_wet: false,
            on: false,
        }
    }

    /// Re-idle the fade (transport reset / sample-rate change): drop any
    /// in-flight fade. The edge memory (`on`) is left to the next
    /// [`Self::prime`], so a still-engaged effect doesn't spuriously re-fade.
    pub(crate) fn reset(&mut self) {
        self.remaining = 0;
    }

    /// Adopt `on` as the current flag state with no fade — the first-block seed
    /// after construction or a reset, so an effect that starts engaged is simply
    /// on (no startup ramp) and only a genuine user edge arms a fade.
    pub(crate) fn prime(&mut self, on: bool) {
        self.on = on;
        self.remaining = 0;
    }

    /// Arm a fade on a flag edge. No-op if the flag is unchanged. Returns `true`
    /// only on the **off→on** edge, so the caller can reset that stage's DSP
    /// state before the wet fades in from a clean tail.
    pub(crate) fn arm(&mut self, now_on: bool) -> bool {
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
    pub(crate) fn active(&self) -> bool {
        self.remaining > 0
    }

    /// `(w_dry, w_wet)` for sample `i` within the current block (whose start had
    /// `remaining` samples left). `t` spans `[0,1]` across the window and clamps
    /// past its end, so the last fade sample lands exactly on the target.
    #[inline]
    pub(crate) fn weights_at(&self, i: usize) -> (f32, f32) {
        let span = (self.len as f32 - 1.0).max(1.0);
        let start = (self.len - self.remaining) as f32;
        let t = ((start + i as f32) / span).min(1.0);
        let rise = raised_cosine_rise(t);
        if self.to_wet {
            (1.0 - rise, rise)
        } else {
            (rise, 1.0 - rise)
        }
    }

    /// Consume a processed block of `n` samples.
    #[inline]
    pub(crate) fn advance(&mut self, n: usize) {
        self.remaining = self.remaining.saturating_sub(n);
    }
}

/// One block-rate glide step: a one-pole move toward `tgt`, snapping to it once
/// within [`GLIDE_SNAP_EPS`] so the value settles exactly (see the constant).
#[inline]
fn glide_step(cur: f32, tgt: f32, coeff: f32) -> f32 {
    let next = cur + coeff * (tgt - cur);
    if (tgt - next).abs() < GLIDE_SNAP_EPS {
        tgt
    } else {
        next
    }
}

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

/// Block-rate vs snap classification for a per-patch param. Gain-like
/// continuous values (levels, pulse widths, every fixed-route depth, the
/// cross-mod amount) glide at block rate; selectors/bools/enums and downstream-
/// smoothed params (cutoff/reso/drive) snap.
///
/// `LayerLevel` and `Spread` (0015) are mixer-class continuous values read into
/// [`crate::voice::BlockCtx`] each block: LayerLevel is a per-layer gain that
/// zippers on automation; Spread recomputes per-voice pan for all 8 slots, so a
/// step jumps the stereo image on every voice at once. Both glide at block rate
/// so the gain / pan coefficients move across the glide window instead of
/// snapping. A Spread glide to 0.0 still settles exactly (see `glide_step`), so
/// the mono fast path (`spread == 0.0`) re-arms once the glide lands.
#[inline]
fn patch_glide(p: PatchParam) -> Glide {
    use PatchParam::*;
    match p {
        Osc1Level | Osc2Level | SubLevel | NoiseLevel | Osc1PulseWidth | Osc2PulseWidth
        | CrossModAmount | PitchLfoDepth | PitchEnvDepth | PitchWheelDepth | PwmLfoDepth
        | PwmEnvDepth | CutoffLfo1Depth | CutoffLfo2Depth | CutoffEnvDepth | VelCutoffDepth
        | ModWheelPwm | ModWheelCutoff | ModWheelReso | ModWheelCrossModSweep
        | AmpLfoDepth | LayerLevel | Spread => Glide::Block,
        _ => Glide::Snap,
    }
}

/// Classification for a global param. Master volume is glided per-sample by the
/// dedicated [`Smoothed`]; reverb/phaser/chorus/delay knobs glide at block rate
/// because the engine reads them straight into each effect's per-block
/// `set_params` (or, for the FDN, its per-sample blend / coefficients); the rest
/// snap.
///
/// Chorus/delay smoothing (0015): the FX bus reads these into `MasterFx::update`
/// each control block, and the DSP `set_params` snaps its targets, so unsmoothed
/// automation steps audibly —
///
/// - **ChorusDepth / ChorusMix, DelayFeedback / DelayMix** zipper as gain-like
///   blends → block glide here, exactly like the reverb/phaser knobs.
/// - **DelayTime** deliberately **snaps** here: a block-rate glide of the *value*
///   would still step the read pointer at each block boundary (a buzz on fast
///   sweeps). Its ramp instead lives one level down, per sample, inside
///   [`vxn_dsp::StereoDelay`] (slewed read distance through the line's
///   fractional `read`) — the same arrangement as cutoff/reso, which snap here
///   because the ladder ramps their coefficients. Block-gliding it too would
///   double-smooth and fight the DSP slew.
/// - **ChorusRate** deliberately **snaps**: `StereoChorus::set_params` only
///   updates the LFO increment, the phase is continuous across a rate change, so
///   there is no sample discontinuity to smooth (same reasoning as the per-patch
///   `LfoRate`, which also snaps). PhaserRate glides only because it shares the
///   reverb/phaser block above; chorus rate has no such coupling.
#[inline]
fn global_glide(g: GlobalParam) -> Glide {
    match g {
        GlobalParam::MasterVolume => Glide::PerSample,
        GlobalParam::ReverbMix
        | GlobalParam::ReverbSize
        | GlobalParam::ReverbDecay
        | GlobalParam::ReverbDamp
        | GlobalParam::PhaserRate
        | GlobalParam::PhaserDepth
        | GlobalParam::PhaserFB
        | GlobalParam::PhaserMix
        | GlobalParam::ChorusDepth
        | GlobalParam::ChorusMix
        | GlobalParam::DelayFeedback
        | GlobalParam::DelayMix => Glide::Block,
        // ChorusRate + DelayTime snap — see the doc comment (LFO phase is
        // continuous; the delay-time ramp lives in StereoDelay per sample).
        _ => Glide::Snap,
    }
}

/// Smooths gain-like parameter values between the raw target store and the
/// engine's per-block read. Cutoff/resonance/drive are deliberately *not*
/// handled here — the ladder interpolates their coefficients per sample.
pub struct ParamSmoother {
    /// Smoothed current values for every param (both layers + global).
    /// Block-rate params glide here; snap params mirror their target each block;
    /// the per-sample volume value is taken from [`Self::next_volume`] instead.
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
                targets.global().get(GlobalParam::MasterVolume),
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
        self.volume
            .snap(targets.global().get(GlobalParam::MasterVolume));
    }

    /// Advance block-rate smoothers one step toward `targets`, snap the rest,
    /// and arm the per-sample volume target. Call once per control block.
    pub fn tick_block(&mut self, targets: &ParamValues) {
        let coeff = self.block_coeff;
        for layer in Layer::ALL {
            let cur = self.current.layer_mut(layer);
            let tgt = targets.layer(layer);
            for p in PatchParam::all() {
                match patch_glide(p) {
                    Glide::Block => {
                        cur.set(p, glide_step(cur.get(p), tgt.get(p), coeff));
                    }
                    // Snap is the only other patch outcome.
                    _ => cur.set(p, tgt.get(p)),
                }
            }
        }
        let cur = self.current.global_mut();
        let tgt = targets.global();
        for g in GlobalParam::all() {
            match global_glide(g) {
                Glide::PerSample => {
                    self.volume.set_target(tgt.get(g));
                    cur.set(g, tgt.get(g));
                }
                Glide::Block => {
                    cur.set(g, glide_step(cur.get(g), tgt.get(g), coeff));
                }
                Glide::Snap => cur.set(g, tgt.get(g)),
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

    fn patch_target(p: PatchParam, v: f32) -> ParamValues {
        let mut pv = ParamValues::default();
        pv.layer_mut(Layer::Upper).set(p, v);
        pv
    }

    #[test]
    fn block_param_glides_toward_target() {
        let mut s = ParamSmoother::new(48_000.0, &ParamValues::default());
        let start = ParamValues::default()
            .layer(Layer::Upper)
            .get(PatchParam::Osc1Level);
        let targets = patch_target(PatchParam::Osc1Level, 0.0);
        s.tick_block(&targets);
        let after_one = s.values().layer(Layer::Upper).get(PatchParam::Osc1Level);
        assert!(
            after_one < start && after_one > 0.0,
            "no glide: {after_one}"
        );
        for _ in 0..2000 {
            s.tick_block(&targets);
        }
        assert!(
            s.values()
                .layer(Layer::Upper)
                .get(PatchParam::Osc1Level)
                .abs()
                < 1e-3
        );
    }

    #[test]
    fn block_param_settles_exactly_to_zero() {
        // A block-rate glide must reach its target *exactly* in bounded time,
        // not crawl down the one-pole tail forever. CrossModAmount / NoiseLevel
        // gate expensive paths off `!= 0.0`, so a residual epsilon would keep
        // them armed and CPU pinned after the param is zeroed.
        let start = patch_target(PatchParam::NoiseLevel, 1.0);
        let mut s = ParamSmoother::new(48_000.0, &start);
        let zero = ParamValues::default(); // NoiseLevel target 0.0
        let mut blocks = 0;
        loop {
            s.tick_block(&zero);
            blocks += 1;
            if s.values().layer(Layer::Upper).get(PatchParam::NoiseLevel) == 0.0 {
                break;
            }
            assert!(blocks < 1000, "NoiseLevel never reached exactly 0.0");
        }
        // 10 ms time constant at the control rate settles well under 1000 blocks.
        assert!(blocks < 1000);
    }

    #[test]
    fn snap_params_jump_immediately() {
        // Cutoff is snapped here (ladder interpolates its coeffs downstream).
        let mut s = ParamSmoother::new(48_000.0, &ParamValues::default());
        let targets = patch_target(PatchParam::Cutoff, 100.0);
        s.tick_block(&targets);
        assert_eq!(
            s.values().layer(Layer::Upper).get(PatchParam::Cutoff),
            100.0
        );
    }

    #[test]
    fn route_depths_are_block_smoothed() {
        assert_eq!(patch_glide(PatchParam::CutoffLfo1Depth), Glide::Block);
        assert_eq!(patch_glide(PatchParam::CutoffLfo2Depth), Glide::Block);
        assert_eq!(patch_glide(PatchParam::PitchEnvDepth), Glide::Block);
        assert_eq!(patch_glide(PatchParam::NoiseLevel), Glide::Block);
        // Selectors snap (discrete).
        assert_eq!(patch_glide(PatchParam::PitchLfoSrc), Glide::Snap);
    }

    #[test]
    fn mixer_params_are_block_smoothed() {
        // 0015: LayerLevel (per-layer gain) and Spread (per-voice pan) glide so
        // automation does not zipper the gain or jump the stereo image.
        assert_eq!(patch_glide(PatchParam::LayerLevel), Glide::Block);
        assert_eq!(patch_glide(PatchParam::Spread), Glide::Block);
    }

    #[test]
    fn fx_params_smoothing_policy() {
        // 0015: chorus/delay knobs that zipper or click glide at block rate;
        // ChorusRate snaps (LFO phase is continuous across rate changes — see
        // `global_glide`'s doc comment). This test pins both halves of that
        // decision so a future edit can't silently drop a glide or start
        // smoothing the rate.
        for g in [
            GlobalParam::ChorusDepth,
            GlobalParam::ChorusMix,
            GlobalParam::DelayFeedback,
            GlobalParam::DelayMix,
        ] {
            assert_eq!(global_glide(g), Glide::Block, "{g:?} should block-glide");
        }
        assert_eq!(
            global_glide(GlobalParam::ChorusRate),
            Glide::Snap,
            "ChorusRate snaps by design"
        );
        // DelayTime snaps here — its ramp is per-sample inside StereoDelay.
        assert_eq!(
            global_glide(GlobalParam::DelayTime),
            Glide::Snap,
            "DelayTime snaps in the smoother; StereoDelay slews it"
        );
        // On/off switches are discrete.
        assert_eq!(global_glide(GlobalParam::ChorusOn), Glide::Snap);
        assert_eq!(global_glide(GlobalParam::DelayOn), Glide::Snap);
    }

    #[test]
    fn both_layers_smooth_independently() {
        let mut s = ParamSmoother::new(48_000.0, &ParamValues::default());
        let mut targets = ParamValues::default();
        targets
            .layer_mut(Layer::Upper)
            .set(PatchParam::Cutoff, 100.0);
        targets
            .layer_mut(Layer::Lower)
            .set(PatchParam::Cutoff, 200.0);
        s.tick_block(&targets);
        assert_eq!(
            s.values().layer(Layer::Upper).get(PatchParam::Cutoff),
            100.0
        );
        assert_eq!(
            s.values().layer(Layer::Lower).get(PatchParam::Cutoff),
            200.0
        );
    }

    #[test]
    fn volume_glides_per_sample() {
        let mut s = ParamSmoother::new(48_000.0, &ParamValues::default());
        let mut targets = ParamValues::default();
        targets.global_mut().set(GlobalParam::MasterVolume, 0.0);
        s.tick_block(&targets);
        let v0 = s.next_volume();
        let v1 = s.next_volume();
        assert!(v0 > 0.0 && v1 < v0, "no per-sample glide: {v0} -> {v1}");
    }

    #[test]
    fn snap_all_settles_volume_and_values() {
        let mut s = ParamSmoother::new(48_000.0, &ParamValues::default());
        let mut targets = ParamValues::default();
        targets.global_mut().set(GlobalParam::MasterVolume, 0.3);
        s.snap_all(&targets);
        assert_eq!(s.next_volume(), 0.3);
        assert_eq!(s.values().global().get(GlobalParam::MasterVolume), 0.3);
    }
}
