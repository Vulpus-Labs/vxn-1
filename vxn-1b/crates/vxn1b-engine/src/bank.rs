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
//! **Scope (this step).** All four assign modes are live: the
//! [`crate::voice::Voices`] coordinator resolves Poly/Unison/Solo/Twin and hands
//! this bank a per-lane detune (cents) plus a stack-width `level_comp`.
//! `CrossModAmount` is live per lane (0242) — the PM kernel takes a per-lane
//! index whenever a route is active, and the broadcast scalar otherwise, so an
//! unrouted patch is bit-unchanged. `HpfCutoff` is still deferred (the HPF is
//! set bank-wide) — inert at the factory default, so the parity gate is
//! unaffected. The
//! per-voice component trims landed with global drift (0218) and are likewise
//! inert at the default `MasterDrift = 0`.

use vxn_dsp::{
    AdsrCore, AdsrShape, AdsrStage, CHANNELS_PER_LAYER, CONTROL_BLOCK, FilterMode, FilterSlope,
    LfoCore, LfoShape, NoiseColor, OtaLadderCoeffs, PolyHpf, PolyNoiseBank, PolyOscillator,
    PolyOtaLadder, Waveform, note_to_hz, poly_ring_mod, poly_sub_square,
};

use crate::eval::{SourceInputs, env_time_scale, eval_dests, eval_sources, lfo_rate_scale};
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
    /// Stack-width output scaling from the voice allocator (`1/√len`), so
    /// Unison's 16 copies and Twin's 2 don't jump the level against Poly's 1.
    /// Folded into the per-lane pan gains once per block.
    pub level_comp: f32,
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
    /// The patch's envelope settings as last pushed by [`Self::set_envelopes`],
    /// kept so a lane's cooked params can be *re-derived* (0268). Without this
    /// the per-lane note-on env scale would be wiped by the next envelope or
    /// drift param change, which re-pushes the patch values to every lane.
    env_patch: EnvPatch,
    /// Per-lane A/D/R multiplier from the matrix, latched at note-on (0268):
    /// `[0]` = env 1, `[1]` = env 2. Exactly `1.0` when nothing is routed.
    env_scale: [[f32; N]; 2],
    /// Per-lane sustain-level *offset* from the matrix, latched at note-on
    /// alongside [`Self::env_scale`] (0270). Additive, clamped into `[0, 1]` at
    /// apply time; exactly `0.0` when nothing is routed.
    env_sus_mod: [[f32; N]; 2],
    /// Per-lane `Lfo1Rate` dest total carried over from the **previous** control
    /// block (0269). LFO 1 is a matrix *source*, so the lanes must tick before
    /// the matrix is evaluated; reading last block's total is what breaks that
    /// circle. Exactly `0.0` (→ unity rate) when nothing is routed.
    lfo1_rate_mod: [f32; N],
}

/// The envelope half of the patch: both ADSRs, their shapes, and the drift
/// amount scaling the per-lane trims. Held by [`RenderBank`] so
/// [`RenderBank::apply_env_lane`] can re-cook one lane without the caller
/// having to re-supply the patch (0268).
#[derive(Clone, Copy)]
struct EnvPatch {
    env1: (f32, f32, f32, f32),
    env1_shape: AdsrShape,
    env2: (f32, f32, f32, f32),
    env2_shape: AdsrShape,
    drift: f32,
}

impl Default for EnvPatch {
    fn default() -> Self {
        // Matches `AdsrCore::new`'s zeroed cook, so a bank that somehow renders
        // before the synth's first `set_envelopes` behaves as it always did.
        Self {
            env1: (0.0, 0.0, 0.0, 0.0),
            env1_shape: AdsrShape::Linear,
            env2: (0.0, 0.0, 0.0, 0.0),
            env2_shape: AdsrShape::Linear,
            drift: 0.0,
        }
    }
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
            env_patch: EnvPatch::default(),
            env_scale: [[1.0; N]; 2],
            env_sus_mod: [[0.0; N]; 2],
            lfo1_rate_mod: [0.0; N],
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
        // Cooked note-on scales belong to notes, not to the lane: a reset has
        // no notes, so every lane returns to the patch's plain envelope times.
        self.env_scale = [[1.0; N]; 2];
        self.env_sus_mod = [[0.0; N]; 2];
        for v in 0..N {
            self.apply_env_lane(v);
        }
        self.lfo1_rate_mod = [0.0; N];
    }

    /// Apply ADSR params to all lanes (called when an envelope param *or*
    /// `drift_amount` changed).
    ///
    /// `drift_amount` scales the fixed per-lane trims (0218): each lane's A/D/R
    /// times and sustain get a constant multiplicative nudge from [`VoiceTrim`],
    /// so a held chord's voices breathe at subtly different rates like real
    /// per-voice analog tolerance. At `drift_amount = 0` every factor is exactly
    /// `1.0`, so all lanes receive bit-identical params.
    ///
    /// The patch is *stored* as well as applied (0268), so the per-lane note-on
    /// envelope scale can be folded in here and survive a param change mid-note:
    /// tweaking Decay while a scaled note rings re-cooks that lane at its own
    /// multiplier rather than snapping it back to the patch value.
    pub fn set_envelopes(
        &mut self,
        env1: (f32, f32, f32, f32),
        env1_shape: AdsrShape,
        env2: (f32, f32, f32, f32),
        env2_shape: AdsrShape,
        drift_amount: f32,
    ) {
        self.env_patch = EnvPatch { env1, env1_shape, env2, env2_shape, drift: drift_amount };
        for v in 0..N {
            self.apply_env_lane(v);
        }
    }

    /// Cook one lane's two envelopes from the stored patch × that lane's drift
    /// trims (0218) × its latched matrix time scale (0268), with the latched
    /// sustain offset added on top (0270). The time factors are independent
    /// multipliers on A/D/R; sustain takes the drift trim multiplicatively and
    /// the matrix offset additively, clamped into `[0, 1]` last so no
    /// combination of the two can leave the legal range.
    fn apply_env_lane(&mut self, v: usize) {
        let p = self.env_patch;
        let time_mag = TRIM_ENV_TIME * p.drift;
        let sus_mag = TRIM_SUSTAIN * p.drift;
        let trim_t = 1.0 + self.trim.env_time[v] * time_mag;
        let trim_s = 1.0 + self.trim.sustain[v] * sus_mag;

        let t1 = trim_t * self.env_scale[0][v];
        let s1 = (p.env1.2 * trim_s + self.env_sus_mod[0][v]).clamp(0.0, 1.0);
        self.env1[v].set_params(p.env1.0 * t1, p.env1.1 * t1, s1, p.env1.3 * t1);
        self.env1[v].set_shape(p.env1_shape);

        let t2 = trim_t * self.env_scale[1][v];
        let s2 = (p.env2.2 * trim_s + self.env_sus_mod[1][v]).clamp(0.0, 1.0);
        self.env2[v].set_params(p.env2.0 * t2, p.env2.1 * t2, s2, p.env2.3 * t2);
        self.env2[v].set_shape(p.env2_shape);
    }

    /// Latch lane `v`'s envelope time scales (0268) and sustain offsets (0270)
    /// from this block's dest totals, and re-cook the lane if any moved.
    ///
    /// Called **only** on the lane's note-on trigger: `AdsrCore` holds cooked
    /// per-sample increments, so tracking the dest continuously would make a
    /// held note's decay lurch every time the source moved. Latched at the
    /// trigger, each note in a chord instead keeps whatever length the sources
    /// (mod wheel, `Spread`, `NoteRandom`, velocity, key, LFO 2 sampled at the
    /// keypress) said at the moment it started, for the whole life of the note —
    /// including its release, which is the point of scaling R at all.
    ///
    /// The same argument covers sustain (0270): it is the envelope's *held*
    /// level, and it also sets the decay rate, so tracking it continuously
    /// would both step a ringing note and bend a decay already in flight.
    ///
    /// Note the two sources that read as ~zero here by construction: the
    /// envelopes themselves (level ≈ 0 at the trigger) and `Aftertouch`
    /// (pressure arrives after the note). They are routable, just not useful.
    #[inline]
    fn cook_env_mods(&mut self, v: usize, dests: &[f32; crate::matrix::N_DESTS]) {
        let t1 = env_time_scale(dests[DestId::Env1Scale.idx().unwrap()]);
        let t2 = env_time_scale(dests[DestId::Env2Scale.idx().unwrap()]);
        let u1 = dests[DestId::Env1Sustain.idx().unwrap()];
        let u2 = dests[DestId::Env2Sustain.idx().unwrap()];
        if t1 != self.env_scale[0][v]
            || t2 != self.env_scale[1][v]
            || u1 != self.env_sus_mod[0][v]
            || u2 != self.env_sus_mod[1][v]
        {
            self.env_scale[0][v] = t1;
            self.env_scale[1][v] = t2;
            self.env_sus_mod[0][v] = u1;
            self.env_sus_mod[1][v] = u2;
            self.apply_env_lane(v);
        }
    }

    /// DSP trigger for lane `v` at note-on: reset oscillators to a decorrelated
    /// start phase, restart the LFO 1 onset (and phase unless free-running), and
    /// mark a pending trigger for the next render's envelope re-arm.
    ///
    /// `start_phase` overrides the lane's deterministic phase: `None` keeps
    /// [`lane_phase`] (Poly / Solo / Twin — decorrelated but reproducible),
    /// `Some(p)` stamps `p`. Unison passes a fresh random phase per voice so a
    /// stack of near-identical copies doesn't comb into a synchronised null.
    pub fn trigger_lane(
        &mut self,
        v: usize,
        lfo1_shape: LfoShape,
        lfo1_free_run: bool,
        start_phase: Option<f32>,
    ) {
        self.trigger_pending[v] = true;
        self.lfo1_onset.retrigger(v);
        if !lfo1_free_run {
            self.lfo1[v].retrigger(lfo1_shape);
        }
        self.osc1.reset(v);
        self.osc2.reset(v);
        let ph = start_phase.unwrap_or_else(|| lane_phase(v));
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
        detune_cents: &[f32],
        stack_pos: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        let os = ctx.os;
        let base_frames = out_l.len() / os;
        let base_rate = ctx.os_sample_rate / os as f32;

        // Per-voice LFO 1: tick each lane's phase once for this block. LFOs tick
        // even on silent blocks so free-run phase keeps drifting.
        // The rate is the panel's resolved Hz — sync already applied (0267) —
        // times this lane's `Lfo1Rate` multiplier from last block (0269), so a
        // synced LFO under a power-of-two amount stays on the grid.
        let mut lfo1_raw = [0.0f32; N];
        for (v, (lfo, raw)) in self.lfo1.iter_mut().zip(lfo1_raw.iter_mut()).enumerate() {
            lfo.set_rate(ctx.lfo1_rate_hz * lfo_rate_scale(self.lfo1_rate_mod[v]));
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
        // Per-oscillator PWM targets (0261): the combined `Pwm` dest summed with
        // each osc's own dest, so the pair is equal whenever only `Pwm` is routed.
        let mut pwm_tgt = [(0.0f32, 0.0f32); N];
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
                // The lane's place in the image comes from its position within
                // its *stack* (ADR 0003), not from its lane index: a stack's
                // lanes are wherever the allocator put them, and stacks vary in
                // width. Width 1 stamps 0.0, so a plain poly voice is centred.
                spread_pos: stack_pos[v] * ctx.spread,
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
            // drift/glide/detune are already block-rate. `detune` is the
            // allocator's per-lane Unison/Twin offset in cents; it lands on both
            // oscillators, so the whole voice moves rather than beating with
            // itself (that is Osc 2 Fine's job).
            let detune = detune_cents[v] * 0.01;
            base1[v] = ctx.base_semis + nf + ctx.osc1_semi + detune + self.osc1.drift_value[v];
            base2[v] = ctx.base_semis + nf + ctx.osc2_semi + detune + self.osc2.drift_value[v];
            pitch_tgt[v] = dests[DestId::Pitch.idx().unwrap()];
            sweep_tgt[v] = dests[DestId::XModSweep.idx().unwrap()];
            pwm_tgt[v] = (
                render::pwm_offset(&dests, DestId::Osc1Pwm),
                render::pwm_offset(&dests, DestId::Osc2Pwm),
            );
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
            // Next block's rate for this lane (0269).
            self.lfo1_rate_mod[v] = dests[DestId::Lfo1Rate.idx().unwrap()];

            if self.trigger_pending[v] {
                // Envelope time scales (0268) and sustain offsets (0270) are
                // latched here, before the envelopes re-arm below.
                self.cook_env_mods(v, &dests);
                // A fresh note must not inherit the *stolen* note's LFO rate for
                // a block: re-rate the lane now from this block's own total. The
                // tick above already happened, so this lands from the next one —
                // one block earlier than the carry-over would.
                self.lfo1[v].set_rate(ctx.lfo1_rate_hz * lfo_rate_scale(self.lfo1_rate_mod[v]));
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
            // Clamped per oscillator (0261): a route driving osc 1 to the rail
            // must not clip osc 2's width.
            let (pwm_s1, pwm_s2) = self.smooth.pwm_current(v);
            pw1[v] = (ctx.osc1_pw + pwm_s1).clamp(0.05, 0.95);
            pw2[v] = (ctx.osc2_pw + pwm_s2).clamp(0.05, 0.95);

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
        //
        // The allocator's stack-width compensation (`1/√width`) rides along
        // here — a per-lane constant for the block, so a wide stack costs one
        // multiply per lane rather than anything in the frame loop.
        let mut pan_l = [0.0f32; N];
        let mut pan_r = [0.0f32; N];
        for v in 0..N {
            let (gl, gr) = voice_pan_gains(self.smooth.pan_current(v));
            pan_l[v] = gl * ctx.level_comp;
            pan_r[v] = gr * ctx.level_comp;
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
                        let (pwm_s1, pwm_s2) = self.smooth.tick_pwm(v, pwm_tgt[v]);
                        pw1[v] = (ctx.osc1_pw + pwm_s1).clamp(0.05, 0.95);
                        pw2[v] = (ctx.osc2_pw + pwm_s2).clamp(0.05, 0.95);
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
            level_comp: 1.0,
            matrix: m,
            mod_wheel: 0.0,
            pitch_wheel: 0.0,
        }
    }

    // ── Pan as a matrix destination (0260) ──────────────────────────────────

    /// Render one lane and return its `(L, R)` peaks.
    /// `pos` is the lane's place within its stack (ADR 0003) — the stereo fan's
    /// input, stamped by the allocator rather than derived from the lane index.
    fn lane_peaks(
        bank: &mut RenderBank,
        c: &BlockCtx,
        lane: usize,
        pos: f32,
        frames: usize,
    ) -> (f32, f32) {
        let mut note = [60u8; N];
        note[lane] = 60;
        let mut gate = [false; N];
        let mut active = [false; N];
        gate[lane] = true;
        active[lane] = true;
        let mut stack_pos = [0.0f32; N];
        stack_pos[lane] = pos;
        let mut l = vec![0.0; frames];
        let mut r = vec![0.0; frames];
        bank.render(
            c, &note, &gate, &mut active, &[1.0; N], &[0.0; N], &[0.0; N], &[0.0; N], &stack_pos,
            &mut l, &mut r,
        );
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

    /// The default `Spread → Pan` route fans a stack across the image: with
    /// Spread up, the lane at the bottom of its stack goes hard left and the one
    /// at the top hard right — the behaviour that used to be hard-wired DSP,
    /// now driven by the allocator's stack position (ADR 0003).
    #[test]
    fn default_route_places_stack_lanes_across_the_image() {
        let m = default_patch();
        let mut c = ctx(&m);
        c.spread = 1.0;

        let mut bank = fast_bank();
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        let (l0, r0) = lane_peaks(&mut bank, &c, 0, -1.0, 512);
        assert!(l0 > 0.0 && r0 < l0 * 1e-3, "stack bottom must sit hard left: {l0} vs {r0}");

        let mut bank = fast_bank();
        bank.trigger_lane(7, LfoShape::Sine, false, None);
        let (l7, r7) = lane_peaks(&mut bank, &c, 7, 1.0, 512);
        assert!(r7 > 0.0 && l7 < r7 * 1e-3, "stack top must sit hard right: {l7} vs {r7}");
    }

    /// Spread at 0 centres every lane, so the two channels are bit-identical —
    /// the parity condition, and what the unity-centre normalisation preserves.
    #[test]
    fn spread_zero_centres_every_lane() {
        let m = default_patch();
        let c = ctx(&m); // spread: 0.0
        let mut bank = fast_bank();
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        let (note, gate, mut active, vel, pres, rnd) = book();
        let mut l = vec![0.0; 512];
        let mut r = vec![0.0; 512];
        bank.render(&c, &note, &gate, &mut active, &vel, &pres, &rnd, &[0.0; N], &[0.0; N], &mut l, &mut r);
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
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        let (l, r) = lane_peaks(&mut bank, &c, 0, -1.0, 512);
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
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        let (l_right, r_right) = lane_peaks(&mut bank, &c, 0, 0.0, 2048);
        assert!(r_right > l_right * 10.0, "LFO +1 must pan right: {l_right} vs {r_right}");

        c.lfo2_val = -1.0;
        let mut bank = fast_bank();
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        let (l_left, r_left) = lane_peaks(&mut bank, &c, 0, 0.0, 2048);
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
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        let (note, gate, mut active, vel, pres, rnd) = book();
        let mut l = vec![0.0; 512];
        let mut r = vec![0.0; 512];
        bank.render(&c, &note, &gate, &mut active, &vel, &pres, &rnd, &[0.0; N], &[0.0; N], &mut l, &mut r);
        assert!(r.iter().all(|&s| s.abs() < 1e-3), "hard left should leave R quiet");

        // Slam to hard right for the next block.
        c.lfo2_val = 1.0;
        bank.render(&c, &note, &gate, &mut active, &vel, &pres, &rnd, &[0.0; N], &[0.0; N], &mut l, &mut r);
        let head = r[..64].iter().fold(0.0f32, |a, &s| a.max(s.abs()));
        let tail = r[448..].iter().fold(0.0f32, |a, &s| a.max(s.abs()));
        assert!(tail > 0.0, "the voice must arrive in R");
        assert!(
            head < tail * 0.75,
            "R should fill in across the block, not step: head {head} vs tail {tail}"
        );
    }

    // ── Per-oscillator PWM destinations (0261) ──────────────────────────────

    /// The oscillator's pulse is DC-blocked (`− (2w − 1)`), so width has to be
    /// read out of the *level*, not the mean: a ±1 pulse of duty `w` has
    /// RMS `2√(w(1−w))` — 1.0 at a square, falling monotonically as the width
    /// opens toward the rail. The filter is parked wide open so the shape
    /// survives to the output.
    fn pwm_ctx(m: &MatrixTable) -> BlockCtx<'_> {
        let mut c = ctx(m);
        c.osc1_wave = Waveform::Pulse;
        c.osc2_wave = Waveform::Pulse;
        c.cutoff = 18_000.0;
        c.resonance = 0.0;
        c.hpf_cutoff = HPF_OFF_HZ;
        c
    }

    /// RMS of the rendered lane, skipping the envelope attack. `osc` selects
    /// which oscillator is audible — the other is muted, so the level read is
    /// that oscillator's width alone.
    fn osc_rms(m: &MatrixTable, osc: usize, mod_wheel: f32) -> f32 {
        let mut c = pwm_ctx(m);
        c.mod_wheel = mod_wheel;
        c.osc1_level = if osc == 1 { 0.8 } else { 0.0 };
        c.osc2_level = if osc == 2 { 0.8 } else { 0.0 };
        let mut bank = fast_bank();
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        let (note, gate, mut active, vel, pres, rnd) = book();
        let frames = 4096;
        let mut l = vec![0.0; frames];
        let mut r = vec![0.0; frames];
        bank.render(
            &c, &note, &gate, &mut active, &vel, &pres, &rnd, &[0.0; N], &[0.0; N], &mut l, &mut r,
        );
        let tail = &l[512..];
        (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt()
    }

    fn pwm_route(dest: DestId, depth: f32) -> MatrixTable {
        let mut m = default_patch();
        m.slots[1].depth = 0.0; // silence the default vibrato: pitch must be steady
        m.slots[3] = MatrixSlot {
            source: SourceId::ModWheel,
            dest,
            depth,
            curve: Curve::Lin,
            scale_src: SourceId::None,
        };
        m
    }

    /// A route into `Osc1Pwm` moves osc 1's width and leaves osc 2 at its patch
    /// value — the whole point of the split.
    #[test]
    fn osc1_pwm_route_moves_only_osc1() {
        let m = pwm_route(DestId::Osc1Pwm, 0.6);
        let (base1, base2) = (osc_rms(&m, 1, 0.0), osc_rms(&m, 2, 0.0));
        let (mod1, mod2) = (osc_rms(&m, 1, 1.0), osc_rms(&m, 2, 1.0));
        assert!(
            mod1 < base1 * 0.9,
            "osc 1's width must open (level drops off the square): {base1} → {mod1}"
        );
        assert!(
            (mod2 - base2).abs() < base2 * 1e-3,
            "osc 2 must stay at its patch width: {base2} → {mod2}"
        );
    }

    /// The combined `Pwm` dest still moves both oscillators together — existing
    /// patches are unaffected by the split.
    #[test]
    fn combined_pwm_route_still_moves_both() {
        let m = pwm_route(DestId::Pwm, 0.6);
        let (base1, base2) = (osc_rms(&m, 1, 0.0), osc_rms(&m, 2, 0.0));
        let (mod1, mod2) = (osc_rms(&m, 1, 1.0), osc_rms(&m, 2, 1.0));
        assert!(
            mod1 < base1 * 0.9 && mod2 < base2 * 0.9,
            "both must move: {base1} → {mod1}, {base2} → {mod2}"
        );
        assert!(
            (mod1 - mod2).abs() < mod1 * 1e-3,
            "and to the same width: {mod1} vs {mod2}"
        );
    }

    /// `Pwm` and `Osc1Pwm` on the same patch **sum** on osc 1, while osc 2 sees
    /// the combined route alone.
    #[test]
    fn combined_and_per_osc_routes_sum_on_osc1() {
        let both = pwm_route(DestId::Pwm, 0.3);
        let mut summed = both;
        summed.slots[4] = MatrixSlot {
            source: SourceId::ModWheel,
            dest: DestId::Osc1Pwm,
            depth: 0.3,
            curve: Curve::Lin,
            scale_src: SourceId::None,
        };
        // Osc 1 under (Pwm 0.3 + Osc1Pwm 0.3) should match a single Pwm 0.6.
        let doubled = pwm_route(DestId::Pwm, 0.6);
        let one = osc_rms(&summed, 1, 1.0);
        let want_one = osc_rms(&doubled, 1, 1.0);
        assert!(
            (one - want_one).abs() < want_one * 1e-3,
            "osc 1 sums the two routes: {one} vs {want_one}"
        );
        // Osc 2 sees only the combined route.
        let two = osc_rms(&summed, 2, 1.0);
        let want_two = osc_rms(&both, 2, 1.0);
        assert!(
            (two - want_two).abs() < want_two * 1e-3,
            "osc 2 sees `Pwm` alone: {two} vs {want_two}"
        );
        assert!(two > one * 1.05, "and so sits at a narrower width: {two} vs {one}");
    }

    /// A route driving osc 1 to the width rail must not clip osc 2 — the clamp
    /// is per oscillator.
    #[test]
    fn width_clamp_is_per_oscillator() {
        // +0.5 on a 0.5 base ⇒ osc 1 pinned at the 0.95 rail, where the pulse's
        // RMS is 2√(0.95·0.05) ≈ 0.44 of the square's.
        let m = pwm_route(DestId::Osc1Pwm, 1.0);
        let (base1, railed) = (osc_rms(&m, 1, 0.0), osc_rms(&m, 1, 1.0));
        assert!(
            railed < base1 * 0.6 && railed > 0.0,
            "osc 1 should sit at the wide rail, not silent: {base1} → {railed}"
        );
        let (base2, mod2) = (osc_rms(&m, 2, 0.0), osc_rms(&m, 2, 1.0));
        assert!(
            (mod2 - base2).abs() < base2 * 1e-3,
            "osc 2 unclipped: {base2} → {mod2}"
        );
    }

    /// The `pwm_active` gate fires when *either* lane is live, and a patch with
    /// no PWM route at all stays on the block-constant path.
    #[test]
    fn pwm_gate_tracks_either_lane() {
        let mut s = MotionSmoother::new(48_000.0);
        assert!(!s.pwm_active(0, (0.0, 0.0)), "no route ⇒ block-constant widths");
        assert!(s.pwm_active(0, (0.4, 0.0)), "osc 1 route arms the gate");
        assert!(s.pwm_active(0, (0.0, 0.4)), "osc 2 route arms the gate");
        s.tick_pwm(0, (0.4, 0.0));
        assert!(s.pwm_active(0, (0.0, 0.0)), "residual state keeps it armed");
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
        bank.render(&ctx(&m), &note, &gate, &mut active, &[0.0; N], &[0.0; N], &[0.0; N], &[0.0; N], &[0.0; N], &mut l, &mut r);
        assert!(l.iter().chain(r.iter()).all(|&s| s == 0.0));
    }

    #[test]
    fn triggered_voice_renders_sound() {
        let m = default_patch();
        let mut bank = RenderBank::new(48_000.0, 1);
        // Fast attack so the VCA opens within the block.
        bank.set_envelopes((0.001, 0.2, 0.8, 0.2), AdsrShape::Linear, (0.001, 0.2, 0.8, 0.2), AdsrShape::Linear, 0.0);
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        let (note, gate, mut active, vel, pres, rnd) = book();
        let mut l = vec![0.0; 256];
        let mut r = vec![0.0; 256];
        bank.render(&ctx(&m), &note, &gate, &mut active, &vel, &pres, &rnd, &[0.0; N], &[0.0; N], &mut l, &mut r);
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
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        let (note, gate, mut active, vel, pres, rnd) = book();
        let mut l = vec![0.0; 512];
        let mut r = vec![0.0; 512];
        let mut c = ctx(&m);
        c.os = 1;
        bank.render(&c, &note, &gate, &mut active, &vel, &pres, &rnd, &[0.0; N], &[0.0; N], &mut l, &mut r);
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
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        let note = [60u8; N];
        let gate = [false; N]; // already released
        let mut active = [false; N];
        active[0] = true;
        let mut l = vec![0.0; 256];
        let mut r = vec![0.0; 256];
        bank.render(&ctx(&m), &note, &gate, &mut active, &[1.0; N], &[0.0; N], &[0.0; N], &[0.0; N], &[0.0; N], &mut l, &mut r);
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
            bank.trigger_lane(0, LfoShape::Sine, false, None);
            let (note, gate, mut active, vel, pres, rnd) = book();
            let mut c = ctx(&m);
            c.osc1_level = 0.0;
            c.osc2_level = 0.0;
            c.noise_level = 0.8;
            c.resonance = 0.6;
            c.drift_amount = drift;
            let mut l = vec![0.0; 512];
            let mut r = vec![0.0; 512];
            bank.render(&c, &note, &gate, &mut active, &vel, &pres, &rnd, &[0.0; N], &[0.0; N], &mut l, &mut r);
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

    // ── Envelope time-scale dests, cooked at note-on (0268) ─────────────────

    /// A matrix with one Mod Wheel → `dest` route at `depth`.
    fn wheel_route(dest: DestId, depth: f32) -> MatrixTable {
        let mut m = MatrixTable::default();
        m.slots[0] = MatrixSlot {
            source: SourceId::ModWheel,
            dest,
            depth,
            curve: Curve::Lin,
            scale_src: SourceId::None,
        };
        m
    }

    /// Level reached after `ticks` samples of a fresh attack on lane `v`'s
    /// env 2 — a longer attack means a lower level. `AdsrCore` exposes no
    /// getters, so the cooked times are measured behaviourally (as the drift
    /// trims are).
    fn attack_level(bank: &mut RenderBank, v: usize, ticks: usize) -> f32 {
        bank.env2[v].reset();
        let mut level = 0.0;
        for i in 0..ticks {
            level = bank.env2[v].tick(i == 0, true);
        }
        level
    }

    /// Render one 128-frame block for the lane-0 note of [`book`].
    fn render_block(bank: &mut RenderBank, c: &BlockCtx) {
        let (note, gate, mut active, vel, pres, rnd) = book();
        let mut l = vec![0.0; 128];
        let mut r = vec![0.0; 128];
        bank.render(
            c, &note, &gate, &mut active, &vel, &pres, &rnd, &[0.0; N], &[0.0; N], &mut l, &mut r,
        );
    }

    fn env_bank() -> RenderBank {
        let env = (0.05, 0.1, 0.5, 0.2);
        let mut bank = RenderBank::new(48_000.0, 3);
        bank.set_envelopes(env, AdsrShape::Linear, env, AdsrShape::Linear, 0.0);
        bank
    }

    #[test]
    fn env_scale_latches_at_note_on_and_holds_for_the_note() {
        let m = wheel_route(DestId::Env2Scale, 1.0);
        let mut bank = env_bank();
        // Nothing triggered yet: every lane sits at exactly unity.
        assert_eq!(bank.env_scale[1], [1.0; N]);
        let unscaled = attack_level(&mut bank, 0, 64);

        // Wheel up at the trigger → the 2× rail.
        let mut c = ctx(&m);
        c.mod_wheel = 1.0;
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        render_block(&mut bank, &c);
        assert!((bank.env_scale[1][0] - 2.0).abs() < 1e-6, "{}", bank.env_scale[1][0]);
        let scaled = attack_level(&mut bank, 0, 64);
        assert!(scaled < unscaled * 0.6, "2× attack must climb slower: {scaled} vs {unscaled}");
        // Env 1 has no route, and untriggered lanes are untouched.
        assert_eq!(bank.env_scale[0], [1.0; N]);
        assert_eq!(bank.env_scale[1][1], 1.0);

        // The wheel drops mid-note: the cooked scale holds — this dest does not
        // track, or a held note's decay would lurch as the source moved.
        c.mod_wheel = 0.0;
        render_block(&mut bank, &c);
        assert!((bank.env_scale[1][0] - 2.0).abs() < 1e-6);

        // The *next* note-on re-cooks it at the wheel's new position.
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        render_block(&mut bank, &c);
        assert!((bank.env_scale[1][0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn no_env_scale_route_renders_bit_identically() {
        let m = default_patch();
        let render = |wheel: f32| -> Vec<f32> {
            let mut bank = env_bank();
            bank.trigger_lane(0, LfoShape::Sine, false, None);
            let mut c = ctx(&m);
            c.mod_wheel = wheel;
            let (note, gate, mut active, vel, pres, rnd) = book();
            let mut l = vec![0.0; 512];
            let mut r = vec![0.0; 512];
            bank.render(
                &c, &note, &gate, &mut active, &vel, &pres, &rnd, &[0.0; N], &[0.0; N], &mut l,
                &mut r,
            );
            assert_eq!(bank.env_scale, [[1.0; N]; 2], "an unrouted dest must stay unity");
            assert_eq!(bank.env_sus_mod, [[0.0; N]; 2], "an unrouted dest must stay neutral");
            l
        };
        assert!(render(0.0).iter().any(|&s| s != 0.0), "the patch must sound");
        assert_eq!(render(0.0), render(1.0));
    }

    /// An envelope (or drift) param change mid-note re-pushes the patch to every
    /// lane. It must fold the lane's latched scale back in rather than snapping
    /// the ringing note to the patch times.
    #[test]
    fn envelope_param_change_mid_note_keeps_the_cooked_scale() {
        let m = wheel_route(DestId::Env2Scale, 1.0);
        let mut bank = env_bank();
        let mut c = ctx(&m);
        c.mod_wheel = 1.0;
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        render_block(&mut bank, &c);
        assert!((bank.env_scale[1][0] - 2.0).abs() < 1e-6);

        // Same envelope, freshly pushed (as a Decay tweak would).
        let env = (0.05, 0.1, 0.5, 0.2);
        bank.set_envelopes(env, AdsrShape::Linear, env, AdsrShape::Linear, 0.0);
        assert!((bank.env_scale[1][0] - 2.0).abs() < 1e-6);

        let mut plain = env_bank();
        let scaled = attack_level(&mut bank, 0, 64);
        let unscaled = attack_level(&mut plain, 0, 64);
        assert!(scaled < unscaled * 0.6, "re-push must keep the 2×: {scaled} vs {unscaled}");
    }

    /// Negative depth shortens by exactly as much as positive lengthens, and the
    /// exponent clamp holds the rails whatever the depth stack says.
    #[test]
    fn env_scale_rails_are_half_and_double() {
        let mut bank = env_bank();
        for (depth, want) in [(-1.0, 0.5), (1.0, 2.0), (0.5, 2.0f32.sqrt())] {
            let m = wheel_route(DestId::Env1Scale, depth);
            let mut c = ctx(&m);
            c.mod_wheel = 1.0;
            bank.trigger_lane(0, LfoShape::Sine, false, None);
            render_block(&mut bank, &c);
            assert!(
                (bank.env_scale[0][0] - want).abs() < 1e-6,
                "depth {depth} → {} (want {want})",
                bank.env_scale[0][0]
            );
        }
        // Two full-depth routes sum to 2 octaves and clamp back to the 2× rail.
        let mut m = wheel_route(DestId::Env1Scale, 1.0);
        m.slots[1] = m.slots[0];
        let mut c = ctx(&m);
        c.mod_wheel = 1.0;
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        render_block(&mut bank, &c);
        assert!((bank.env_scale[0][0] - 2.0).abs() < 1e-6);
    }

    /// Level a lane's env 2 settles at after a full attack + decay — i.e. its
    /// cooked sustain.
    fn sustain_level(bank: &mut RenderBank, v: usize) -> f32 {
        bank.env2[v].reset();
        let mut level = 0.0;
        // 0.05 s attack + 0.1 s decay at 48 kHz = 7200 samples; 20k is settled.
        for i in 0..20_000 {
            level = bank.env2[v].tick(i == 0, true);
        }
        level
    }

    #[test]
    fn env_sustain_dest_offsets_the_patch_level() {
        let mut plain = env_bank();
        // The patch's own sustain, for reference (env_bank cooks 0.5).
        let base = sustain_level(&mut plain, 0);
        assert!((base - 0.5).abs() < 1e-3, "patch sustain should be 0.5, got {base}");

        // Wheel up through a +0.25 route → 0.75.
        let m = wheel_route(DestId::Env2Sustain, 0.25);
        let mut bank = env_bank();
        let mut c = ctx(&m);
        c.mod_wheel = 1.0;
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        render_block(&mut bank, &c);
        assert!((bank.env_sus_mod[1][0] - 0.25).abs() < 1e-6);
        let lifted = sustain_level(&mut bank, 0);
        assert!((lifted - 0.75).abs() < 1e-3, "{lifted}");

        // Negative depth pulls it down; env 1 and untriggered lanes untouched.
        let m = wheel_route(DestId::Env2Sustain, -0.25);
        let mut bank = env_bank();
        let mut c = ctx(&m);
        c.mod_wheel = 1.0;
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        render_block(&mut bank, &c);
        assert!((sustain_level(&mut bank, 0) - 0.25).abs() < 1e-3);
        assert_eq!(bank.env_sus_mod[0], [0.0; N]);
        assert_eq!(bank.env_sus_mod[1][1], 0.0);
    }

    /// Additive, so a route can reach both rails from any patch value — the
    /// thing a multiplier cannot do — and the clamp holds them.
    #[test]
    fn env_sustain_reaches_both_rails_and_clamps() {
        for (depth, want) in [(1.0, 1.0), (-1.0, 0.0)] {
            let m = wheel_route(DestId::Env2Sustain, depth);
            let mut bank = env_bank();
            let mut c = ctx(&m);
            c.mod_wheel = 1.0;
            bank.trigger_lane(0, LfoShape::Sine, false, None);
            render_block(&mut bank, &c);
            let got = sustain_level(&mut bank, 0);
            assert!((got - want).abs() < 1e-3, "depth {depth} → {got} (want {want})");
        }
    }

    /// Latched, not tracked: sustain sets the held level *and* the decay rate,
    /// so a mid-note change would step a ringing note and bend a decay in
    /// flight. The next note-on re-cooks.
    #[test]
    fn env_sustain_latches_at_note_on() {
        let m = wheel_route(DestId::Env2Sustain, 0.25);
        let mut bank = env_bank();
        let mut c = ctx(&m);
        c.mod_wheel = 1.0;
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        render_block(&mut bank, &c);
        assert!((bank.env_sus_mod[1][0] - 0.25).abs() < 1e-6);

        c.mod_wheel = 0.0;
        render_block(&mut bank, &c);
        assert!((bank.env_sus_mod[1][0] - 0.25).abs() < 1e-6, "must hold for the note");

        bank.trigger_lane(0, LfoShape::Sine, false, None);
        render_block(&mut bank, &c);
        assert_eq!(bank.env_sus_mod[1][0], 0.0, "the next note-on re-cooks");
    }

    /// The drift trim is multiplicative on sustain and the matrix offset is
    /// additive; both must survive an envelope param re-push.
    #[test]
    fn env_sustain_offset_survives_a_param_change() {
        let m = wheel_route(DestId::Env2Sustain, 0.25);
        let mut bank = env_bank();
        let mut c = ctx(&m);
        c.mod_wheel = 1.0;
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        render_block(&mut bank, &c);

        let env = (0.05, 0.1, 0.5, 0.2);
        bank.set_envelopes(env, AdsrShape::Linear, env, AdsrShape::Linear, 0.0);
        assert!((bank.env_sus_mod[1][0] - 0.25).abs() < 1e-6);
        assert!((sustain_level(&mut bank, 0) - 0.75).abs() < 1e-3);
    }

    // ── Per-voice LFO 1 rate (0269) ─────────────────────────────────────────

    /// Phase advanced over 8 blocks, measured *after* the lane's first block so
    /// the carry-over total is already in force.
    fn lfo1_advance(m: &MatrixTable, wheel: f32) -> f32 {
        let mut bank = env_bank();
        let mut c = ctx(m);
        c.mod_wheel = wheel;
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        render_block(&mut bank, &c);
        let start = bank.lfo1[0].phase();
        for _ in 0..8 {
            render_block(&mut bank, &c);
        }
        bank.lfo1[0].phase() - start
    }

    #[test]
    fn lfo1_rate_dest_multiplies_the_resolved_hz() {
        // Depth 1 × gain 2 = 2 octaves = 4× the panel rate.
        let m = wheel_route(DestId::Lfo1Rate, 1.0);
        let plain = lfo1_advance(&m, 0.0);
        let fast = lfo1_advance(&m, 1.0);
        assert!(plain > 0.0, "the LFO must run at all");
        assert!((fast / plain - 4.0).abs() < 0.02, "wheel up → 4×: {fast} vs {plain}");
        // Negative depth is the same distance the other way (0.25×).
        let down = lfo1_advance(&wheel_route(DestId::Lfo1Rate, -1.0), 1.0);
        assert!((down / plain - 0.25).abs() < 0.02, "wheel up at −1 → 0.25×: {down}");
    }

    /// The route is a multiplier on whatever the panel resolved — which under
    /// tempo sync (0267) is the subdivision's Hz. Two octaves of amount on a
    /// synced rate is still a subdivision, which is why the mapping is `2^x`.
    #[test]
    fn lfo1_rate_multiplies_a_synced_rate_the_same_way() {
        let m = wheel_route(DestId::Lfo1Rate, 1.0);
        let advance_at = |hz: f32, wheel: f32| -> f32 {
            let mut bank = env_bank();
            let mut c = ctx(&m);
            c.lfo1_rate_hz = hz; // as `sync::lfo_rate_hz` would have resolved it
            c.mod_wheel = wheel;
            bank.trigger_lane(0, LfoShape::Sine, false, None);
            render_block(&mut bank, &c);
            let start = bank.lfo1[0].phase();
            for _ in 0..8 {
                render_block(&mut bank, &c);
            }
            bank.lfo1[0].phase() - start
        };
        // 1/8 at 120 BPM = 4 Hz; ×4 = 16 Hz = 1/32, still on the grid.
        let eighth = advance_at(4.0, 0.0);
        let scaled = advance_at(4.0, 1.0);
        let thirty_second = advance_at(16.0, 0.0);
        assert!((scaled - thirty_second).abs() < 1e-6, "{scaled} vs {thirty_second}");
        assert!((scaled / eighth - 4.0).abs() < 0.02);
    }

    /// A stolen lane must not run a block at the *previous* note's rate: the
    /// trigger re-rates the lane from its own block's total.
    #[test]
    fn a_fresh_note_does_not_inherit_the_stolen_notes_rate() {
        let m = wheel_route(DestId::Lfo1Rate, 1.0);
        let mut bank = env_bank();
        let mut c = ctx(&m);
        c.mod_wheel = 1.0;
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        render_block(&mut bank, &c);
        assert!((bank.lfo1_rate_mod[0] - 2.0).abs() < 1e-6);

        // Wheel down, new note: the lane is re-rated on the trigger block, so
        // the first block after it already runs at 1×.
        c.mod_wheel = 0.0;
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        render_block(&mut bank, &c);
        let start = bank.lfo1[0].phase();
        render_block(&mut bank, &c);
        let stepped = bank.lfo1[0].phase() - start;
        // One block at the panel's 5 Hz over the 1500 Hz control rate.
        let want = 5.0 / (48_000.0 / CONTROL_BLOCK as f32);
        assert!((stepped - want).abs() < 1e-6, "{stepped} vs {want}");
    }

    #[test]
    fn an_unrouted_lfo1_rate_stays_at_the_panel_hz() {
        let m = default_patch();
        let mut bank = env_bank();
        let mut c = ctx(&m);
        c.mod_wheel = 1.0;
        bank.trigger_lane(0, LfoShape::Sine, false, None);
        render_block(&mut bank, &c);
        assert_eq!(bank.lfo1_rate_mod, [0.0; N], "no route → no rate offset");
        assert_eq!(lfo_rate_scale(0.0), 1.0);
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
