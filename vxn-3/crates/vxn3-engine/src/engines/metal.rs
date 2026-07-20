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
/// HP-filtered white-noise layer, blended against the metallic (modal+XOR) sum. This is the
/// `patches-drums` hi-hat/cymbal "air" — those voices are `metal·tone + hp_noise·(1-tone)`;
/// here `noise` is that `(1-tone)`. 0 = pure metal (the pre-0189 sound, bit-for-bit).
pub const P_NOISE: usize = 9;
/// Metal param count `P`.
pub const METAL_P: usize = 10;

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
    ParamMeta { name: "Noise", unit: MacroUnit::Percent, min: 0.0, max: 1.0, default: 0.0 },
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
        macro_names: Default::default(),
    }
}

/// The default Metal flavour — the pure-modal hat/cymbal body (XOR + shimmer off), so
/// it matches the pre-0183 character at the base.
pub fn metal_default_flavour() -> Flavour {
    metal_flavour([700.0, 0.9, 0.08, 0.4, 44.0, 0.4, 0.0, 6.0, 6500.0, 0.0], [0.4, 0.5, 0.5])
}

// ── Authored Metal flavours (0183) ───────────────────────────────────────────────
// Base = [body_hz, open_s, closed_s, excite, split, xor_mix, shimmer, rate_hz, bright_hz].
// Choke is by note vs `split`: play the "open" note (≥ split) for a ring, a note < split
// to choke it. So one flavour covers a closed *and* an open hit on the same body.

/// The one 808 hi-hat: a single point in Metal's config space that **both** the Closed Hat
/// and Open Hat voices use. The 808 relationship — closed and open are the *same* oscillator
/// bank, differing only in decay — is expressed as: identical base vector, with the sequenced
/// **note** selecting which decay sounds (`note < split` = closed 0.045 s, `note ≥ split` =
/// open ~0.65 s). The two library voices differ only in their default note; a **choke group**
/// (track routing) gives the mutual cut across the two tracks. Noise-dominant so it reads as
/// hiss not pitched metal: modal ring ~15 % (xor_mix 0.85), noise 0.78, HP 8 kHz.
///
/// `[base_hz, open_decay, closed_decay, excite, split, xor_mix, shimmer, rate, bright, noise]`
pub fn flavour_hat() -> Flavour {
    metal_flavour([900.0, 0.42, 0.045, 0.35, 44.0, 0.85, 0.0, 6.0, 8000.0, 0.78], [0.12, 0.5, 0.5])
}

/// Ride — tonal, sustained, gentle shimmer, more modal than square, defined bell.
pub fn flavour_ride() -> Flavour {
    metal_flavour([400.0, 1.25, 0.15, 0.2, 44.0, 0.2, 0.35, 5.0, 4500.0, 0.2], [0.5, 0.4, 0.35])
}

/// Crash — long bright wash: low body, half square / half modal, strong shimmer.
pub fn flavour_crash() -> Flavour {
    metal_flavour([300.0, 1.67, 0.2, 0.4, 44.0, 0.5, 0.55, 4.0, 7000.0, 0.4], [0.7, 0.6, 0.35])
}

/// The authored Metal flavours (name → flavour), for the editor / factory bank.
pub fn metal_flavours() -> [(&'static str, Flavour); 5] {
    [
        ("default", metal_default_flavour()),
        ("Closed Hat", flavour_hat()),
        ("Open Hat", flavour_hat()),
        ("Ride", flavour_ride()),
        ("Crash", flavour_crash()),
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
    /// XOR-path (and noise-path) highpass cutoff (Hz) — brightness.
    pub bright: f32,
    /// HP-white-noise blend `0..1` (0 = pure metal, 1 = pure HP noise). The hi-hat "air".
    pub noise: f32,
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
            noise: 0.0,
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

    // HP-white-noise layer (shares hp_coef cutoff; own filter state + PRNG).
    rng: u32,
    nhp_y: f32,
    nhp_x1: f32,

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
            out_gain: 0.4,
            xor_phase: [0; XOR_OSCS],
            xor_inc: [0; XOR_OSCS],
            xor_env: 0.0,
            hp_coef: 0.0,
            hp_y: 0.0,
            hp_x1: 0.0,
            rng: 0x2545_F491,
            nhp_y: 0.0,
            nhp_x1: 0.0,
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
        self.patch.noise = r[P_NOISE];
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

    /// xorshift32 white noise in `[-1, 1)` — the hi-hat/cymbal "air" source.
    #[inline]
    fn white(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as i32 as f32) * (1.0 / 2_147_483_648.0)
    }
}

impl TrackEngine for Metal {
    fn render(&mut self, out: &mut [f32]) {
        let d = self.cur_decay;
        let g = self.out_gain;
        let mix = self.patch.xor_mix;
        let shim = self.patch.shimmer;
        let noise = self.patch.noise;
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

            // HP-white-noise "air": envelope-tracked white noise, same choke decay + HP cutoff
            // as the XOR path. Blended against the metallic sum so the hi-hat/cymbal reaches
            // its `patches-drums` hiss (metal·(1-noise) + hp_noise·noise).
            // ×2.0 (not the XOR path's ×0.5): the HP strips ~⅔ of white-noise energy, and a
            // noise-dominant hat no longer has the modal ring to carry its level, so the noise
            // must be hotter to sit with the kick/snare.
            let nraw = self.white() * self.xor_env * 2.0;
            let nhy = hp * (self.nhp_y + nraw - self.nhp_x1);
            self.nhp_y = nhy;
            self.nhp_x1 = nraw;

            // Blend + shimmer tremolo. `mix = 0, noise = 0, shim = 0` ⇒ `modal * g` bit-for-bit.
            let metal_sum = modal * (1.0 - mix) + hy * mix;
            let mixed = metal_sum * (1.0 - noise) + nhy * noise;
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

    /// Cross-track choke (a closed hit cutting an open ring on another track): collapse the
    /// live decay to a fast ~5 ms release. The next `on_trig` restores the flavour's decay.
    fn choke(&mut self) {
        self.cur_decay = decay_coef(0.005, self.sample_rate);
    }

    fn reset(&mut self) {
        self.re = [0.0; METAL_MODES];
        self.im = [0.0; METAL_MODES];
        self.xor_env = 0.0;
        self.hp_y = 0.0;
        self.hp_x1 = 0.0;
        self.nhp_y = 0.0;
        self.nhp_x1 = 0.0;
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
        let modal = [1500.0, 0.8, 0.08, 0.6, 44.0, 0.0, 0.0, 6.0, 7000.0, 0.0];
        let metal = [1500.0, 0.8, 0.08, 0.6, 44.0, 0.85, 0.0, 6.0, 7000.0, 0.0];
        let m = render_open(metal_flavour(modal, [0.0; MACRO_SLOTS]), 4_800);
        let x = render_open(metal_flavour(metal, [0.0; MACRO_SLOTS]), 4_800);
        assert_ne!(m, x, "XOR mix changed nothing");
        assert!(hf_fraction(&x) > hf_fraction(&m) * 1.3, "XOR not brighter: {} vs {}", hf_fraction(&m), hf_fraction(&x));
    }

    /// The HP-noise layer adds broadband "air": blending it in keeps the voice audible and
    /// makes it HF-richer than the pure metallic ring (the `patches-drums` hi-hat hiss).
    #[test]
    fn noise_layer_adds_air() {
        let dry = [2000.0, 0.3, 0.08, 0.6, 44.0, 0.5, 0.0, 6.0, 8000.0, 0.0];
        let air = [2000.0, 0.3, 0.08, 0.6, 44.0, 0.5, 0.0, 6.0, 8000.0, 0.8];
        let d = render_open(metal_flavour(dry, [0.0; MACRO_SLOTS]), 4_800);
        let a = render_open(metal_flavour(air, [0.0; MACRO_SLOTS]), 4_800);
        assert_ne!(d, a, "noise layer changed nothing");
        assert!(rms(&a) > 0.01, "noise voice silent: {}", rms(&a));
        assert!(hf_fraction(&a) > hf_fraction(&d), "noise not brighter: {} vs {}", hf_fraction(&d), hf_fraction(&a));
    }

    /// The shimmer LFO amplitude-modulates the output: with shimmer on, the windowed
    /// amplitude envelope wobbles more than the smooth decay of the shimmer-off ring.
    #[test]
    fn shimmer_modulates_amplitude() {
        let flat = [1000.0, 2.5, 0.1, 0.6, 44.0, 0.0, 0.0, 6.0, 5000.0, 0.0];
        let wob = [1000.0, 2.5, 0.1, 0.6, 44.0, 0.0, 0.9, 8.0, 5000.0, 0.0];
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

    /// Authored flavours are distinct sounds. The Closed/Open Hat pair is deliberately **one**
    /// config point (the 808 shared bank) distinguished only by the played note, so equal
    /// flavours are skipped here — their note-selected distinction is checked below.
    #[test]
    fn metal_flavours_are_distinct() {
        let flavs = metal_flavours();
        let rendered: Vec<Vec<f32>> = flavs.iter().map(|(_, f)| render_open(f.clone(), 9_600)).collect();
        for i in 0..rendered.len() {
            for j in (i + 1)..rendered.len() {
                if flavs[i].1 == flavs[j].1 {
                    continue; // same config point (the hat) — distinct by note, not by config
                }
                assert_ne!(rendered[i], rendered[j], "'{}' and '{}' identical", flavs[i].0, flavs[j].0);
            }
        }
    }

    /// The single hat point renders a *closed* hat below `split` and an *open* hat at/above it:
    /// the note selects the decay, so one config yields two distinct sounds (the 808 relation).
    #[test]
    fn hat_note_selects_open_vs_closed() {
        let render_note = |note: f32| {
            let mut e = Metal::with_default_patch(48_000.0);
            e.apply_flavour(flavour_hat());
            let mut buf = vec![0.0_f32; 9_600];
            e.on_trig(note, 1.0);
            e.render(&mut buf);
            buf
        };
        let closed = render_note(CLOSED);
        let open = render_note(OPEN);
        assert_ne!(closed, open, "hat note did not select the decay");
        // Open rings well past the closed hat's short gate.
        assert!(rms(&open[4_800..]) > rms(&closed[4_800..]) * 2.0, "open not longer than closed");
    }

    /// `choke()` fast-releases a sounding ring (the cross-track cut): after a choke the tail
    /// collapses far below the pre-choke level, without a re-trig.
    #[test]
    fn choke_cuts_the_ring() {
        let mut e = Metal::with_default_patch(48_000.0);
        e.apply_flavour(flavour_hat());
        e.on_trig(OPEN, 1.0); // long open ring
        let mut before = vec![0.0_f32; 2_400];
        e.render(&mut before);
        e.choke();
        let mut after = vec![0.0_f32; 4_800];
        e.render(&mut after);
        assert!(
            rms(&after[2_400..]) < rms(&before) * 0.1,
            "choke did not cut the ring: pre {} vs post {}",
            rms(&before),
            rms(&after[2_400..])
        );
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
