//! `Metal` — the metallic-percussion family (ADR 0001 §5; flavour runtime 0180/0183).
//! Lanes = **modes**, not voices.
//!
//! Two tone sources, blended per flavour:
//!
//! - a **modal bank** — eight inharmonic partials of one body as damped complex
//!   rotations (the tonal ring: ride, cymbal);
//! - an **XOR-pair metallic** source — six square oscillators whose sign-parity is a
//!   cheap 808/909-style metallic buzz (the noisy hat character), high-passed for
//!   brightness.
//!
//! `on_trig` **injects excitation** into the persistent modal state *and* the XOR
//! envelope — a re-hit rides the still-decaying ring. **Choke** is damping: a "closed"
//! hit (note below `closed_below`) switches the shared decay short, collapsing an
//! in-progress "open" ring on the same body. A slow **shimmer** LFO tremolos the output
//! for cymbal/ride movement. The family is on the flavour runtime (ADR 0005): a base
//! vector + macro-binding table resolved per trig.
//!
//! The modal loop is SoA over `METAL_MODES` and autovectorises; the XOR oscillators +
//! LFO are cheap scalar work outside it.

use vxn3_dsp::{decay_coef, fast_sine_q32, phase_inc_hz};

use crate::flavour::{Binding, Curve, Flavour, ParamMeta};
use crate::track_engine::{EngineKind, MACRO_SLOTS, MacroUnit, TrackEngine};

/// Modal partial count (the engine-declared lane budget). Two NEON `f32x4`.
pub const METAL_MODES: usize = 8;
/// XOR metallic-source oscillator count (the 808/909 six-square trick).
const XOR_OSCS: usize = 6;

/// Inharmonic partial ratios (cymbal/hat-like) and their relative amplitudes.
const RATIOS: [f32; METAL_MODES] = [1.0, 1.31, 1.83, 2.41, 2.74, 3.23, 3.78, 4.21];
const AMPS: [f32; METAL_MODES] = [1.0, 0.85, 0.72, 0.62, 0.54, 0.46, 0.40, 0.34];
/// Inharmonic ratios for the six XOR square oscillators (metallic buzz).
const XOR_RATIOS: [f32; XOR_OSCS] = [1.0, 1.34, 1.79, 2.35, 2.98, 3.71];

/// The **Metal** family's parameter space (ADR 0005 §Family): index → metadata.
pub const P_BASE_HZ: usize = 0;
pub const P_OPEN_DECAY: usize = 1;
pub const P_CLOSED_DECAY: usize = 2;
pub const P_EXCITE: usize = 3;
pub const P_CLOSED_BELOW: usize = 4;
pub const P_XOR_MIX: usize = 5;
pub const P_SHIMMER: usize = 6;
pub const P_SHIMMER_RATE: usize = 7;
pub const P_BRIGHT: usize = 8;
/// Metal param count `P`.
pub const METAL_P: usize = 9;

/// Per-param metadata for the Metal family — queryable on the main thread.
pub static METAL_PARAMS: [ParamMeta; METAL_P] = [
    ParamMeta { name: "Body", unit: MacroUnit::Hertz, min: 200.0, max: 3000.0, default: 1200.0 },
    ParamMeta { name: "Open", unit: MacroUnit::Seconds, min: 0.1, max: 3.0, default: 1.1 },
    ParamMeta { name: "Closed", unit: MacroUnit::Seconds, min: 0.02, max: 0.4, default: 0.08 },
    ParamMeta { name: "Excite", unit: MacroUnit::Percent, min: 0.0, max: 1.0, default: 0.5 },
    ParamMeta { name: "Split", unit: MacroUnit::Ratio, min: 0.0, max: 127.0, default: 44.0 },
    ParamMeta { name: "Metal", unit: MacroUnit::Percent, min: 0.0, max: 1.0, default: 0.0 },
    ParamMeta { name: "Shimmer", unit: MacroUnit::Percent, min: 0.0, max: 1.0, default: 0.0 },
    ParamMeta { name: "Rate", unit: MacroUnit::Hertz, min: 1.0, max: 12.0, default: 6.0 },
    ParamMeta { name: "Bright", unit: MacroUnit::Hertz, min: 500.0, max: 10000.0, default: 5000.0 },
];

/// Build a Metal flavour from a full base vector + macro defaults, wiring the three
/// standard host-macro bindings (ring length / excitation / body pitch). One place the
/// base values live, so the 0187 TOML-bank move is mechanical.
fn metal_flavour(base: [f32; METAL_P], macro_defaults: [f32; MACRO_SLOTS]) -> Flavour {
    Flavour {
        base: base.to_vec(),
        bindings: vec![
            Binding { slot: 0, param: P_OPEN_DECAY as u8, curve: Curve::Linear, depth: 1.9 },
            Binding { slot: 1, param: P_EXCITE as u8, curve: Curve::Linear, depth: 0.5 },
            Binding { slot: 2, param: P_BASE_HZ as u8, curve: Curve::Linear, depth: 1800.0 },
        ],
        macro_defaults,
    }
}

/// The default Metal flavour — the pure-modal hat/cymbal body (XOR + shimmer off), so
/// it matches the pre-0183 character at the base.
pub fn metal_default_flavour() -> Flavour {
    metal_flavour([1200.0, 1.1, 0.08, 0.5, 44.0, 0.0, 0.0, 6.0, 5000.0], [0.5; MACRO_SLOTS])
}

// ── Authored Metal flavours (0183) ───────────────────────────────────────────────
// Base = [body_hz, open_s, closed_s, excite, split, xor_mix, shimmer, rate_hz, bright_hz].
// Choke is by note vs `split`: play the "open" note (≥ split) for a ring, a note < split
// to choke it. So one flavour covers a closed *and* an open hit on the same body.

/// Closed hat — high, tight, noisy (mostly XOR), short ring.
pub fn flavour_closed_hat() -> Flavour {
    metal_flavour([2000.0, 0.25, 0.05, 0.6, 44.0, 0.7, 0.0, 6.0, 7500.0], [0.1, 0.5, 0.6])
}

/// Open hat — same bright metal, long ring (choked by a closed hit on the body).
pub fn flavour_open_hat() -> Flavour {
    metal_flavour([2000.0, 0.9, 0.1, 0.6, 44.0, 0.7, 0.0, 6.0, 7500.0], [0.5, 0.5, 0.6])
}

/// Ride — tonal, sustained, gentle shimmer, more modal than noise.
pub fn flavour_ride() -> Flavour {
    metal_flavour([1000.0, 2.0, 0.15, 0.4, 44.0, 0.3, 0.4, 5.0, 4000.0], [0.7, 0.4, 0.4])
}

/// Cymbal — long bright wash, half metal / half modal, strong shimmer.
pub fn flavour_cymbal() -> Flavour {
    metal_flavour([800.0, 2.8, 0.2, 0.6, 44.0, 0.5, 0.5, 4.0, 6000.0], [0.8, 0.6, 0.35])
}

/// The authored Metal flavours (name → flavour), for the editor / factory bank.
pub fn metal_flavours() -> [(&'static str, Flavour); 5] {
    [
        ("default", metal_default_flavour()),
        ("closed-hat", flavour_closed_hat()),
        ("open-hat", flavour_open_hat()),
        ("ride", flavour_ride()),
        ("cymbal", flavour_cymbal()),
    ]
}

/// Resolved / cooked effective params for the current trig.
#[derive(Copy, Clone, Debug)]
pub struct MetalPatch {
    pub base_hz: f32,
    pub open_decay_s: f32,
    pub closed_decay_s: f32,
    pub excite: f32,
    /// Notes strictly below this trigger a closed (damped) hit; at/above it, open.
    pub closed_below: f32,
    /// Modal ↔ XOR-metallic blend (0 = pure modal, 1 = pure XOR).
    pub xor_mix: f32,
    /// Output shimmer-LFO depth (0 = none).
    pub shimmer: f32,
    /// Shimmer-LFO rate (Hz).
    pub shimmer_rate: f32,
    /// XOR-path highpass cutoff (Hz) — brightness.
    pub bright: f32,
}

impl Default for MetalPatch {
    fn default() -> Self {
        Self {
            base_hz: 1_200.0,
            open_decay_s: 1.1,
            closed_decay_s: 0.08,
            excite: 0.5,
            closed_below: 44.0,
            xor_mix: 0.0,
            shimmer: 0.0,
            shimmer_rate: 6.0,
            bright: 5_000.0,
        }
    }
}

pub struct Metal {
    patch: MetalPatch,
    flavour: Flavour,
    macros: [f32; MACRO_SLOTS],
    dirty: bool,
    sample_rate: f32,

    // Per-mode rotation (cooked from base × ratio).
    cos_w: [f32; METAL_MODES],
    sin_w: [f32; METAL_MODES],
    amp: [f32; METAL_MODES],
    // Per-mode complex state (the ring).
    re: [f32; METAL_MODES],
    im: [f32; METAL_MODES],

    // Cooked decay coefficients + the currently active one (switched by choke).
    open_decay: f32,
    closed_decay: f32,
    cur_decay: f32,
    out_gain: f32,

    // XOR metallic source.
    xor_phase: [u32; XOR_OSCS],
    xor_inc: [u32; XOR_OSCS],
    xor_env: f32,
    hp_coef: f32,
    hp_y: f32,
    hp_x1: f32,

    // Shimmer LFO.
    lfo_phase: u32,
    lfo_inc: u32,
}

impl Metal {
    pub fn from_flavour(sample_rate: f32, flavour: Flavour) -> Self {
        let macros = flavour.macro_defaults;
        let mut e = Self {
            patch: MetalPatch::default(),
            flavour,
            macros,
            dirty: false,
            sample_rate,
            cos_w: [0.0; METAL_MODES],
            sin_w: [0.0; METAL_MODES],
            amp: AMPS,
            re: [0.0; METAL_MODES],
            im: [0.0; METAL_MODES],
            open_decay: 0.0,
            closed_decay: 0.0,
            cur_decay: 0.0,
            out_gain: 0.25,
            xor_phase: [0; XOR_OSCS],
            xor_inc: [0; XOR_OSCS],
            xor_env: 0.0,
            hp_coef: 0.0,
            hp_y: 0.0,
            hp_x1: 0.0,
            lfo_phase: 0,
            lfo_inc: 0,
        };
        e.resolve_patch();
        e.cur_decay = e.open_decay;
        e
    }

    pub fn with_default_patch(sample_rate: f32) -> Self {
        Self::from_flavour(sample_rate, metal_default_flavour())
    }

    fn resolve_patch(&mut self) {
        let mut r = [0.0_f32; METAL_P];
        crate::flavour::resolve(&METAL_PARAMS, &self.flavour.base, &self.flavour.bindings, &self.macros, &mut r);
        self.patch.base_hz = r[P_BASE_HZ];
        self.patch.open_decay_s = r[P_OPEN_DECAY];
        self.patch.closed_decay_s = r[P_CLOSED_DECAY];
        self.patch.excite = r[P_EXCITE];
        self.patch.closed_below = r[P_CLOSED_BELOW];
        self.patch.xor_mix = r[P_XOR_MIX];
        self.patch.shimmer = r[P_SHIMMER];
        self.patch.shimmer_rate = r[P_SHIMMER_RATE];
        self.patch.bright = r[P_BRIGHT];
        self.cook();
        self.dirty = false;
    }

    fn cook(&mut self) {
        let two_pi = std::f32::consts::TAU;
        for (k, &ratio) in RATIOS.iter().enumerate() {
            let f = (self.patch.base_hz * ratio).min(self.sample_rate * 0.49);
            let w = two_pi * f / self.sample_rate;
            self.cos_w[k] = w.cos();
            self.sin_w[k] = w.sin();
        }
        for (j, &ratio) in XOR_RATIOS.iter().enumerate() {
            let f = (self.patch.base_hz * ratio).min(self.sample_rate * 0.49);
            self.xor_inc[j] = phase_inc_hz(f, self.sample_rate) as u32;
        }
        self.open_decay = decay_coef(self.patch.open_decay_s, self.sample_rate);
        self.closed_decay = decay_coef(self.patch.closed_decay_s, self.sample_rate);
        self.hp_coef = (-two_pi * self.patch.bright / self.sample_rate).exp();
        self.lfo_inc = phase_inc_hz(self.patch.shimmer_rate, self.sample_rate) as u32;
    }
}

impl TrackEngine for Metal {
    fn render(&mut self, out: &mut [f32]) {
        let d = self.cur_decay;
        let g = self.out_gain;
        let mix = self.patch.xor_mix;
        let shim = self.patch.shimmer;
        let hp = self.hp_coef;

        for s in out.iter_mut() {
            // Modal bank (SoA, autovectorises).
            let mut modal = 0.0_f32;
            for k in 0..METAL_MODES {
                let nr = d * (self.re[k] * self.cos_w[k] - self.im[k] * self.sin_w[k]);
                let ni = d * (self.re[k] * self.sin_w[k] + self.im[k] * self.cos_w[k]);
                self.re[k] = nr;
                self.im[k] = ni;
                modal += nr * self.amp[k];
            }

            // XOR metallic: sign-parity of six squares, enveloped + high-passed.
            let mut parity = 1.0_f32;
            for j in 0..XOR_OSCS {
                self.xor_phase[j] = self.xor_phase[j].wrapping_add(self.xor_inc[j]);
                let sgn = 1.0 - 2.0 * (self.xor_phase[j] >> 31) as f32; // ±1 square
                parity *= sgn;
            }
            self.xor_env *= d; // shares the choke decay
            let xraw = parity * self.xor_env * 0.5;
            let hy = hp * (self.hp_y + xraw - self.hp_x1);
            self.hp_y = hy;
            self.hp_x1 = xraw;

            // Blend + shimmer tremolo. `mix = 0, shim = 0` ⇒ `modal * g` bit-for-bit.
            let mixed = modal * (1.0 - mix) + hy * mix;
            self.lfo_phase = self.lfo_phase.wrapping_add(self.lfo_inc);
            let gmod = 1.0 + shim * 0.5 * fast_sine_q32(self.lfo_phase);
            *s = mixed * g * gmod;
        }
    }

    fn on_trig(&mut self, note: f32, velocity: f32) {
        if self.dirty {
            self.resolve_patch();
        }
        // Inject excitation into every mode (re-excites a live ring) + the XOR env.
        let e = velocity * self.patch.excite;
        for k in 0..METAL_MODES {
            self.re[k] += e * self.amp[k];
        }
        self.xor_env += e;
        // Choke: a closed hit collapses the (shared) decay → damps any open ring.
        self.cur_decay = if note < self.patch.closed_below {
            self.closed_decay
        } else {
            self.open_decay
        };
    }

    fn reset(&mut self) {
        self.re = [0.0; METAL_MODES];
        self.im = [0.0; METAL_MODES];
        self.xor_env = 0.0;
        self.hp_y = 0.0;
        self.hp_x1 = 0.0;
        self.cur_decay = self.open_decay;
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.cook();
        self.cur_decay = self.open_decay;
    }

    fn kind(&self) -> EngineKind {
        EngineKind::Metal
    }

    fn set_macro(&mut self, slot: usize, value: f32) {
        if slot < MACRO_SLOTS && self.macros[slot] != value {
            self.macros[slot] = value;
            self.dirty = true;
        }
    }

    fn family_params(&self) -> &'static [ParamMeta] {
        &METAL_PARAMS
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
        if let Some(flavour) = Flavour::deserialize(bytes, METAL_P)? {
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

    fn hf_fraction(buf: &[f32]) -> f32 {
        let hf: f32 = buf.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
        let total: f32 = buf.iter().map(|&x| x * x).sum::<f32>().max(1e-12);
        hf / total
    }

    const OPEN: f32 = 46.0;
    const CLOSED: f32 = 42.0;

    fn render_open(flav: Flavour, n: usize) -> Vec<f32> {
        let mut e = Metal::with_default_patch(48_000.0);
        e.apply_flavour(flav);
        let mut buf = vec![0.0_f32; n];
        e.on_trig(OPEN, 1.0);
        e.render(&mut buf);
        buf
    }

    #[test]
    fn open_hit_rings() {
        let mut e = Metal::with_default_patch(48_000.0);
        e.on_trig(OPEN, 1.0);
        let mut buf = vec![0.0_f32; 4_800];
        e.render(&mut buf);
        assert!(rms(&buf) > 0.01, "metallic ring audible, rms={}", rms(&buf));
        assert!(buf.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn rehit_reexcites_the_same_body() {
        let mut e = Metal::with_default_patch(48_000.0);
        e.on_trig(OPEN, 1.0);
        let mut warm = vec![0.0_f32; 9_600];
        e.render(&mut warm);
        let before = rms(&warm[warm.len() - 480..]);

        e.on_trig(OPEN, 1.0);
        let mut after = vec![0.0_f32; 480];
        e.render(&mut after);
        assert!(rms(&after) > before * 1.5, "re-hit should re-excite: before={before}, after={}", rms(&after));
    }

    #[test]
    fn closed_hit_chokes_open_ring_via_damping() {
        let mut a = Metal::with_default_patch(48_000.0);
        let mut b = Metal::with_default_patch(48_000.0);
        a.on_trig(OPEN, 1.0);
        b.on_trig(OPEN, 1.0);
        let mut a1 = vec![0.0_f32; 4_800];
        let mut b1 = vec![0.0_f32; 4_800];
        a.render(&mut a1);
        b.render(&mut b1);

        b.on_trig(CLOSED, 1.0); // choke

        let mut a2 = vec![0.0_f32; 9_600];
        let mut b2 = vec![0.0_f32; 9_600];
        a.render(&mut a2);
        b.render(&mut b2);
        let a_tail = rms(&a2[a2.len() - 2_400..]);
        let b_tail = rms(&b2[b2.len() - 2_400..]);
        assert!(b_tail < a_tail * 0.25, "closed hit should choke: open={a_tail}, choked={b_tail}");
    }

    // ── Enriched Metal family (0183) ─────────────────────────────────────────

    /// The XOR metallic source adds bright buzz: blending it in makes the tone
    /// HF-richer and audibly different from the pure modal ring.
    #[test]
    fn xor_source_adds_metallic_brightness() {
        let modal = [1500.0, 0.8, 0.08, 0.6, 44.0, 0.0, 0.0, 6.0, 7000.0];
        let metal = [1500.0, 0.8, 0.08, 0.6, 44.0, 0.85, 0.0, 6.0, 7000.0];
        let m = render_open(metal_flavour(modal, [0.0; MACRO_SLOTS]), 4_800);
        let x = render_open(metal_flavour(metal, [0.0; MACRO_SLOTS]), 4_800);
        assert_ne!(m, x, "XOR mix changed nothing");
        assert!(hf_fraction(&x) > hf_fraction(&m) * 1.3, "XOR not brighter: {} vs {}", hf_fraction(&m), hf_fraction(&x));
    }

    /// The shimmer LFO amplitude-modulates the output: with shimmer on, the windowed
    /// amplitude envelope wobbles more than the smooth decay of the shimmer-off ring.
    #[test]
    fn shimmer_modulates_amplitude() {
        let flat = [1000.0, 2.5, 0.1, 0.6, 44.0, 0.0, 0.0, 6.0, 5000.0];
        let wob = [1000.0, 2.5, 0.1, 0.6, 44.0, 0.0, 0.9, 8.0, 5000.0];
        let a = render_open(metal_flavour(flat, [0.0; MACRO_SLOTS]), 24_000);
        let b = render_open(metal_flavour(wob, [0.0; MACRO_SLOTS]), 24_000);
        assert_ne!(a, b, "shimmer changed nothing");
        // Coefficient of variation of short-window RMS over a steady mid-section.
        let cov = |buf: &[f32]| {
            let wins: Vec<f32> = buf[2_400..12_000].chunks(256).map(rms).collect();
            let mean = wins.iter().sum::<f32>() / wins.len() as f32;
            let var = wins.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / wins.len() as f32;
            var.sqrt() / mean.max(1e-9)
        };
        assert!(cov(&b) > cov(&a) * 1.5, "shimmer did not wobble amplitude: {} vs {}", cov(&a), cov(&b));
    }

    /// Every authored flavour is audibly distinct (pairwise), via the registry.
    #[test]
    fn metal_flavours_are_distinct() {
        let flavs = metal_flavours();
        let rendered: Vec<Vec<f32>> = flavs.iter().map(|(_, f)| render_open(f.clone(), 9_600)).collect();
        for i in 0..rendered.len() {
            for j in (i + 1)..rendered.len() {
                assert_ne!(rendered[i], rendered[j], "'{}' and '{}' identical", flavs[i].0, flavs[j].0);
            }
        }
    }

    #[test]
    fn family_params_are_queryable() {
        let e = Metal::with_default_patch(48_000.0);
        let p = e.family_params();
        assert_eq!(p.len(), METAL_P);
        assert_eq!(p[P_XOR_MIX].name, "Metal");
        assert_eq!(p[P_SHIMMER].name, "Shimmer");
    }
}
