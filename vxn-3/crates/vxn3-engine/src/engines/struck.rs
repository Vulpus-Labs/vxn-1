//! `Struck` — the BridgedT struck-resonator family (ADR 0001 §6; ADR 0005; ticket
//! 0184). The fourth and final voice family; the `patches-drums` "2" resonator school.
//!
//! A small bank of `STRUCK_MODES` inharmonic partials, **struck** by a shaped excitation
//! and ringing with:
//!
//! - **pitch-droop** — the whole body glides down after the hit (a real membrane's
//!   pitch envelope), a per-sample multiplier relaxing to 1 (as in `Kick/Tone`);
//! - **Q-as-decay** — one decay time sets the ring length;
//! - a **selectable excitation shape** — dirac / exp / half-sine / filtered-click —
//!   the transient that strikes the body.
//!
//! Pitch follows the sequenced **note** (× `Tune`), consistent with the vxn-3 thesis
//! (drum-vs-note is patch, pitch is the sequencer's). `Inharm` blends the partial
//! ratios from harmonic (drum: kick2/tom2/claves2) toward inharmonic (modal cymbal).
//!
//! On the flavour runtime (ADR 0005): base vector + macro-binding table resolved per
//! trig. The mode loop is SoA and autovectorises; the droop + shaped excitation are
//! cheap scalar work outside it.

use vxn3_dsp::{SILENCE_EPS, decay_coef, fast_sine_q32, note_to_freq, phase_inc_hz};

use crate::flavour::{Binding, Curve, Flavour, ParamMeta};
use crate::track_engine::{EngineKind, MACRO_SLOTS, MacroUnit, TrackEngine};

/// Struck modal partial count (one NEON `f32x4`).
pub const STRUCK_MODES: usize = 4;

/// Harmonic partial ratios (drum-like) and inharmonic ratios (cymbal-like); `Inharm`
/// blends between them. Static relative amplitudes tilt toward the fundamental.
const HARMONIC: [f32; STRUCK_MODES] = [1.0, 2.0, 3.0, 4.0];
const INHARMONIC: [f32; STRUCK_MODES] = [1.0, 1.31, 1.83, 2.41];
const MODE_AMP: [f32; STRUCK_MODES] = [1.0, 0.55, 0.35, 0.22];

/// The **Struck** family's parameter space (ADR 0005 §Family): index → metadata.
pub const P_DECAY: usize = 0;
pub const P_TUNE: usize = 1;
pub const P_DROOP_DEPTH: usize = 2;
pub const P_DROOP_TIME: usize = 3;
pub const P_EXC_SHAPE: usize = 4;
pub const P_EXC_TIME: usize = 5;
pub const P_EXCITE: usize = 6;
pub const P_INHARM: usize = 7;
/// Struck param count `P`.
pub const STRUCK_P: usize = 8;

/// Excitation shapes (the `ExcShape` param, rounded to an index).
const EXC_DIRAC: u8 = 0;
const EXC_EXP: u8 = 1;
const EXC_HALF_SINE: u8 = 2;
const EXC_FILTERED_CLICK: u8 = 3;
/// Fixed dirac / click decay to -60 dB (s) — a near-impulse.
const DIRAC_DECAY_S: f32 = 0.0005;

/// Per-param metadata for the Struck family — queryable on the main thread.
pub static STRUCK_PARAMS: [ParamMeta; STRUCK_P] = [
    ParamMeta { name: "Decay", unit: MacroUnit::Seconds, min: 0.05, max: 2.0, default: 0.4 },
    ParamMeta { name: "Tune", unit: MacroUnit::Ratio, min: 0.25, max: 4.0, default: 1.0 },
    ParamMeta { name: "Droop", unit: MacroUnit::Semitones, min: 0.0, max: 24.0, default: 6.0 },
    ParamMeta { name: "Glide", unit: MacroUnit::Seconds, min: 0.005, max: 0.2, default: 0.04 },
    ParamMeta { name: "Shape", unit: MacroUnit::Ratio, min: 0.0, max: 3.0, default: 1.0 },
    ParamMeta { name: "Strike", unit: MacroUnit::Seconds, min: 0.001, max: 0.05, default: 0.008 },
    ParamMeta { name: "Excite", unit: MacroUnit::Percent, min: 0.0, max: 1.0, default: 0.6 },
    ParamMeta { name: "Inharm", unit: MacroUnit::Percent, min: 0.0, max: 1.0, default: 0.0 },
];

/// Build a Struck flavour from a full base vector + macro defaults, wiring the three
/// standard host-macro bindings (ring length / excitation / brightness). One place the
/// base values live, so the 0187 TOML-bank move is mechanical.
fn struck_flavour(base: [f32; STRUCK_P], macro_defaults: [f32; MACRO_SLOTS]) -> Flavour {
    Flavour {
        base: base.to_vec(),
        bindings: vec![
            Binding { slot: 0, param: P_DECAY as u8, curve: Curve::Linear, depth: 1.6 },
            Binding { slot: 1, param: P_EXCITE as u8, curve: Curve::Linear, depth: 0.4 },
            Binding { slot: 2, param: P_INHARM as u8, curve: Curve::Linear, depth: 1.0 },
        ],
        macro_defaults,
    }
}

/// The default Struck flavour — a serviceable struck body (harmonic, exp excitation).
pub fn struck_default_flavour() -> Flavour {
    struck_flavour([0.4, 1.0, 6.0, 0.04, 1.0, 0.008, 0.6, 0.0], [0.5; MACRO_SLOTS])
}

// ── Authored Struck flavours (0184) ──────────────────────────────────────────────
// Base = [decay, tune, droop_st, glide_s, exc_shape, strike_s, excite, inharm]. Pitch
// comes from the sequenced note × Tune.

/// kick2 — deep droop, exp strike, long-ish harmonic body.
pub fn flavour_kick2() -> Flavour {
    struck_flavour([0.45, 1.0, 14.0, 0.055, EXC_EXP as f32, 0.010, 0.75, 0.0], [0.4, 0.6, 0.1])
}

/// tom2 — moderate droop, half-sine strike, a touch inharmonic.
pub fn flavour_tom2() -> Flavour {
    struck_flavour([0.55, 1.0, 8.0, 0.06, EXC_HALF_SINE as f32, 0.012, 0.6, 0.1], [0.5, 0.5, 0.15])
}

/// claves2 — short, high, near-no droop, a dirac click.
pub fn flavour_claves2() -> Flavour {
    struck_flavour([0.1, 2.0, 2.0, 0.012, EXC_DIRAC as f32, 0.002, 0.5, 0.2], [0.1, 0.4, 0.2])
}

/// modal cymbal — long, high, inharmonic, filtered-click strike, no droop.
pub fn flavour_modal_cymbal() -> Flavour {
    struck_flavour([1.8, 3.0, 0.0, 0.02, EXC_FILTERED_CLICK as f32, 0.02, 0.6, 0.9], [0.8, 0.5, 0.9])
}

/// The authored Struck flavours (name → flavour), for the editor / factory bank.
pub fn struck_flavours() -> [(&'static str, Flavour); 5] {
    [
        ("default", struck_default_flavour()),
        ("kick2", flavour_kick2()),
        ("tom2", flavour_tom2()),
        ("claves2", flavour_claves2()),
        ("modal-cymbal", flavour_modal_cymbal()),
    ]
}

/// Resolved / cooked effective params for the current trig.
#[derive(Copy, Clone, Debug)]
pub struct StruckPatch {
    pub decay_s: f32,
    pub tune: f32,
    pub droop_depth_st: f32,
    pub droop_time_s: f32,
    pub exc_shape: u8,
    pub exc_time_s: f32,
    pub excite: f32,
    pub inharm: f32,
}

impl Default for StruckPatch {
    fn default() -> Self {
        Self {
            decay_s: 0.4,
            tune: 1.0,
            droop_depth_st: 6.0,
            droop_time_s: 0.04,
            exc_shape: EXC_EXP,
            exc_time_s: 0.008,
            excite: 0.6,
            inharm: 0.0,
        }
    }
}

pub struct Struck {
    patch: StruckPatch,
    flavour: Flavour,
    macros: [f32; MACRO_SLOTS],
    dirty: bool,
    sample_rate: f32,

    // Cooked.
    ratio: [f32; STRUCK_MODES], // harmonic↔inharmonic blend
    decay_coef: f32,
    droop_coef: f32,
    exc_coef: f32,       // exp/click envelope decay
    exc_total_samps: f32, // half-sine window length
    out_gain: f32,

    // Per-mode state (SoA).
    phase: [u32; STRUCK_MODES],
    mode_inc: [f32; STRUCK_MODES], // per-trig, from note × tune × ratio
    amp_env: [f32; STRUCK_MODES],

    // Shared droop + excitation state.
    droop: f32,   // pitch multiplier relaxing to 1
    exc_env: f32, // excitation amplitude
    exc_pos: f32, // half-sine progress (samples)
    rng: u32,
    active: bool,
}

impl Struck {
    pub fn from_flavour(sample_rate: f32, flavour: Flavour) -> Self {
        let macros = flavour.macro_defaults;
        let mut e = Self {
            patch: StruckPatch::default(),
            flavour,
            macros,
            dirty: false,
            sample_rate,
            ratio: HARMONIC,
            decay_coef: 0.0,
            droop_coef: 0.0,
            exc_coef: 0.0,
            exc_total_samps: 1.0,
            out_gain: 0.3,
            phase: [0; STRUCK_MODES],
            mode_inc: [0.0; STRUCK_MODES],
            amp_env: [0.0; STRUCK_MODES],
            droop: 1.0,
            exc_env: 0.0,
            exc_pos: 0.0,
            rng: 0x9E37_79B9,
            active: false,
        };
        e.resolve_patch();
        e
    }

    pub fn with_default_patch(sample_rate: f32) -> Self {
        Self::from_flavour(sample_rate, struck_default_flavour())
    }

    fn resolve_patch(&mut self) {
        let mut r = [0.0_f32; STRUCK_P];
        crate::flavour::resolve(&STRUCK_PARAMS, &self.flavour.base, &self.flavour.bindings, &self.macros, &mut r);
        self.patch.decay_s = r[P_DECAY];
        self.patch.tune = r[P_TUNE];
        self.patch.droop_depth_st = r[P_DROOP_DEPTH];
        self.patch.droop_time_s = r[P_DROOP_TIME];
        self.patch.exc_shape = r[P_EXC_SHAPE].round().clamp(0.0, 3.0) as u8;
        self.patch.exc_time_s = r[P_EXC_TIME];
        self.patch.excite = r[P_EXCITE];
        self.patch.inharm = r[P_INHARM];
        self.cook();
        self.dirty = false;
    }

    fn cook(&mut self) {
        let inh = self.patch.inharm.clamp(0.0, 1.0);
        for (r, (&h, &i)) in self.ratio.iter_mut().zip(HARMONIC.iter().zip(INHARMONIC.iter())) {
            *r = h + inh * (i - h);
        }
        self.decay_coef = decay_coef(self.patch.decay_s, self.sample_rate);
        self.droop_coef = decay_coef(self.patch.droop_time_s, self.sample_rate);
        self.exc_total_samps = (self.patch.exc_time_s * self.sample_rate).max(1.0);
        let exc_time = match self.patch.exc_shape {
            EXC_DIRAC | EXC_FILTERED_CLICK => DIRAC_DECAY_S,
            _ => self.patch.exc_time_s,
        };
        self.exc_coef = decay_coef(exc_time, self.sample_rate);
    }

    #[inline]
    fn white(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as i32 as f32) * (1.0 / 2_147_483_648.0)
    }

    /// One excitation sample for the current shape. Advances the excitation state.
    #[inline]
    fn excitation(&mut self) -> f32 {
        match self.patch.exc_shape {
            EXC_HALF_SINE => {
                if self.exc_pos < self.exc_total_samps {
                    let v = (std::f32::consts::PI * self.exc_pos / self.exc_total_samps).sin();
                    self.exc_pos += 1.0;
                    v * self.exc_env
                } else {
                    0.0
                }
            }
            EXC_FILTERED_CLICK => {
                self.exc_env *= self.exc_coef;
                self.white() * self.exc_env
            }
            // dirac (fast decay) / exp (slower) share an exponential envelope.
            _ => {
                self.exc_env *= self.exc_coef;
                self.exc_env
            }
        }
    }
}

impl TrackEngine for Struck {
    fn render(&mut self, out: &mut [f32]) {
        let dec = self.decay_coef;
        let dcoef = self.droop_coef;
        let g = self.out_gain;

        for s in out.iter_mut() {
            // Pitch droop relaxes toward 1.0 (shared across modes).
            self.droop = 1.0 + (self.droop - 1.0) * dcoef;

            let mut acc = 0.0_f32;
            for k in 0..STRUCK_MODES {
                self.amp_env[k] *= dec;
                let inc = (self.mode_inc[k] * self.droop) as u32;
                self.phase[k] = self.phase[k].wrapping_add(inc);
                acc += fast_sine_q32(self.phase[k]) * self.amp_env[k];
            }
            // Shaped excitation transient (the "strike"), added on top of the ring.
            acc += self.excitation();
            *s = acc * g;
        }

        // Free the body once the ring and excitation have died.
        if self.active
            && self.exc_env < SILENCE_EPS
            && self.amp_env.iter().all(|&a| a < SILENCE_EPS)
        {
            self.active = false;
        }
    }

    fn on_trig(&mut self, note: f32, velocity: f32) {
        if self.dirty {
            self.resolve_patch();
        }
        let f0 = note_to_freq(note) * self.patch.tune;
        let e = velocity * self.patch.excite;
        #[allow(clippy::needless_range_loop)] // indexes several parallel per-mode arrays
        for k in 0..STRUCK_MODES {
            self.mode_inc[k] = phase_inc_hz(f0 * self.ratio[k], self.sample_rate);
            self.phase[k] = 0;
            self.amp_env[k] = e * MODE_AMP[k];
        }
        self.droop = (self.patch.droop_depth_st / 12.0).exp2();
        self.exc_env = e;
        self.exc_pos = 0.0;
        self.active = true;
    }

    fn reset(&mut self) {
        self.phase = [0; STRUCK_MODES];
        self.amp_env = [0.0; STRUCK_MODES];
        self.droop = 1.0;
        self.exc_env = 0.0;
        self.exc_pos = 0.0;
        self.active = false;
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.cook();
    }

    fn kind(&self) -> EngineKind {
        EngineKind::Struck
    }

    fn set_macro(&mut self, slot: usize, value: f32) {
        if slot < MACRO_SLOTS && self.macros[slot] != value {
            self.macros[slot] = value;
            self.dirty = true;
        }
    }

    fn family_params(&self) -> &'static [ParamMeta] {
        &STRUCK_PARAMS
    }

    fn apply_flavour(&mut self, flavour: Flavour) {
        self.flavour = flavour;
        self.dirty = true;
    }

    fn serialize_patch(&self, out: &mut Vec<u8>) {
        self.flavour.serialize(out);
    }

    fn deserialize_patch(&mut self, bytes: &[u8]) -> Result<(), ()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if let Some(flavour) = Flavour::deserialize(bytes, STRUCK_P)? {
            self.macros = flavour.macro_defaults;
            self.flavour = flavour;
            self.dirty = true;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(b: &[f32]) -> f32 {
        (b.iter().map(|&x| x * x).sum::<f32>() / b.len().max(1) as f32).sqrt()
    }

    fn zc(b: &[f32]) -> usize {
        b.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count()
    }

    fn render(flav: Flavour, note: f32, n: usize) -> Vec<f32> {
        let mut e = Struck::with_default_patch(48_000.0);
        e.apply_flavour(flav);
        let mut buf = vec![0.0_f32; n];
        e.on_trig(note, 1.0);
        e.render(&mut buf);
        buf
    }

    #[test]
    fn struck_hit_rings_then_decays() {
        let mut e = Struck::with_default_patch(48_000.0);
        e.on_trig(40.0, 1.0);
        let mut body = vec![0.0_f32; 4_800];
        e.render(&mut body);
        assert!(rms(&body) > 0.01, "struck body audible, rms={}", rms(&body));
        assert!(body.iter().all(|x| x.is_finite()));

        let mut decay = vec![0.0_f32; 144_000]; // 3 s ≫ decay
        e.render(&mut decay);
        let mut tail = vec![0.0_f32; 9_600];
        e.render(&mut tail);
        assert!(rms(&tail) < 1e-4, "fully decayed, rms={}", rms(&tail));
        assert!(!e.active, "body freed");
    }

    /// Pitch-droop: the fundamental starts high and glides down — more zero crossings in
    /// the first window than a later one (same excitation energy region).
    #[test]
    fn pitch_droops_downward() {
        // Strong droop, slow-ish glide, minimal excitation transient noise.
        let flav = struck_flavour([0.6, 1.0, 18.0, 0.08, EXC_EXP as f32, 0.006, 0.7, 0.0], [0.0; MACRO_SLOTS]);
        let buf = render(flav, 45.0, 9_600);
        let early = zc(&buf[240..1_200]); // skip the strike transient
        let late = zc(&buf[4_800..5_760]);
        assert!(early > late, "pitch did not droop: early zc {early} vs late {late}");
    }

    /// The excitation shape changes the strike: a dirac and a half-sine produce a
    /// different onset waveform.
    #[test]
    fn excitation_shape_changes_onset() {
        let dirac = struck_flavour([0.4, 1.0, 6.0, 0.04, EXC_DIRAC as f32, 0.012, 0.6, 0.0], [0.0; MACRO_SLOTS]);
        let hsine = struck_flavour([0.4, 1.0, 6.0, 0.04, EXC_HALF_SINE as f32, 0.012, 0.6, 0.0], [0.0; MACRO_SLOTS]);
        let a = render(dirac, 45.0, 480);
        let b = render(hsine, 45.0, 480);
        assert_ne!(a, b, "excitation shape changed nothing");
    }

    /// Q-as-decay: a short-decay flavour has far less late-window energy than a long one.
    #[test]
    fn decay_controls_ring_length() {
        let short = struck_flavour([0.08, 1.0, 6.0, 0.04, EXC_EXP as f32, 0.008, 0.6, 0.0], [0.0; MACRO_SLOTS]);
        let long = struck_flavour([1.5, 1.0, 6.0, 0.04, EXC_EXP as f32, 0.008, 0.6, 0.0], [0.0; MACRO_SLOTS]);
        let a = render(short, 45.0, 24_000);
        let b = render(long, 45.0, 24_000);
        let w = 12_000..24_000; // 250–500 ms
        assert!(rms(&b[w.clone()]) > rms(&a[w]) * 4.0, "decay did not control ring length");
    }

    #[test]
    fn struck_flavours_are_distinct() {
        let flavs = struck_flavours();
        // Play each near its natural register so claves2/cymbal don't alias to silence.
        let rendered: Vec<Vec<f32>> = flavs.iter().map(|(_, f)| render(f.clone(), 50.0, 9_600)).collect();
        for i in 0..rendered.len() {
            for j in (i + 1)..rendered.len() {
                assert_ne!(rendered[i], rendered[j], "'{}' and '{}' identical", flavs[i].0, flavs[j].0);
            }
        }
    }

    #[test]
    fn family_params_are_queryable() {
        let e = Struck::with_default_patch(48_000.0);
        let p = e.family_params();
        assert_eq!(p.len(), STRUCK_P);
        assert_eq!(p[P_DROOP_DEPTH].name, "Droop");
        assert_eq!(p[P_EXC_SHAPE].name, "Shape");
    }
}
