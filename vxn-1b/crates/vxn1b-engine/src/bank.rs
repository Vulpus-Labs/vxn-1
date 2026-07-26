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
//! **Scope (this step).** Poly only (matches the 0198 allocator); unison/mono,
//! the per-voice component trims (E022), and the two scalar-kernel dests
//! (`HpfCutoff`, `CrossModAmount`) are deferred — all inert at the factory
//! default, so the parity gate is unaffected.

use vxn_dsp::{
    AdsrCore, AdsrShape, AdsrStage, CHANNELS_PER_LAYER, CONTROL_BLOCK, FilterMode, FilterSlope,
    LfoCore, LfoShape, NoiseColor, OtaLadderCoeffs, PolyHpf, PolyNoiseBank, PolyOscillator,
    PolyOtaLadder, Waveform, note_to_hz, one_pole_coeff, poly_ring_mod, poly_sub_square,
};

use crate::eval::{SourceInputs, eval_dests, eval_sources};
use crate::matrix::{DestId, MatrixTable, SourceId};
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
    lfo1_seed: u64,
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
            lfo1_seed: rng_seed,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.env1 = std::array::from_fn(|_| AdsrCore::new(sample_rate));
        self.env2 = std::array::from_fn(|_| AdsrCore::new(sample_rate));
        let control_rate = sample_rate / CONTROL_BLOCK as f32;
        let seed = self.lfo1_seed;
        self.lfo1 = std::array::from_fn(|i| LfoCore::new(control_rate, lfo1_seed(seed, i)));
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
    }

    /// Apply ADSR params to all lanes (called when an envelope param changed).
    /// Trims (E022) deferred — every lane gets identical params for now.
    pub fn set_envelopes(
        &mut self,
        env1: (f32, f32, f32, f32),
        env1_shape: AdsrShape,
        env2: (f32, f32, f32, f32),
        env2_shape: AdsrShape,
    ) {
        for e in &mut self.env1 {
            e.set_params(env1.0, env1.1, env1.2, env1.3);
            e.set_shape(env1_shape);
        }
        for e in &mut self.env2 {
            e.set_params(env2.0, env2.1, env2.2, env2.3);
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
        let mut pw1 = [0.5f32; N];
        let mut pw2 = [0.5f32; N];
        let mut amp_c = [AmpCoeffs::default(); N];
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

            let (s1, s2) = render::voice_pitches(
                &dests,
                ctx.cross_mod_type,
                ctx.base_semis,
                nf,
                ctx.osc1_semi,
                ctx.osc2_semi,
                0.0, // detune (unison deferred)
                self.osc1.drift_value[v],
                self.osc2.drift_value[v],
            );
            self.osc1.inc[v] = note_to_hz(s1) / ctx.os_sample_rate;
            self.osc2.inc[v] = note_to_hz(s2) / ctx.os_sample_rate;
            pw1[v] = render::voice_pw(&dests, ctx.osc1_pw);
            pw2[v] = render::voice_pw(&dests, ctx.osc2_pw);

            let cutoff_hz = render::voice_cutoff_hz(
                &dests,
                ctx.cutoff,
                self.osc1.drift_value[v],
                self.osc2.drift_value[v],
                0.0, // drift key-track coupling (trim deferred)
                0.0,
                0.0,
                ctx.drift_amount,
            );
            let resonance = render::voice_resonance(&dests, ctx.resonance);
            self.ladder.set_coeffs(
                v,
                OtaLadderCoeffs::new(cutoff_hz, ctx.os_sample_rate, resonance, ctx.drive),
            );

            amp_c[v] = amp_coeffs(ctx.matrix, &sources);
        }
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

        // Equal-sum pan (VXN1): pos in [-1,1], (gL,gR) sums to 2, so spread=0
        // collapses to identical L/R.
        let mut pan_l = [0.0f32; N];
        let mut pan_r = [0.0f32; N];
        for v in 0..N {
            let pos = pan_position(v) * ctx.spread;
            pan_l[v] = 1.0 - pos;
            pan_r[v] = 1.0 + pos;
        }

        let ring_on = ctx.ring_mode;
        let ring_gain = 10.0f32.powf(RING_DRIVE_DB / 20.0);
        let noise_on = ctx.noise_level != 0.0;
        let sub_on = ctx.sub_level != 0.0;
        let osc1_runs =
            ctx.sync || ctx.pm_index != 0.0 || ring_on || ctx.osc1_level != 0.0 || sub_on;
        let osc2_runs = ctx.sync || ctx.pm_index != 0.0 || ring_on || ctx.osc2_level != 0.0;

        let env_static = self.envelopes_static(&trig, active, gate);
        // VCA base constant across the block when envelopes are static.
        if env_static {
            for v in 0..N {
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
            // Per-frame VCA: tick envelopes, substitute fresh levels into the
            // factored Amp (step 2). Skipped when the block is envelope-static
            // (amp constant, computed above).
            if !env_static {
                for v in 0..N {
                    let t = trig[v] && base_i == 0;
                    let e1 = self.env1[v].tick(t, gate[v]);
                    let e2 = self.env2[v].tick(t, gate[v]);
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
                } else if ctx.pm_index != 0.0 {
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
        let coeff = slot.depth * gain * scale;
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

// `one_pole_coeff` is imported for the forthcoming LFO→Amp declick (block-rate
// one-pole on the non-env Amp part); referenced here to keep the import live
// until that path lands.
#[allow(dead_code)]
const _: fn(f32, f32) -> f32 = one_pole_coeff;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matrix::default_patch;

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
        bank.set_envelopes((0.001, 0.2, 0.8, 0.2), AdsrShape::Linear, (0.001, 0.2, 0.8, 0.2), AdsrShape::Linear);
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
        bank.set_envelopes((0.001, 0.2, 0.8, 0.2), AdsrShape::Linear, (0.005, 0.2, 0.8, 0.2), AdsrShape::Linear);
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
        bank.set_envelopes((0.001, 0.001, 0.0, 0.001), AdsrShape::Linear, (0.001, 0.001, 0.0, 0.001), AdsrShape::Linear);
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
}
