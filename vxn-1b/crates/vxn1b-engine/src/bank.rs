//! Matrix-driven 8-wide render bank (ticket 0202, steps 1–2).
//!
//! A fork of VXN1's `VoiceBank` render path
//! ([`vxn-1/crates/vxn-engine/src/voice.rs`]) with the fixed per-channel routing
//! deleted: instead of `resolve_mod` → `ModOut`, each lane evaluates the matrix
//! ([`crate::eval`]) into a [`DestVals`] and the [`crate::render`] apply layer
//! maps it onto the same DSP consumption points. Everything downstream of that
//! — the osc→hpf→ladder→VCA→pan render loop and its fast-path branches — is
//! VXN1's, so the sound is identical when the matrix reproduces VXN1's routes.
//!
//! **Width.** `vxn-dsp`'s poly kernels are 8-wide (`CHANNELS_PER_LAYER`), reused
//! verbatim (ADR §1). A bank is one 8-lane SoA group; the engine runs **two**
//! for 16-voice poly (step 3). The per-voice bookkeeping (note/gate/active/…)
//! lives in the 16-wide [`crate::voice::Voices`] coordinator and is threaded in
//! per render call; this bank owns only DSP + trigger state.
//!
//! **Per-frame Amp (step 2).** Every non-Amp dest is resolved once per control
//! block from block-start source values (VXN1's `resolve_mod` granularity). The
//! **Amp** dest is special: the VCA must follow the amp envelope *per base
//! frame* to stay click-free (ADR §3). Since the Amp accumulation is linear in
//! its sources, the block-start pass factors it into
//! `amp = static + e1·env1 + e2·env2` (see [`AmpCoeffs`]); per frame the fresh
//! envelope levels are substituted — 2 FMAs, not a full matrix re-eval (which
//! the ticket forbids on the hot path).
//!
//! **Scope (this step).** Poly only (matches the 0198 allocator); unison/mono
//! is deferred. `CrossModAmount` is live per lane (0242): the PM kernel takes a
//! per-lane index whenever a route is active and the broadcast scalar otherwise,
//! so an unrouted patch is bit-unchanged. `HpfCutoff` stays deferred — the HPF
//! is set bank-wide. Both are inert at the factory default, so the parity gate
//! is unaffected. The
//! per-voice component trims landed with global drift (0218) and are likewise
//! inert at the default `MasterDrift = 0`.

use vxn_dsp::{
    AdsrCore, AdsrShape, AdsrStage, CHANNELS_PER_LAYER, CONTROL_BLOCK, FilterMode, FilterSlope,
    LfoCore, LfoShape, NoiseColor, OtaLadderCoeffs, PolyHpf, PolyNoiseBank, PolyOscillator,
    PolyOtaLadder, Waveform, note_to_hz, poly_ring_mod, poly_sub_square,
};

use crate::eval::{SourceInputs, eval_dests, eval_sources};
use crate::matrix::{DestId, MatrixTable, SourceId};
use crate::mod_smoothing::{MotionSmoother, PITCH_QUANTUM};
use crate::params::CrossModType;
use crate::render;

/// Lanes per bank — the shared DSP kernel width.
const N: usize = CHANNELS_PER_LAYER;

/// HPF cutoff at or below this (Hz) is bypassed (matches VXN1).
const HPF_OFF_HZ: f32 = 20.0;

/// Fixed ring-mod diode drive (dB), as VXN1.
const RING_DRIVE_DB: f32 = 1.0;

/// Independent drift streams for osc1/osc2 (as VXN1) so the two oscillators in a
/// voice wander independently.
const OSC1_DRIFT_SALT: u64 = 0xA1F7_0501;
const OSC2_DRIFT_SALT: u64 = 0xB2E8_0502;

/// Fixed per-voice component-tolerance trims (0218), ported unchanged from
/// VXN1's `VoiceTrim` (E022 / 0124). Unlike the drift *walk* on osc pitch these
/// are constant per-lane offsets — frozen at construction like a real synth's
/// power-on calibration spread — on envelope times, sustain, base cutoff and
/// resonance. Each is a normalised `[-1, 1]` draw ([`trim_draw`]) scaled by
/// `drift_amount` (the one global "analog" amount) and the per-target magnitude
/// below, so `drift_amount = 0` collapses every lane back to bit-identical
/// shared params (the parity-gate contract).
///
/// Magnitudes are the *max* fractional deviation at `drift_amount = 1.0`:
/// envelope A/D/R ±12%, sustain ±3%, resonance ±7% (component tolerance), and
/// base cutoff a deliberately tiny ±3 cents — enough for gentle beating between
/// voices, small enough that self-oscillating "whistle" tones never read as out
/// of tune.
const TRIM_ENV_TIME: f32 = 0.12;
const TRIM_SUSTAIN: f32 = 0.03;
const TRIM_RESO: f32 = 0.07;
const TRIM_CUTOFF_CENTS: f32 = 3.0;

/// Salts selecting the four independent trim streams from the bank seed.
const TRIM_ENV_SALT: u64 = 0xC3D9_0601;
const TRIM_SUS_SALT: u64 = 0xD4EA_0602;
const TRIM_CUT_SALT: u64 = 0xE5FB_0603;
const TRIM_RESO_SALT: u64 = 0xF60C_0604;

/// One deterministic per-lane trim draw in `[-1, 1]`. A SplitMix64 finaliser
/// over `base ⊕ salt ⊕ lane` — no state, no walk, reproducible. Distinct salts
/// decorrelate the four targets (and the pitch drift) so a voice's bright filter
/// doesn't imply a long decay.
#[inline]
fn trim_draw(base: u64, salt: u64, lane: usize) -> f32 {
    let mut z = base
        .wrapping_add(salt)
        .wrapping_add((lane as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // Top 24 bits → [0,1), then map to [-1, 1].
    let unit = (z >> 40) as f32 / (1u64 << 24) as f32;
    unit * 2.0 - 1.0
}

/// Fixed per-lane trim table: one normalised `[-1, 1]` draw per lane per target,
/// generated once from the bank seed and never reset on note-on — a property of
/// the lane, mirroring the drift seed. Scaled at apply time by `drift_amount`
/// and the per-target magnitude. The two banks of a synth take distinct seeds,
/// so all 16 voices draw independently.
#[derive(Clone, Copy)]
struct VoiceTrim {
    /// Multiplies envelope attack/decay/release per lane.
    env_time: [f32; N],
    /// Multiplies envelope sustain level per lane.
    sustain: [f32; N],
    /// Base-cutoff offset per lane, in normalised units (× [`TRIM_CUTOFF_CENTS`]).
    cutoff: [f32; N],
    /// Multiplies resonance per lane.
    reso: [f32; N],
}

impl VoiceTrim {
    fn new(base: u64) -> Self {
        Self {
            env_time: std::array::from_fn(|i| trim_draw(base, TRIM_ENV_SALT, i)),
            sustain: std::array::from_fn(|i| trim_draw(base, TRIM_SUS_SALT, i)),
            cutoff: std::array::from_fn(|i| trim_draw(base, TRIM_CUT_SALT, i)),
            reso: std::array::from_fn(|i| trim_draw(base, TRIM_RESO_SALT, i)),
        }
    }
}

/// Golden-ratio per-lane start phase (decorrelates a chord's transients).
#[inline]
fn lane_phase(lane: usize) -> f32 {
    // Must stay bit-identical to VXN1's `channel_phase` GOLDEN so the default
    // patch's per-voice osc start phases match for the render-parity gate.
    #[allow(clippy::excessive_precision)]
    const GOLDEN: f32 = 0.6180339887;
    ((lane as f32 + 1.0) * GOLDEN).fract()
}

/// Per-voice LFO 1 two-stage onset (delay → fade), forked from VXN1.
#[derive(Clone)]
struct Lfo1Onset {
    t: [f32; N],
}

impl Lfo1Onset {
    fn new() -> Self {
        Self { t: [f32::MAX; N] }
    }
    fn reset(&mut self) {
        self.t = [f32::MAX; N];
    }
    #[inline]
    fn retrigger(&mut self, v: usize) {
        self.t[v] = 0.0;
    }
    #[inline]
    fn gain(&self, v: usize, delay: f32, fade: f32) -> f32 {
        let t = self.t[v];
        if t < delay {
            0.0
        } else if fade <= 0.0 {
            1.0
        } else {
            ((t - delay) / fade).min(1.0)
        }
    }
    #[inline]
    fn advance(&mut self, dt: f32, cap: f32) {
        for t in &mut self.t {
            if *t < cap {
                *t = (*t + dt).min(cap);
            }
        }
    }
}

/// Block context shared by both banks — **mod-agnostic**: it carries the raw
/// synthesis params + the matrix table + the patch-global source scalars, not
/// VXN1's resolved route structs. The engine (step 3) builds it from
/// [`crate::params::Params`] each block.
pub struct BlockCtx<'a> {
    pub os_sample_rate: f32,
    pub os: usize,
    // Osc / mixer
    pub osc1_wave: Waveform,
    pub osc2_wave: Waveform,
    pub osc1_level: f32,
    pub osc2_level: f32,
    pub sub_level: f32,
    pub noise_level: f32,
    pub noise_color: NoiseColor,
    pub osc1_pw: f32,
    pub osc2_pw: f32,
    pub osc1_semi: f32,
    pub osc2_semi: f32,
    // Cross-mod
    pub sync: bool,
    pub pm_index: f32,
    pub ring_mode: bool,
    pub cross_mod_type: CrossModType,
    // Filter
    pub cutoff: f32,
    /// Key-track amount (0245): `1.0` = 1 oct of cutoff per oct of key, pivoting
    /// at C0. Drives both the static note term and the drift coupling in
    /// [`render::voice_cutoff_hz`].
    pub filter_key_track: f32,
    pub hpf_cutoff: f32,
    pub resonance: f32,
    pub drive: f32,
    pub filter_mode: FilterMode,
    pub filter_slope: FilterSlope,
    // Global / sources
    pub base_semis: f32,
    pub lfo1_shape: LfoShape,
    pub lfo1_rate_hz: f32,
    pub lfo1_delay_time: f32,
    pub lfo1_fade: f32,
    pub lfo2_val: f32,
    pub portamento_time: f32,
    pub amp_env_bypass: bool,
    pub drift_amount: f32,
    pub spread: f32,
    /// The routing topology + depths.
    pub matrix: &'a MatrixTable,
    /// Patch-global source scalars for the matrix (per-voice sources are read
    /// from the bank/voice state).
    pub mod_wheel: f32,
    pub pitch_wheel: f32,
}

/// Per-lane linear factoring of the Amp dest for the per-frame VCA (step 2):
/// `amp = static + e1·env1 + e2·env2`. `e1`/`e2` collect the depth·gain·scale of
/// the `Lin`-curve Env→Amp slots; `static` is the Amp contribution of every
/// other slot at block-start values (non-linear Env→Amp curves fold into
/// `static` at their block-start level — an accepted approximation outside the
/// default/common Lin case).
#[derive(Clone, Copy, Default)]
struct AmpCoeffs {
    stat: f32,
    e1: f32,
    e2: f32,
}

/// The 8-wide DSP + trigger state. Bookkeeping (note/gate/active/velocity/
/// pressure/note_random) is threaded in per [`Self::render`] call.
pub struct RenderBank {
    osc1: PolyOscillator,
    osc2: PolyOscillator,
    noise: PolyNoiseBank,
    hpf: PolyHpf,
    ladder: PolyOtaLadder,
    env1: [AdsrCore; N],
    env2: [AdsrCore; N],
    lfo1: [LfoCore; N],
    lfo1_onset: Lfo1Onset,
    /// Per-lane glided pitch (MIDI note as f32) + whether it has a from-pitch.
    glide_semi: [f32; N],
    glide_valid: [bool; N],
    /// Set at note-on trigger, consumed (taken) at the next render.
    trigger_pending: [bool; N],
    /// Per-lane discontinuity guards on the pitch/PWM/Amp matrix dests (0208).
    smooth: MotionSmoother,
    lfo1_seed: u64,
    /// Fixed per-lane component-tolerance trims (0218). Frozen at construction,
    /// scaled at apply time by the global drift amount.
    trim: VoiceTrim,
}

impl RenderBank {
    /// Lanes per bank (= the shared DSP kernel width).
    pub const LANES: usize = N;

    pub fn new(sample_rate: f32, rng_seed: u64) -> Self {
        let control_rate = sample_rate / CONTROL_BLOCK as f32;
        let mut osc1 = PolyOscillator::new();
        let mut osc2 = PolyOscillator::new();
        osc1.set_drift_seed(rng_seed.wrapping_add(OSC1_DRIFT_SALT) as u32);
        osc2.set_drift_seed(rng_seed.wrapping_add(OSC2_DRIFT_SALT) as u32);
        Self {
            osc1,
            osc2,
            noise: PolyNoiseBank::new(rng_seed),
            hpf: PolyHpf::new(),
            ladder: PolyOtaLadder::new(),
            env1: std::array::from_fn(|_| AdsrCore::new(sample_rate)),
            env2: std::array::from_fn(|_| AdsrCore::new(sample_rate)),
            lfo1: std::array::from_fn(|i| LfoCore::new(control_rate, lfo1_seed(rng_seed, i))),
            lfo1_onset: Lfo1Onset::new(),
            glide_semi: [0.0; N],
            glide_valid: [false; N],
            trigger_pending: [false; N],
            smooth: MotionSmoother::new(sample_rate),
            lfo1_seed: rng_seed,
            trim: VoiceTrim::new(rng_seed),
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.env1 = std::array::from_fn(|_| AdsrCore::new(sample_rate));
        self.env2 = std::array::from_fn(|_| AdsrCore::new(sample_rate));
        let control_rate = sample_rate / CONTROL_BLOCK as f32;
        let seed = self.lfo1_seed;
        self.lfo1 = std::array::from_fn(|i| LfoCore::new(control_rate, lfo1_seed(seed, i)));
        self.smooth.set_sample_rate(sample_rate);
        self.reset();
    }

    pub fn reset(&mut self) {
        self.osc1 = PolyOscillator::new();
        self.osc2 = PolyOscillator::new();
        self.osc1
            .set_drift_seed(self.lfo1_seed.wrapping_add(OSC1_DRIFT_SALT) as u32);
        self.osc2
            .set_drift_seed(self.lfo1_seed.wrapping_add(OSC2_DRIFT_SALT) as u32);
        self.noise.reset();
        self.hpf.reset();
        self.ladder.reset();
        for e in &mut self.env1 {
            e.reset();
        }
        for e in &mut self.env2 {
            e.reset();
        }
        for l in &mut self.lfo1 {
            l.reset();
        }
        self.lfo1_onset.reset();
        self.glide_semi = [0.0; N];
        self.glide_valid = [false; N];
        self.trigger_pending = [false; N];
        self.smooth.reset();
    }

    /// Apply ADSR params to all lanes (called when an envelope param *or*
    /// `drift_amount` changed).
    ///
    /// `drift_amount` scales the fixed per-lane trims (0218): each lane's A/D/R
    /// times and sustain get a constant multiplicative nudge from [`VoiceTrim`],
    /// so a held chord's voices breathe at subtly different rates like real
    /// per-voice analog tolerance. At `drift_amount = 0` every factor is exactly
    /// `1.0`, so all lanes receive bit-identical params.
    pub fn set_envelopes(
        &mut self,
        env1: (f32, f32, f32, f32),
        env1_shape: AdsrShape,
        env2: (f32, f32, f32, f32),
        env2_shape: AdsrShape,
        drift_amount: f32,
    ) {
        let time_mag = TRIM_ENV_TIME * drift_amount;
        let sus_mag = TRIM_SUSTAIN * drift_amount;
        for (v, e) in self.env1.iter_mut().enumerate() {
            let t = 1.0 + self.trim.env_time[v] * time_mag;
            let s = (env1.2 * (1.0 + self.trim.sustain[v] * sus_mag)).clamp(0.0, 1.0);
            e.set_params(env1.0 * t, env1.1 * t, s, env1.3 * t);
            e.set_shape(env1_shape);
        }
        for (v, e) in self.env2.iter_mut().enumerate() {
            let t = 1.0 + self.trim.env_time[v] * time_mag;
            let s = (env2.2 * (1.0 + self.trim.sustain[v] * sus_mag)).clamp(0.0, 1.0);
            e.set_params(env2.0 * t, env2.1 * t, s, env2.3 * t);
            e.set_shape(env2_shape);
        }
    }

    /// DSP trigger for lane `v` at note-on: reset oscillators to a decorrelated
    /// start phase, restart the LFO 1 onset (and phase unless free-running), and
    /// mark a pending trigger for the next render's envelope re-arm.
    pub fn trigger_lane(&mut self, v: usize, lfo1_shape: LfoShape, lfo1_free_run: bool) {
        self.trigger_pending[v] = true;
        self.lfo1_onset.retrigger(v);
        if !lfo1_free_run {
            self.lfo1[v].retrigger(lfo1_shape);
        }
        self.osc1.reset(v);
        self.osc2.reset(v);
        let ph = lane_phase(v);
        self.osc1.phase[v] = ph;
        self.osc2.phase[v] = ph;
    }

    /// True when no lane is active and none is pending — the caller can skip
    /// this bank's render entirely.
    pub fn is_silent(&self, active: &[bool]) -> bool {
        !active.iter().any(|&a| a) && !self.trigger_pending.iter().any(|&t| t)
    }

    /// Envelope block-skip predicate (VXN1): no trigger this block and every
    /// active lane holds both envelopes in Sustain, so env levels are constant.
    #[inline]
    fn envelopes_static(&self, trig: &[bool; N], active: &[bool], gate: &[bool]) -> bool {
        trig.iter().all(|&t| !t)
            && (0..N).all(|v| {
                !active[v]
                    || (gate[v]
                        && self.env1[v].stage == AdsrStage::Sustain
                        && self.env2[v].stage == AdsrStage::Sustain)
            })
    }

    /// Render one control block for this bank into the oversampled stereo
    /// buffers (length = `base_frames · ctx.os`), accumulating. The per-lane
    /// bookkeeping slices (length `N`) are owned by the [`crate::voice::Voices`]
    /// coordinator; `active` is `&mut` so fully-released voices free.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        ctx: &BlockCtx,
        note: &[u8],
        gate: &[bool],
        active: &mut [bool],
        velocity: &[f32],
        pressure: &[f32],
        note_random: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        let os = ctx.os;
        let base_frames = out_l.len() / os;
        let base_rate = ctx.os_sample_rate / os as f32;

        // Per-voice LFO 1: tick each lane's phase once for this block. LFOs tick
        // even on silent blocks so free-run phase keeps drifting.
        let mut lfo1_raw = [0.0f32; N];
        for (lfo, raw) in self.lfo1.iter_mut().zip(lfo1_raw.iter_mut()) {
            lfo.set_rate(ctx.lfo1_rate_hz);
            *raw = lfo.next(ctx.lfo1_shape);
        }
        self.osc1.tick_drift(ctx.drift_amount);
        self.osc2.tick_drift(ctx.drift_amount);
        let onset_cap = ctx.lfo1_delay_time + ctx.lfo1_fade;
        let onset_dt = 1.0 / base_rate;

        // Silent fast path — advance the onset/LFO only.
        if self.is_silent(active) {
            self.lfo1_onset
                .advance(onset_dt * base_frames as f32, onset_cap);
            return;
        }

        let (glide, glide_coeff) = block_glide(ctx.portamento_time, base_frames, base_rate);

        // ── Block-start per-lane resolution ──
        // Pitch-family dests (Pitch, XModSweep) are smoothed per-quantum inside
        // the frame loop; PWM and the non-env Amp part get a block-rate one-pole
        // here (0208). We stash the un-modulated pitch base + the smoother
        // targets per lane so the frame loop can re-cook `inc` as the cascade
        // glides. `g1`/`g2` gate XModSweep onto the mode-selected osc, exactly as
        // `render::voice_pitches` does.
        let (g1, g2) = sweep_gates(ctx.cross_mod_type);
        let mut pw1 = [0.5f32; N];
        let mut pw2 = [0.5f32; N];
        let mut amp_c = [AmpCoeffs::default(); N];
        let mut amp_stat_tgt = [0.0f32; N];
        let mut base1 = [0.0f32; N];
        let mut base2 = [0.0f32; N];
        let mut pitch_tgt = [0.0f32; N];
        let mut sweep_tgt = [0.0f32; N];
        let mut pwm_tgt = [0.0f32; N];
        let mut xmod_tgt = [0.0f32; N];
        let mut pm_idx = [0.0f32; N];
        let mut pitch_active = [false; N];
        let mut pwm_active = [false; N];
        let mut xmod_active = [false; N];
        let mut pan_tgt = [0.0f32; N];
        let mut pan_active = [false; N];
        // `CrossModAmount` only means anything in PM mode — Off/Sync/Ring ignore
        // the amount, as VXN1 does — so the dest is gated on the mode rather
        // than silently accumulating smoother state the kernel can't read.
        let pm_mode = matches!(ctx.cross_mod_type, CrossModType::Pm);
        for v in 0..N {
            let lfo1 = lfo1_raw[v] * self.lfo1_onset.gain(v, ctx.lfo1_delay_time, ctx.lfo1_fade);
            // Matrix sources use env levels at block start (VXN1 granularity).
            let inp = SourceInputs {
                env1: self.env1[v].level,
                env2: self.env2[v].level,
                lfo1,
                lfo2: ctx.lfo2_val,
                velocity: velocity[v],
                note: note[v],
                mod_wheel: ctx.mod_wheel,
                pitch_wheel: ctx.pitch_wheel,
                aftertouch: pressure[v],
                note_random: note_random[v],
                // The lane's own place in the image, already scaled by the
                // Spread knob (0260). Routing this to `Pan` at depth 1 — what
                // the default patch does — is VXN1's hard-wired unison spread,
                // now expressed as topology.
                spread_pos: pan_position(v) * ctx.spread,
            };
            let sources = eval_sources(&inp);
            let mut dests = [0.0f32; crate::matrix::N_DESTS];
            eval_dests(ctx.matrix, &sources, &mut dests);

            // Portamento glide toward the target note (per-voice).
            let target = note[v] as f32;
            if self.trigger_pending[v] {
                if !glide || !self.glide_valid[v] {
                    self.glide_semi[v] = target;
                }
                self.glide_valid[v] = true;
            }
            self.glide_semi[v] += glide_coeff * (target - self.glide_semi[v]);
            let nf = self.glide_semi[v];

            // Un-modulated pitch base per osc (everything except the smoothed
            // Pitch/XModSweep matrix dests). Constant across the control block —
            // drift/glide are already block-rate.
            base1[v] = ctx.base_semis + nf + ctx.osc1_semi + self.osc1.drift_value[v];
            base2[v] = ctx.base_semis + nf + ctx.osc2_semi + self.osc2.drift_value[v];
            pitch_tgt[v] = dests[DestId::Pitch.idx().unwrap()];
            sweep_tgt[v] = dests[DestId::XModSweep.idx().unwrap()];
            pwm_tgt[v] = dests[DestId::Pwm.idx().unwrap()];
            // Pan (0260). Clamped here rather than at the gains: the smoother
            // should chase a reachable position, or an over-deep route would
            // leave it creeping toward a target the law can never render.
            pan_tgt[v] = dests[DestId::Pan.idx().unwrap()].clamp(-1.0, 1.0);
            xmod_tgt[v] = if pm_mode {
                dests[DestId::CrossModAmount.idx().unwrap()]
            } else {
                0.0
            };

            // Non-env Amp coefficient: `e1`/`e2` stay per-frame exact; `stat` (the
            // non-envelope routes) is the target the per-frame Amp one-pole glides
            // toward in the render loop.
            let ac = amp_coeffs(ctx.matrix, &sources);
            amp_stat_tgt[v] = ac.stat;

            // A fresh note snaps its lane so it starts settled (static sources
            // land zipper-free; no glide from the stolen voice's stale state).
            if self.trigger_pending[v] {
                self.smooth.snap_pitch(v, pitch_tgt[v], sweep_tgt[v]);
                self.smooth.snap_slow(v, pwm_tgt[v], xmod_tgt[v], ac.stat);
                // A stolen lane must not glide across the image from wherever
                // the previous note sat.
                self.smooth.snap_pan(v, pan_tgt[v]);
            }
            amp_c[v] = AmpCoeffs { stat: self.smooth.amp_stat_current(v), ..ac };

            // PWM: per-quantum one-pole on the matrix offset (peek here; the frame
            // loop advances it), then VXN1's clamp.
            pwm_active[v] = active[v] && self.smooth.pwm_active(v, pwm_tgt[v]);
            pan_active[v] = active[v] && self.smooth.pan_active(v, pan_tgt[v]);
            let pwm_s = self.smooth.pwm_current(v);
            pw1[v] = (ctx.osc1_pw + pwm_s).clamp(0.05, 0.95);
            pw2[v] = (ctx.osc2_pw + pwm_s).clamp(0.05, 0.95);

            // Cross-mod amount: same treatment as PWM (0242). Only the matrix
            // *offset* is smoothed; the patch scalar rides on top, so a patch
            // with no route on the dest keeps `ctx.pm_index` bit-exact and every
            // lane stays on the broadcast kernel below. Clamped non-negative —
            // `render::voice_cross_mod_amount` is the statement of that rule.
            xmod_active[v] = active[v] && pm_mode && self.smooth.xmod_active(v, xmod_tgt[v]);
            pm_idx[v] = (ctx.pm_index + self.smooth.xmod_current(v)).max(0.0);

            // Provisional `inc` from the base (smoothed pitch ≈ 0 when inactive);
            // active lanes get re-cooked per quantum in the frame loop.
            pitch_active[v] = active[v] && self.smooth.pitch_active(v, pitch_tgt[v], sweep_tgt[v]);
            self.osc1.inc[v] = note_to_hz(base1[v]) / ctx.os_sample_rate;
            self.osc2.inc[v] = note_to_hz(base2[v]) / ctx.os_sample_rate;

            // Filter key-track: the played note against VXN1's C0 pivot, at the
            // `filter_key_track` amount (0245), and — at that same amount — the
            // voice's *drifted* pitch (0218), since the keyboard CV a real VCF
            // tracks carries the VCO's drift, so the tracked cutoff wanders with
            // it. Plus the fixed per-lane cutoff tolerance: a constant
            // ±TRIM_CUTOFF_CENTS offset at full drift, enough for gentle
            // inter-voice beating, never enough to detune a whistle.
            let cutoff_hz = render::voice_cutoff_hz(
                &dests,
                ctx.cutoff,
                note[v] as f32,
                ctx.filter_key_track,
                self.osc1.drift_value[v],
                self.osc2.drift_value[v],
                self.trim.cutoff[v],
                TRIM_CUTOFF_CENTS,
                ctx.drift_amount,
            );
            // Fixed per-lane resonance tolerance: voices cross the
            // self-oscillation threshold at slightly different settings, so near
            // the edge one can whistle while a neighbour stays quiet.
            let resonance = render::voice_resonance(&dests, ctx.resonance)
                * (1.0 + self.trim.reso[v] * TRIM_RESO * ctx.drift_amount);
            self.ladder.set_coeffs(
                v,
                OtaLadderCoeffs::new(cutoff_hz, ctx.os_sample_rate, resonance, ctx.drive),
            );
        }
        let pitch_any = pitch_active.iter().any(|&a| a);
        let pwm_any = pwm_active.iter().any(|&a| a);
        let xmod_any = xmod_active.iter().any(|&a| a);
        let pan_any = pan_active.iter().any(|&a| a);
        self.ladder.set_response(ctx.filter_mode, ctx.filter_slope);

        let hpf_active = ctx.hpf_cutoff > HPF_OFF_HZ;
        if hpf_active {
            self.hpf.set_cutoff_all(ctx.hpf_cutoff, ctx.os_sample_rate);
        }
        self.ladder.prepare_ramp(base_frames);

        let mut trig = [false; N];
        trig.iter_mut()
            .zip(self.trigger_pending.iter_mut())
            .for_each(|(t, p)| *t = std::mem::take(p));

        // Scratch lane buffers.
        let mut o1 = [0.0f32; N];
        let mut o2 = [0.0f32; N];
        let mut ring = [0.0f32; N];
        let mut sub = [0.0f32; N];
        let mut noise = [0.0f32; N];
        let mut mix = [0.0f32; N];
        let mut hp = [0.0f32; N];
        let mut filt = [0.0f32; N];
        let mut amp = [0.0f32; N];

        // Voice pan (0260). The position comes from the matrix `Pan` dest —
        // the default patch routes `Spread` there at depth 1, which is how
        // unison spread survives as topology rather than hard wiring — and the
        // law is the same unity-centre constant power the layer mixer uses
        // (0248): `gl² + gr²` constant across the sweep, centre exactly 1.0.
        //
        // The old law was VXN1's equal-sum (`1 − pos`, `1 + pos`). Equal-sum is
        // defensible for a *static* placement, but total power rises toward the
        // extremes, so an LFO routed here would audibly pump — which is exactly
        // what this dest exists to allow. Centre stays unity, so a spread-0
        // patch is bit-identical and the parity fork still holds.
        let mut pan_l = [0.0f32; N];
        let mut pan_r = [0.0f32; N];
        for v in 0..N {
            let (gl, gr) = voice_pan_gains(self.smooth.pan_current(v));
            pan_l[v] = gl;
            pan_r[v] = gr;
        }

        let ring_on = ctx.ring_mode;
        let ring_gain = 10.0f32.powf(RING_DRIVE_DB / 20.0);
        let noise_on = ctx.noise_level != 0.0;
        let sub_on = ctx.sub_level != 0.0;
        // PM engages on the patch amount *or* a live matrix route into
        // `CrossModAmount` — a patch parked at amount 0 with an env into the
        // dest is a legitimate "FM swells in from nothing" sound, and keying
        // only off `ctx.pm_index` would render it unmodulated.
        let pm_on = ctx.pm_index != 0.0 || xmod_any;
        let osc1_runs = ctx.sync || pm_on || ring_on || ctx.osc1_level != 0.0 || sub_on;
        let osc2_runs = ctx.sync || pm_on || ring_on || ctx.osc2_level != 0.0;

        let env_static = self.envelopes_static(&trig, active, gate);
        // The non-env Amp part glides per frame (0208); while it's still moving
        // the VCA isn't constant even with static envelopes, so it must run the
        // per-frame path too.
        let amp_moving =
            (0..N).any(|v| active[v] && !self.smooth.amp_stat_settled(v, amp_stat_tgt[v]));
        let amp_per_frame = !env_static || amp_moving;
        // VCA constant across the block only when envelopes are static *and* the
        // non-env Amp has settled — then compute it once.
        if !amp_per_frame {
            for v in 0..N {
                amp_c[v].stat = self.smooth.amp_stat_current(v);
                amp[v] = vca(
                    active[v],
                    gate[v],
                    ctx.amp_env_bypass,
                    &amp_c[v],
                    self.env1[v].level,
                    self.env2[v].level,
                );
            }
        }

        for base_i in 0..base_frames {
            // Per-quantum pitch/PWM smoothing (0208): every PITCH_QUANTUM samples,
            // advance the pitch cascade + PWM one-pole a step and re-cook the
            // oscillator increments / pulse widths, so an LFO/env routed to
            // Pitch/XModSweep/PWM ramps in as a slope, not a block-held stair.
            // Only lanes with an active route pay this; static patches keep the
            // block-start values.
            if (pitch_any || pwm_any || xmod_any || pan_any) && base_i % PITCH_QUANTUM == 0 {
                for v in 0..N {
                    if pitch_active[v] {
                        let (p, sw) = self.smooth.tick_pitch(v, pitch_tgt[v], sweep_tgt[v]);
                        let s1 = base1[v] + p + g1 * sw;
                        let s2 = base2[v] + p + g2 * sw;
                        self.osc1.inc[v] = note_to_hz(s1) / ctx.os_sample_rate;
                        self.osc2.inc[v] = note_to_hz(s2) / ctx.os_sample_rate;
                    }
                    if pwm_active[v] {
                        let pwm_s = self.smooth.tick_pwm(v, pwm_tgt[v]);
                        pw1[v] = (ctx.osc1_pw + pwm_s).clamp(0.05, 0.95);
                        pw2[v] = (ctx.osc2_pw + pwm_s).clamp(0.05, 0.95);
                    }
                    if xmod_active[v] {
                        let xmod_s = self.smooth.tick_xmod(v, xmod_tgt[v]);
                        pm_idx[v] = (ctx.pm_index + xmod_s).max(0.0);
                    }
                    if pan_active[v] {
                        // Re-cook this lane's pan gains from the smoothed
                        // position. The summing loop keeps reading `pan_l`/
                        // `pan_r`, so the hot path is untouched — a lane with
                        // no live route never gets here and keeps its
                        // block-start gains.
                        let (gl, gr) = voice_pan_gains(self.smooth.tick_pan(v, pan_tgt[v]));
                        pan_l[v] = gl;
                        pan_r[v] = gr;
                    }
                }
            }

            // Per-frame VCA: tick envelopes (unless static) and glide the non-env
            // Amp part one frame, substituting fresh levels into the factored Amp.
            // Skipped only when both are constant (amp computed once above).
            if amp_per_frame {
                for v in 0..N {
                    let (e1, e2) = if env_static {
                        (self.env1[v].level, self.env2[v].level)
                    } else {
                        let t = trig[v] && base_i == 0;
                        (self.env1[v].tick(t, gate[v]), self.env2[v].tick(t, gate[v]))
                    };
                    amp_c[v].stat = self.smooth.tick_amp_stat(v, amp_stat_tgt[v]);
                    amp[v] = vca(active[v], gate[v], ctx.amp_env_bypass, &amp_c[v], e1, e2);
                }
            }

            let frame = base_i * os;
            for k in 0..os {
                if ctx.sync {
                    self.osc1.process_sync(
                        &mut self.osc2,
                        ctx.osc1_wave,
                        ctx.osc2_wave,
                        &pw1,
                        &pw2,
                        &mut o1,
                        &mut o2,
                    );
                } else if pm_on {
                    // Per-lane index only when a route is live: the broadcast
                    // arm keeps the single hoisted load the unmodulated patch
                    // has always had (`PmIndex` monomorphises both).
                    if xmod_any {
                        self.osc1.process_pm(
                            &mut self.osc2,
                            &pm_idx,
                            ctx.osc1_wave,
                            ctx.osc2_wave,
                            &pw1,
                            &pw2,
                            &mut o1,
                            &mut o2,
                        );
                    } else {
                        self.osc1.process_pm(
                            &mut self.osc2,
                            ctx.pm_index,
                            ctx.osc1_wave,
                            ctx.osc2_wave,
                            &pw1,
                            &pw2,
                            &mut o1,
                            &mut o2,
                        );
                    }
                } else {
                    if osc1_runs {
                        self.osc1.process(ctx.osc1_wave, &pw1, &mut o1);
                    }
                    if osc2_runs {
                        self.osc2.process(ctx.osc2_wave, &pw2, &mut o2);
                    }
                }
                if ring_on {
                    poly_ring_mod(&o1, &o2, ring_gain, &mut ring);
                    for v in 0..N {
                        mix[v] = ring[v] * ctx.osc1_level + o2[v] * ctx.osc2_level;
                    }
                } else {
                    for v in 0..N {
                        mix[v] = o1[v] * ctx.osc1_level + o2[v] * ctx.osc2_level;
                    }
                }
                if sub_on {
                    let (sp, sdt) = if ctx.sync {
                        (&self.osc2.phase, &self.osc2.inc)
                    } else {
                        (&self.osc1.phase, &self.osc1.inc)
                    };
                    poly_sub_square(sp, sdt, &self.osc1.sub_flipflop, &mut sub);
                    for v in 0..N {
                        mix[v] += sub[v] * ctx.sub_level;
                    }
                }
                if noise_on {
                    self.noise.process(ctx.noise_color, &mut noise);
                    for v in 0..N {
                        mix[v] += noise[v] * ctx.noise_level;
                    }
                }
                let ladder_in = if hpf_active {
                    self.hpf.process(&mix, &mut hp);
                    &hp
                } else {
                    &mix
                };
                self.ladder.process(ladder_in, &mut filt);
                let mut sum_l = 0.0;
                let mut sum_r = 0.0;
                for v in 0..N {
                    let s = filt[v] * amp[v];
                    sum_l += s * pan_l[v];
                    sum_r += s * pan_r[v];
                }
                out_l[frame + k] += sum_l;
                out_r[frame + k] += sum_r;
            }

            self.ladder.tick_coeffs();
            self.lfo1_onset.advance(onset_dt, onset_cap);

            if !env_static {
                for v in 0..N {
                    if active[v] && !gate[v] && self.env1[v].is_idle() && self.env2[v].is_idle() {
                        active[v] = false;
                    }
                }
            }
        }
    }
}

/// Decorrelated per-lane LFO 1 seed (VXN1).
#[inline]
fn lfo1_seed(base: u64, lane: usize) -> u64 {
    base.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((lane as u64 + 1).wrapping_mul(0x632B_E5A6))
}

/// Fixed per-lane pan positions in [-1, 1], evenly spread (VXN1's `PAN_POSITIONS`).
#[inline]
fn pan_position(lane: usize) -> f32 {
    const P: [f32; N] = [
        -1.0,
        -5.0 / 7.0,
        -3.0 / 7.0,
        -1.0 / 7.0,
        1.0 / 7.0,
        3.0 / 7.0,
        5.0 / 7.0,
        1.0,
    ];
    P[lane]
}

/// Constant-power pan gains for a voice position in `[-1, 1]` (0260),
/// normalised to unity at centre — the same law as the layer mixer's
/// [`crate::engine::pan_gains`], applied per lane.
///
/// `gl² + gr²` is constant across the sweep, so a voice swept by an LFO holds
/// its apparent loudness; the `√2` puts centre at exactly 1.0, so a centred
/// (spread-0) patch renders as it did under the old equal-sum law and the
/// parity fork stays valid.
#[inline]
fn voice_pan_gains(pos: f32) -> (f32, f32) {
    let theta = (pos.clamp(-1.0, 1.0) + 1.0) * core::f32::consts::FRAC_PI_4;
    let (sin, cos) = theta.sin_cos();
    (core::f32::consts::SQRT_2 * cos, core::f32::consts::SQRT_2 * sin)
}

/// Portamento glide `(active, coeff)` for the block (VXN1, unison scaling
/// dropped — Poly only). Time 0 snaps.
#[inline]
fn block_glide(portamento_time: f32, base_frames: usize, base_rate: f32) -> (bool, f32) {
    if portamento_time <= 0.0 {
        return (false, 1.0);
    }
    let dt = base_frames as f32 / base_rate;
    (true, 1.0 - (-dt / portamento_time).exp())
}

/// Factor the Amp dest into `static + e1·env1 + e2·env2` from the block-start
/// source table, so the per-frame VCA needs only two FMAs. `Lin`-curve Env→Amp
/// slots (incl. the default Env2→Amp) contribute to `e1`/`e2`; every other Amp
/// slot folds into `static` at its block-start value.
fn amp_coeffs(table: &MatrixTable, sources: &crate::eval::SourceVals) -> AmpCoeffs {
    use crate::matrix::Curve;
    let amp_di = DestId::Amp.idx().unwrap();
    let gain = crate::eval::DEST_GAIN[amp_di];
    let mut c = AmpCoeffs::default();
    for slot in &table.slots {
        if slot.dest != DestId::Amp || slot.depth == 0.0 {
            continue;
        }
        let Some(si) = slot.source.idx() else { continue };
        let scale = match slot.scale_src.idx() {
            Some(sc) => crate::eval::scale_norm(slot.scale_src, sources[sc]),
            None => 1.0,
        };
        // `cook_depth` is identity for Amp — called for parity with
        // `eval_dests` so a future tapered dest can't diverge here.
        let coeff = slot.dest.cook_depth(slot.depth) * gain * scale;
        // Linear env sources become per-frame coefficients; everything else is
        // resolved at block-start value into `stat`.
        match (slot.source, slot.curve) {
            (SourceId::Env1, Curve::Lin) => c.e1 += coeff,
            (SourceId::Env2, Curve::Lin) => c.e2 += coeff,
            _ => c.stat += render_shape(slot.curve, sources[si]) * coeff,
        }
    }
    c
}

/// Curve shaping mirror of [`crate::eval`]'s `shape` (kept private there); used
/// only by [`amp_coeffs`] for the non-linear fold into `stat`.
#[inline]
fn render_shape(curve: crate::matrix::Curve, v: f32) -> f32 {
    use crate::matrix::Curve;
    match curve {
        Curve::Lin => v,
        Curve::Exp => v.abs() * v,
        Curve::Log => {
            let m = v.abs().sqrt();
            if v < 0.0 { -m } else { m }
        }
        Curve::Bipolar => 2.0 * v - 1.0,
    }
}

/// Per-frame VCA gain: 0 when inactive; gate-only in bypass (organ); else the
/// factored Amp total (`static + e1·env1 + e2·env2`) clamped non-negative. For
/// the default Env2→Amp this is exactly `env2` — VXN1's `amp_base(env2)`.
#[inline]
fn vca(active: bool, gate: bool, bypass: bool, c: &AmpCoeffs, env1: f32, env2: f32) -> f32 {
    if !active {
        0.0
    } else if bypass {
        if gate { 1.0 } else { 0.0 }
    } else {
        (c.stat + c.e1 * env1 + c.e2 * env2).max(0.0)
    }
}

/// Gate the `XModSweep` dest onto the mode-selected osc, matching
/// [`render::voice_pitches`]: Off/Ring → both, Sync → osc1, PM → osc2. Returned
/// as `(g1, g2)` multipliers so the frame loop can fold the smoothed sweep into
/// each osc's pitch with a single FMA.
#[inline]
fn sweep_gates(mode: CrossModType) -> (f32, f32) {
    match mode {
        CrossModType::Off | CrossModType::Ring => (1.0, 1.0),
        CrossModType::Sync => (1.0, 0.0),
        CrossModType::Pm => (0.0, 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matrix::{Curve, MatrixSlot, default_patch};

    #[allow(clippy::type_complexity)]
    fn book() -> ([u8; N], [bool; N], [bool; N], [f32; N], [f32; N], [f32; N]) {
        // One active gated voice on lane 0, note 60, full velocity.
        let mut note = [60u8; N];
        note[0] = 60;
        let mut gate = [false; N];
        let mut active = [false; N];
        gate[0] = true;
        active[0] = true;
        (note, gate, active, [1.0; N], [0.0; N], [0.0; N])
    }

    fn ctx<'a>(m: &'a MatrixTable) -> BlockCtx<'a> {
        BlockCtx {
            os_sample_rate: 48_000.0,
            os: 1,
            osc1_wave: Waveform::Saw,
            osc2_wave: Waveform::Saw,
            osc1_level: 0.8,
            osc2_level: 0.0,
            sub_level: 0.0,
            noise_level: 0.0,
            noise_color: NoiseColor::White,
            osc1_pw: 0.5,
            osc2_pw: 0.5,
            osc1_semi: 0.0,
            osc2_semi: 0.0,
            sync: false,
            pm_index: 0.0,
            ring_mode: false,
            cross_mod_type: CrossModType::Off,
            cutoff: 8000.0,
            filter_key_track: 0.0,
            hpf_cutoff: 20.0,
            resonance: 0.2,
            drive: 1.0,
            filter_mode: FilterMode::Lp,
            filter_slope: FilterSlope::Pole4,
            base_semis: 0.0,
            lfo1_shape: LfoShape::Sine,
            lfo1_rate_hz: 5.0,
            lfo1_delay_time: 0.0,
            lfo1_fade: 0.0,
            lfo2_val: 0.0,
            portamento_time: 0.0,
            amp_env_bypass: false,
            drift_amount: 0.0,
            spread: 0.0,
            matrix: m,
            mod_wheel: 0.0,
            pitch_wheel: 0.0,
        }
    }

    // ── Pan as a matrix destination (0260) ──────────────────────────────────

    /// Render one lane and return its `(L, R)` peaks.
    fn lane_peaks(bank: &mut RenderBank, c: &BlockCtx, lane: usize, frames: usize) -> (f32, f32) {
        let mut note = [60u8; N];
        note[lane] = 60;
        let mut gate = [false; N];
        let mut active = [false; N];
        gate[lane] = true;
        active[lane] = true;
        let mut l = vec![0.0; frames];
        let mut r = vec![0.0; frames];
        bank.render(c, &note, &gate, &mut active, &[1.0; N], &[0.0; N], &[0.0; N], &mut l, &mut r);
        (
            l.iter().fold(0.0f32, |a, &s| a.max(s.abs())),
            r.iter().fold(0.0f32, |a, &s| a.max(s.abs())),
        )
    }

    fn fast_bank() -> RenderBank {
        let mut bank = RenderBank::new(48_000.0, 1);
        bank.set_envelopes(
            (0.001, 0.2, 1.0, 0.2), AdsrShape::Linear,
            (0.001, 0.2, 1.0, 0.2), AdsrShape::Linear, 0.0,
        );
        bank
    }

    /// The law: unity at centre, constant power across the sweep.
    #[test]
    fn voice_pan_law_is_constant_power_with_unity_centre() {
        let (cl, cr) = voice_pan_gains(0.0);
        assert!((cl - 1.0).abs() < 1e-6 && (cr - 1.0).abs() < 1e-6, "centre {cl},{cr}");
        for pos in [-1.0_f32, -0.5, 0.0, 0.25, 1.0] {
            let (gl, gr) = voice_pan_gains(pos);
            assert!((gl * gl + gr * gr - 2.0).abs() < 1e-5, "power at {pos}");
        }
        assert!(voice_pan_gains(-1.0).1.abs() < 1e-6, "hard left must silence R");
        assert!(voice_pan_gains(1.0).0.abs() < 1e-6, "hard right must silence L");
    }

    /// The default `Spread → Pan` route reproduces VXN1's unison spread: with
    /// Spread up, lane 0 (hard left) and lane 7 (hard right) land in opposite
    /// channels — the behaviour that used to be hard-wired DSP.
    #[test]
    fn default_route_places_lanes_across_the_image() {
        let m = default_patch();
        let mut c = ctx(&m);
        c.spread = 1.0;

        let mut bank = fast_bank();
        bank.trigger_lane(0, LfoShape::Sine, false);
        let (l0, r0) = lane_peaks(&mut bank, &c, 0, 512);
        assert!(l0 > 0.0 && r0 < l0 * 1e-3, "lane 0 must sit hard left: {l0} vs {r0}");

        let mut bank = fast_bank();
        bank.trigger_lane(7, LfoShape::Sine, false);
        let (l7, r7) = lane_peaks(&mut bank, &c, 7, 512);
        assert!(r7 > 0.0 && l7 < r7 * 1e-3, "lane 7 must sit hard right: {l7} vs {r7}");
    }

    /// Spread at 0 centres every lane, so the two channels are bit-identical —
    /// the parity condition, and what the unity-centre normalisation preserves.
    #[test]
    fn spread_zero_centres_every_lane() {
        let m = default_patch();
        let c = ctx(&m); // spread: 0.0
        let mut bank = fast_bank();
        bank.trigger_lane(0, LfoShape::Sine, false);
        let (note, gate, mut active, vel, pres, rnd) = book();
        let mut l = vec![0.0; 512];
        let mut r = vec![0.0; 512];
        bank.render(&c, &note, &gate, &mut active, &vel, &pres, &rnd, &mut l, &mut r);
        assert!(l.iter().any(|&s| s != 0.0), "the voice must sound");
        assert_eq!(l, r, "spread 0 must stay bit-mono");
    }

    /// Deleting the route makes the Spread knob inert — the honest consequence
    /// of spread being topology now, and worth pinning so it is not mistaken
    /// for a bug later.
    #[test]
    fn without_the_route_spread_does_nothing() {
        let mut m = default_patch();
        m.slots[2] = MatrixSlot::default();
        let mut c = ctx(&m);
        c.spread = 1.0;
        let mut bank = fast_bank();
        bank.trigger_lane(0, LfoShape::Sine, false);
        let (l, r) = lane_peaks(&mut bank, &c, 0, 512);
        assert!(l > 0.0);
        assert!((l - r).abs() < l * 1e-3, "no Pan route ⇒ centred: {l} vs {r}");
    }

    /// An LFO into `Pan` moves a *centred* voice — auto-pan, the thing this
    /// dest exists for. Uses a slow LFO so the block lands on one side.
    #[test]
    fn lfo_into_pan_moves_a_centred_voice() {
        let mut m = default_patch();
        m.slots[3] = MatrixSlot {
            source: SourceId::Lfo2,
            dest: DestId::Pan,
            depth: 1.0,
            curve: Curve::Lin,
            scale_src: SourceId::None,
        };
        let mut c = ctx(&m);
        c.spread = 0.0; // no spread contribution: the LFO is the only pan source

        // LFO 2 pinned hard positive ⇒ hard right, and vice versa.
        c.lfo2_val = 1.0;
        let mut bank = fast_bank();
        bank.trigger_lane(0, LfoShape::Sine, false);
        let (l_right, r_right) = lane_peaks(&mut bank, &c, 0, 2048);
        assert!(r_right > l_right * 10.0, "LFO +1 must pan right: {l_right} vs {r_right}");

        c.lfo2_val = -1.0;
        let mut bank = fast_bank();
        bank.trigger_lane(0, LfoShape::Sine, false);
        let (l_left, r_left) = lane_peaks(&mut bank, &c, 0, 2048);
        assert!(l_left > r_left * 10.0, "LFO −1 must pan left: {l_left} vs {r_left}");
    }

    /// A pan move ramps rather than stepping. Measured on the *envelope* of the
    /// channel being panned into: with a block-rate step, R would be at full
    /// amplitude from the first sample of the new block; with the one-pole it
    /// fills in across the block.
    ///
    /// (Raw sample-to-sample slew is the wrong probe here — a saw's own reset
    /// dwarfs the pan ramp and would mask a step entirely.)
    #[test]
    fn pan_moves_ramp_rather_than_stepping() {
        let mut m = default_patch();
        m.slots[3] = MatrixSlot {
            source: SourceId::Lfo2,
            dest: DestId::Pan,
            depth: 1.0,
            curve: Curve::Lin,
            scale_src: SourceId::None,
        };
        let mut c = ctx(&m);
        c.spread = 0.0;
        c.lfo2_val = -1.0; // hard left: R silent
        let mut bank = fast_bank();
        bank.trigger_lane(0, LfoShape::Sine, false);
        let (note, gate, mut active, vel, pres, rnd) = book();
        let mut l = vec![0.0; 512];
        let mut r = vec![0.0; 512];
        bank.render(&c, &note, &gate, &mut active, &vel, &pres, &rnd, &mut l, &mut r);
        assert!(r.iter().all(|&s| s.abs() < 1e-3), "hard left should leave R quiet");

        // Slam to hard right for the next block.
        c.lfo2_val = 1.0;
        bank.render(&c, &note, &gate, &mut active, &vel, &pres, &rnd, &mut l, &mut r);
        let head = r[..64].iter().fold(0.0f32, |a, &s| a.max(s.abs()));
        let tail = r[448..].iter().fold(0.0f32, |a, &s| a.max(s.abs()));
        assert!(tail > 0.0, "the voice must arrive in R");
        assert!(
            head < tail * 0.75,
            "R should fill in across the block, not step: head {head} vs tail {tail}"
        );
    }

    #[test]
    fn silent_bank_renders_silence() {
        let m = default_patch();
        let mut bank = RenderBank::new(48_000.0, 1);
        let note = [60u8; N];
        let gate = [false; N];
        let mut active = [false; N];
        let mut l = vec![0.0; 64];
        let mut r = vec![0.0; 64];
        bank.render(&ctx(&m), &note, &gate, &mut active, &[0.0; N], &[0.0; N], &[0.0; N], &mut l, &mut r);
        assert!(l.iter().chain(r.iter()).all(|&s| s == 0.0));
    }

    #[test]
    fn triggered_voice_renders_sound() {
        let m = default_patch();
        let mut bank = RenderBank::new(48_000.0, 1);
        // Fast attack so the VCA opens within the block.
        bank.set_envelopes((0.001, 0.2, 0.8, 0.2), AdsrShape::Linear, (0.001, 0.2, 0.8, 0.2), AdsrShape::Linear, 0.0);
        bank.trigger_lane(0, LfoShape::Sine, false);
        let (note, gate, mut active, vel, pres, rnd) = book();
        let mut l = vec![0.0; 256];
        let mut r = vec![0.0; 256];
        bank.render(&ctx(&m), &note, &gate, &mut active, &vel, &pres, &rnd, &mut l, &mut r);
        let peak = l.iter().fold(0.0f32, |a, &s| a.max(s.abs()));
        assert!(peak > 0.0, "a gated voice with the default patch must make sound");
    }

    #[test]
    fn per_frame_amp_tracks_fast_attack_env2() {
        // Default patch = Env2→Amp. With a fast linear attack, the VCA (hence
        // output envelope) must rise within the block — proving per-frame Amp,
        // not a block-rate step.
        let m = default_patch();
        let mut bank = RenderBank::new(48_000.0, 1);
        bank.set_envelopes((0.001, 0.2, 0.8, 0.2), AdsrShape::Linear, (0.005, 0.2, 0.8, 0.2), AdsrShape::Linear, 0.0);
        bank.trigger_lane(0, LfoShape::Sine, false);
        let (note, gate, mut active, vel, pres, rnd) = book();
        let mut l = vec![0.0; 512];
        let mut r = vec![0.0; 512];
        let mut c = ctx(&m);
        c.os = 1;
        bank.render(&c, &note, &gate, &mut active, &vel, &pres, &rnd, &mut l, &mut r);
        // Early samples (attack just started) quieter than late samples.
        let early = l[..64].iter().fold(0.0f32, |a, &s| a.max(s.abs()));
        let late = l[448..].iter().fold(0.0f32, |a, &s| a.max(s.abs()));
        assert!(late > early, "VCA should rise across the block (early {early}, late {late})");
    }

    #[test]
    fn released_voice_frees_when_env_idle() {
        let m = default_patch();
        let mut bank = RenderBank::new(48_000.0, 1);
        // Near-instant release so the voice idles within one block.
        bank.set_envelopes((0.001, 0.001, 0.0, 0.001), AdsrShape::Linear, (0.001, 0.001, 0.0, 0.001), AdsrShape::Linear, 0.0);
        bank.trigger_lane(0, LfoShape::Sine, false);
        let note = [60u8; N];
        let gate = [false; N]; // already released
        let mut active = [false; N];
        active[0] = true;
        let mut l = vec![0.0; 256];
        let mut r = vec![0.0; 256];
        bank.render(&ctx(&m), &note, &gate, &mut active, &[1.0; N], &[0.0; N], &[0.0; N], &mut l, &mut r);
        assert!(!active[0], "an idle released voice must free");
    }

    // ── 0218: global drift / per-voice component trims ──

    /// Consolidated [`VoiceTrim`] properties: bounded draws, per-seed
    /// determinism, and decorrelated streams — the three properties that define
    /// the per-voice spread contract (ported with the trims from VXN1).
    #[test]
    fn trim_properties() {
        // Bounded + varied: every draw stays in [-1, 1] and the lanes are not
        // all identical (the whole point is per-voice spread).
        let t = VoiceTrim::new(0x1234_5678);
        for arr in [&t.env_time, &t.sustain, &t.cutoff, &t.reso] {
            for &x in arr {
                assert!((-1.0..=1.0).contains(&x), "draw {x} out of [-1,1]");
            }
            let first = arr[0];
            assert!(
                arr.iter().any(|&x| (x - first).abs() > 1e-3),
                "all lanes identical — no variance"
            );
        }

        // Deterministic per seed; distinct seeds decorrelate (so a synth's two
        // banks, and the two layers, never share a spread).
        let a = VoiceTrim::new(0xABCD);
        let b = VoiceTrim::new(0xABCD);
        assert_eq!(a.cutoff, b.cutoff);
        assert_eq!(a.env_time, b.env_time);
        let c = VoiceTrim::new(0xABCE);
        assert!(c.cutoff != a.cutoff, "distinct seeds must decorrelate");

        // The four targets draw from distinct salts, so a bright filter does not
        // imply a long decay.
        let d = VoiceTrim::new(0x0F0F_0F0F);
        assert!(d.env_time != d.cutoff);
        assert!(d.cutoff != d.reso);
        assert!(d.sustain != d.env_time);
    }

    #[test]
    fn cutoff_trim_stays_in_tune() {
        // Base-cutoff variance must beat gently, never detune a self-osc
        // whistle: the worst case is max|draw| (= 1) × TRIM_CUTOFF_CENTS at full
        // drift, and must stay inside ±5 cents (we target ±3).
        let t = VoiceTrim::new(0x55AA_55AA);
        let worst = t.cutoff.iter().map(|&x| (x * TRIM_CUTOFF_CENTS).abs()).fold(0.0, f32::max);
        assert!(worst <= 5.0, "voice cutoff offset {worst} cents too large");
    }

    /// Magnitudes match VXN1's (`vxn-engine/src/voice.rs`) — the trims are a
    /// straight lift, so a drift setting sounds the same in both synths.
    #[test]
    fn trim_magnitudes_match_vxn1() {
        assert_eq!(TRIM_ENV_TIME, 0.12);
        assert_eq!(TRIM_SUSTAIN, 0.03);
        assert_eq!(TRIM_RESO, 0.07);
        assert_eq!(TRIM_CUTOFF_CENTS, 3.0);
    }

    /// Envelope trims: at drift 0 every lane is cooked bit-identically; above
    /// zero the lanes' attack rates spread. Measured behaviourally by ticking
    /// each lane's amp envelope (`AdsrCore` exposes no getters).
    #[test]
    fn drift_spreads_envelope_times_across_lanes() {
        let attack_levels = |bank: &mut RenderBank| -> [f32; N] {
            for e in &mut bank.env2 {
                e.reset();
            }
            let mut out = [0.0f32; N];
            for (v, e) in bank.env2.iter_mut().enumerate() {
                let mut level = 0.0;
                for i in 0..64 {
                    level = e.tick(i == 0, true);
                }
                out[v] = level;
            }
            out
        };
        let env = (0.05, 0.1, 0.5, 0.2);

        let mut bank = RenderBank::new(48_000.0, 0xBEEF);
        bank.set_envelopes(env, AdsrShape::Linear, env, AdsrShape::Linear, 0.0);
        let flat = attack_levels(&mut bank);
        assert!(
            flat.iter().all(|&x| x == flat[0]),
            "drift 0 must cook every lane identically: {flat:?}"
        );

        bank.set_envelopes(env, AdsrShape::Linear, env, AdsrShape::Linear, 1.0);
        let spread = attack_levels(&mut bank);
        assert!(
            spread.iter().any(|&x| (x - spread[0]).abs() > 1e-6),
            "drift 1 must spread the lanes' attack rates: {spread:?}"
        );
    }

    /// The filter/envelope trims reach the DSP independently of the osc pitch
    /// walk: on a **noise-only** patch (both oscs muted) pitch drift can't touch
    /// the output, so any difference between drift 0 and drift 1 is the
    /// cutoff/resonance/envelope trims.
    #[test]
    fn trims_change_output_with_the_oscillators_muted() {
        let m = default_patch();
        let render = |drift: f32| -> Vec<f32> {
            let mut bank = RenderBank::new(48_000.0, 7);
            bank.set_envelopes(
                (0.001, 0.2, 0.8, 0.2),
                AdsrShape::Linear,
                (0.001, 0.2, 0.8, 0.2),
                AdsrShape::Linear,
                drift,
            );
            bank.trigger_lane(0, LfoShape::Sine, false);
            let (note, gate, mut active, vel, pres, rnd) = book();
            let mut c = ctx(&m);
            c.osc1_level = 0.0;
            c.osc2_level = 0.0;
            c.noise_level = 0.8;
            c.resonance = 0.6;
            c.drift_amount = drift;
            let mut l = vec![0.0; 512];
            let mut r = vec![0.0; 512];
            bank.render(&c, &note, &gate, &mut active, &vel, &pres, &rnd, &mut l, &mut r);
            l
        };
        let dry = render(0.0);
        let drifted = render(1.0);
        assert!(dry.iter().any(|&s| s != 0.0), "noise patch must sound");
        assert!(
            dry.iter().zip(&drifted).any(|(x, y)| (x - y).abs() > 1e-9),
            "cutoff/reso/envelope trims must reach the DSP"
        );
        // Same drift twice → bit-identical: the trims are frozen draws, not a walk.
        assert_eq!(dry, render(0.0));
    }

    /// Key-track is a param (0245), not a matrix scrape: the drift coupling and
    /// the note term both ride `ctx.filter_key_track`, so a patch whose matrix
    /// never mentions Key still tracks. The maths lives in
    /// [`render::voice_cutoff_hz`]; this pins the wiring — the block ctx carries
    /// the param through to the per-lane cutoff.
    #[test]
    fn key_track_comes_from_the_param_not_the_matrix() {
        let m = MatrixTable::default(); // no Key→Cutoff route at all
        let mut c = ctx(&m);
        c.filter_key_track = 1.0;
        c.cutoff = 16.3516; // C0 — cutoff should land on the played note
        let dests = [0.0f32; crate::matrix::N_DESTS];
        let hz = render::voice_cutoff_hz(
            &dests,
            c.cutoff,
            69.0, // A4
            c.filter_key_track,
            0.0,
            0.0,
            0.0,
            TRIM_CUTOFF_CENTS,
            0.0,
        );
        assert!((hz - 440.0).abs() < 0.5, "A4 with full key-track → 440 Hz, got {hz}");
    }
}
