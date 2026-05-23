//! Structure-of-arrays voice bank: all 16 voices processed together so the
//! oscillator/filter/noise hot path vectorises across voices (see
//! `vxn_dsp::poly`). Envelopes stay scalar (one [`AdsrCore`] per voice) and
//! tick at the base rate; the oscillators and ladder run at the oversampled
//! rate.
//!
//! Modulation model (Jupiter-8-shaped, generalised): ENV-1, ENV-2, LFO,
//! velocity and key-follow are sources; pitch, cutoff, amp and PWM are
//! destinations. Pitch/cutoff/PWM are resolved once per control block; amp is
//! evaluated per base frame (held across oversampled subframes).

use vxn_dsp::{
    AdsrCore, AdsrShape, LadderCoeffs, LadderVariant, MAX_VOICES, NoiseColor, PolyLadder,
    PolyNoise, PolyOscillator, Waveform, fast_exp2, note_to_hz,
};

use crate::modmatrix::{ModDest, ModMatrix};

const N: usize = MAX_VOICES;

/// Control-block context shared by all voices.
pub struct BlockCtx {
    /// Oversampled sample rate (`base_rate * oversample`).
    pub os_sample_rate: f32,
    /// Oversampling factor (1, 2 or 4).
    pub os: usize,
    pub osc1_wave: Waveform,
    pub osc2_wave: Waveform,
    pub osc1_level: f32,
    pub osc2_level: f32,
    pub noise_level: f32,
    pub osc1_pw: f32,
    pub osc2_pw: f32,
    pub osc1_semi: f32,
    pub osc2_semi: f32,
    pub noise_color: NoiseColor,
    pub cutoff: f32,
    pub resonance: f32,
    pub drive: f32,
    pub variant: LadderVariant,
    pub base_semis: f32,
    pub lfo_val: f32,
    pub matrix: ModMatrix,
}

/// All 16 voices in structure-of-arrays form.
pub struct VoiceBank {
    osc1: PolyOscillator,
    osc2: PolyOscillator,
    noise: PolyNoise,
    ladder: PolyLadder,
    env1: [AdsrCore; N],
    env2: [AdsrCore; N],

    note: [u8; N],
    velocity: [f32; N],
    gate: [bool; N],
    active: [bool; N],
    trigger_pending: [bool; N],
    alloc_tick: [u64; N],
}

impl VoiceBank {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            osc1: PolyOscillator::new(),
            osc2: PolyOscillator::new(),
            noise: PolyNoise::new(0x9E37_79B9),
            ladder: PolyLadder::new(),
            env1: std::array::from_fn(|_| AdsrCore::new(sample_rate)),
            env2: std::array::from_fn(|_| AdsrCore::new(sample_rate)),
            note: [0; N],
            velocity: [0.0; N],
            gate: [false; N],
            active: [false; N],
            trigger_pending: [false; N],
            alloc_tick: [0; N],
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.env1 = std::array::from_fn(|_| AdsrCore::new(sample_rate));
        self.env2 = std::array::from_fn(|_| AdsrCore::new(sample_rate));
        self.reset_all();
    }

    pub fn reset_all(&mut self) {
        self.osc1 = PolyOscillator::new();
        self.osc2 = PolyOscillator::new();
        self.noise.reset();
        self.ladder.reset();
        for e in &mut self.env1 {
            e.reset();
        }
        for e in &mut self.env2 {
            e.reset();
        }
        self.active = [false; N];
        self.gate = [false; N];
    }

    pub fn active_count(&self) -> usize {
        self.active.iter().filter(|&&a| a).count()
    }

    /// Apply envelope params to every voice (called by the engine only when an
    /// envelope param changed).
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

    /// Start a note. Phases reset (DCO behaviour); envelopes retrigger from
    /// their current level.
    pub fn note_on(&mut self, note: u8, velocity: f32, alloc_tick: u64) {
        let v = self.allocate(note);
        self.note[v] = note;
        self.velocity[v] = velocity;
        self.gate[v] = true;
        self.active[v] = true;
        self.trigger_pending[v] = true;
        self.alloc_tick[v] = alloc_tick;
        self.osc1.reset(v);
        self.osc2.reset(v);
    }

    pub fn note_off(&mut self, note: u8) {
        for v in 0..N {
            if self.active[v] && self.gate[v] && self.note[v] == note {
                self.gate[v] = false;
            }
        }
    }

    pub fn all_notes_off(&mut self) {
        self.gate = [false; N];
    }

    /// Pick a voice: re-use one already playing this note, else a free voice,
    /// else steal the oldest.
    fn allocate(&self, note: u8) -> usize {
        if let Some(v) = (0..N).find(|&v| self.active[v] && self.note[v] == note) {
            return v;
        }
        if let Some(v) = (0..N).find(|&v| !self.active[v]) {
            return v;
        }
        let mut best = 0;
        let mut best_tick = u64::MAX;
        for v in 0..N {
            if self.alloc_tick[v] < best_tick {
                best_tick = self.alloc_tick[v];
                best = v;
            }
        }
        best
    }

    /// Render one control block into the oversampled mono buffer `out`
    /// (length = `base_frames * ctx.os`), accumulating all voices.
    pub fn render_block(&mut self, out: &mut [f32], ctx: &BlockCtx) {
        let os = ctx.os;
        let base_frames = out.len() / os;

        // ── Per-voice control-rate resolution (block start) ──
        let mut pw1 = [0.5f32; N];
        let mut pw2 = [0.5f32; N];
        for v in 0..N {
            let kf = key_follow(self.note[v]);
            let srcs = [
                self.env1[v].level,
                self.env2[v].level,
                ctx.lfo_val,
                self.velocity[v],
                kf,
            ];
            let pitch_mod = ctx.matrix.dest(ModDest::Pitch, &srcs);
            let cutoff_mod = ctx.matrix.dest(ModDest::Cutoff, &srcs);
            let pwm_mod = ctx.matrix.dest(ModDest::Pwm, &srcs);

            let nf = self.note[v] as f32;
            let s1 = ctx.base_semis + nf + ctx.osc1_semi + pitch_mod;
            let s2 = ctx.base_semis + nf + ctx.osc2_semi + pitch_mod;
            self.osc1.inc[v] = note_to_hz(s1) / ctx.os_sample_rate;
            self.osc2.inc[v] = note_to_hz(s2) / ctx.os_sample_rate;
            pw1[v] = (ctx.osc1_pw + pwm_mod).clamp(0.05, 0.95);
            pw2[v] = (ctx.osc2_pw + pwm_mod).clamp(0.05, 0.95);

            let cutoff_hz = ctx.cutoff * fast_exp2(cutoff_mod / 12.0);
            self.ladder.set_coeffs(
                v,
                LadderCoeffs::new(
                    cutoff_hz,
                    ctx.os_sample_rate,
                    ctx.resonance,
                    ctx.drive,
                    ctx.variant,
                ),
            );
        }
        // Ramp the ladder coefficients across this block's `base_frames * os`
        // samples so block-rate cutoff/LFO/envelope steps become a smooth
        // piecewise-linear coefficient trajectory (no zipper / staircase).
        self.ladder.prepare_ramp(base_frames * os);

        let mut trig = [false; N];
        trig.iter_mut()
            .zip(self.trigger_pending.iter_mut())
            .for_each(|(t, p)| *t = std::mem::take(p));

        // Scratch lane buffers.
        let mut o1 = [0.0f32; N];
        let mut o2 = [0.0f32; N];
        let mut nz = [0.0f32; N];
        let mut mix = [0.0f32; N];
        let mut filt = [0.0f32; N];
        let mut amp = [0.0f32; N];

        for base_i in 0..base_frames {
            // Envelopes + amp (base rate, scalar; gated to 0 for inactive voices).
            for v in 0..N {
                let t = trig[v] && base_i == 0;
                let e1 = self.env1[v].tick(t, self.gate[v]);
                let e2 = self.env2[v].tick(t, self.gate[v]);
                amp[v] = if self.active[v] {
                    let kf = key_follow(self.note[v]);
                    ctx.matrix
                        .dest(ModDest::Amp, &[e1, e2, ctx.lfo_val, self.velocity[v], kf])
                        .max(0.0)
                } else {
                    0.0
                };
            }

            let frame = base_i * os;
            for k in 0..os {
                self.osc1.process(ctx.osc1_wave, &pw1, &mut o1);
                self.osc2.process(ctx.osc2_wave, &pw2, &mut o2);
                self.noise.process(ctx.noise_color, &mut nz);
                for v in 0..N {
                    mix[v] =
                        o1[v] * ctx.osc1_level + o2[v] * ctx.osc2_level + nz[v] * ctx.noise_level;
                }
                self.ladder.process(&mix, &mut filt);
                let mut sum = 0.0;
                for v in 0..N {
                    sum += filt[v] * amp[v];
                }
                out[frame + k] += sum;
            }

            // Free voices whose envelopes have fully released.
            for v in 0..N {
                if self.active[v]
                    && !self.gate[v]
                    && self.env1[v].is_idle()
                    && self.env2[v].is_idle()
                {
                    self.active[v] = false;
                }
            }
        }
    }
}

/// Key-follow source value: octaves relative to middle C (note 60).
#[inline]
fn key_follow(note: u8) -> f32 {
    (note as f32 - 60.0) / 12.0
}
