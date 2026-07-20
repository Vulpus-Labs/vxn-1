//! `Noise` — the noise-percussion family (ADR 0001 §5; flavour runtime 0180/0182).
//!
//! A white-noise burst plus an optional short tuned body. 0182 enriches it to reach
//! its `patches-drums` flavours (snare-noise, clap):
//!
//! - a **state-variable bandpass** (freq + Q) shapes the noise colour (the tone body
//!   stays unfiltered so a snare keeps its low thud);
//! - a **snap** onset transient — a bright, very fast broadband tick post-filter;
//! - a **multi-tap burst gate** — the noise envelope re-fires `taps` times at
//!   `tap-spacing`, which is what makes a clap a clap.
//!
//! The family adopts the flavour runtime (base vector + macro-binding table resolved
//! per trig; ADR 0005). 4-wide SoA voice state in plain arrays with a **branchless**
//! tap gate (compare→0/1 masks, no per-lane branch) so the lane loop stays vectorisable.

use vxn3_dsp::{SILENCE_EPS, decay_coef, fast_sine_q32, note_to_freq, phase_inc_hz};

use crate::flavour::{Binding, Curve, Flavour, ParamMeta};
use crate::track_engine::{EngineKind, LANES, MACRO_SLOTS, MacroUnit, TrackEngine};

/// The **Noise** family's parameter space (ADR 0005 §Family): index → metadata.
pub const P_NOISE_DECAY: usize = 0;
pub const P_TONE_DECAY: usize = 1;
pub const P_TONE_MIX: usize = 2;
pub const P_BAND_FREQ: usize = 3;
pub const P_BAND_Q: usize = 4;
pub const P_SNAP: usize = 5;
pub const P_TAP_COUNT: usize = 6;
pub const P_TAP_SPACING: usize = 7;
/// Noise param count `P`.
pub const NOISE_P: usize = 8;

/// Fixed snap-transient decay to -60 dB (s) — a fast bright tick.
const SNAP_DECAY_S: f32 = 0.002;

/// Per-param metadata for the Noise family — queryable on the main thread by the
/// flavour editor (0185) and value-text (0172).
pub static NOISE_PARAMS: [ParamMeta; NOISE_P] = [
    ParamMeta { name: "Decay", unit: MacroUnit::Seconds, min: 0.02, max: 0.5, default: 0.18 },
    ParamMeta { name: "Tone", unit: MacroUnit::Seconds, min: 0.02, max: 0.5, default: 0.12 },
    ParamMeta { name: "Mix", unit: MacroUnit::Percent, min: 0.0, max: 1.0, default: 0.35 },
    ParamMeta { name: "Band", unit: MacroUnit::Hertz, min: 200.0, max: 8000.0, default: 1500.0 },
    ParamMeta { name: "Q", unit: MacroUnit::Ratio, min: 0.5, max: 8.0, default: 1.0 },
    ParamMeta { name: "Snap", unit: MacroUnit::Percent, min: 0.0, max: 1.0, default: 0.0 },
    ParamMeta { name: "Taps", unit: MacroUnit::Ratio, min: 1.0, max: 4.0, default: 1.0 },
    ParamMeta { name: "Spacing", unit: MacroUnit::Seconds, min: 0.005, max: 0.05, default: 0.012 },
];

/// Build a Noise flavour from a full base vector + macro defaults, wiring the three
/// standard host-macro bindings (burst length / noise↔body mix / band brightness). The
/// single place base values live, so the 0187 TOML-bank move is mechanical.
fn noise_flavour(base: [f32; NOISE_P], macro_defaults: [f32; MACRO_SLOTS]) -> Flavour {
    Flavour {
        base: base.to_vec(),
        bindings: vec![
            Binding { slot: 0, param: P_NOISE_DECAY as u8, curve: Curve::Linear, depth: 0.32 },
            Binding { slot: 1, param: P_TONE_MIX as u8, curve: Curve::Linear, depth: 0.65 },
            Binding { slot: 2, param: P_BAND_FREQ as u8, curve: Curve::Linear, depth: 6500.0 },
        ],
        macro_defaults,
        macro_names: Default::default(),
    }
}

/// The default Noise flavour — a rounder, general-purpose snare: a little snap, single
/// tap, a touch more body than `Snare` (shown as "Noise", the family's neutral start).
pub fn noise_default_flavour() -> Flavour {
    noise_flavour([0.09, 0.16, 0.40, 900.0, 1.0, 0.3, 1.0, 0.012], [0.35, 0.2, 0.35])
}

// ── Authored Noise flavours (0182) ───────────────────────────────────────────────

/// Snare — 808 snare (patches-drums snare defaults): noise-decay 0.2 s, body-decay 0.15 s,
/// tone 0.5 (≈ mix 0.45 → 50/50 body↔noise), band 3 kHz, snap 0.5, one tap. The body tracks
/// the sequenced note, so the snare lane plays ~MIDI 54 (180 Hz) for the classic pitch.
pub fn flavour_snare() -> Flavour {
    noise_flavour([0.07, 0.15, 0.29, 1050.0, 1.2, 0.5, 1.0, 0.012], [0.4, 0.25, 0.3])
}

/// Clap — 808 clap (patches-drums clap defaults): mid band ~1.2 kHz, no tuned body, four
/// rapid bursts (`bursts` 4, `spread` 0.5 → ~12 ms tap spacing) → the clap "brrap".
pub fn flavour_clap() -> Flavour {
    noise_flavour([0.03, 0.02, 0.0, 550.0, 2.0, 0.15, 4.0, 0.012], [0.2, 0.0, 0.1])
}

/// The authored Noise flavours (name → flavour), for the editor / factory bank.
pub fn noise_flavours() -> [(&'static str, Flavour); 3] {
    [
        ("default", noise_default_flavour()),
        ("Snare", flavour_snare()),
        ("Clap", flavour_clap()),
    ]
}

/// Cooked / resolved effective params for the current trig.
#[derive(Copy, Clone, Debug)]
pub struct NoisePatch {
    /// Noise-burst decay to -60 dB (s).
    pub noise_decay_s: f32,
    /// Tuned-body decay to -60 dB (s).
    pub tone_decay_s: f32,
    /// Tuned-body mix 0..1 (0 = pure noise clap, ~0.4 = snare body).
    pub tone_mix: f32,
    /// Noise bandpass centre (Hz).
    pub band_freq: f32,
    /// Noise bandpass resonance (Q).
    pub band_q: f32,
    /// Onset snap-transient level 0..1 (0 = none).
    pub snap: f32,
    /// Number of noise bursts per hit (1 = single, 3-4 = clap).
    pub tap_count: f32,
    /// Interval between taps (s).
    pub tap_spacing_s: f32,
}

impl Default for NoisePatch {
    fn default() -> Self {
        Self {
            noise_decay_s: 0.18,
            tone_decay_s: 0.12,
            tone_mix: 0.35,
            band_freq: 1500.0,
            band_q: 1.0,
            snap: 0.0,
            tap_count: 1.0,
            tap_spacing_s: 0.012,
        }
    }
}

pub struct Noise {
    /// Resolved / cooked effective params for the current trig.
    patch: NoisePatch,
    /// Installed flavour (base + bindings + macro defaults); serialised as the patch.
    flavour: Flavour,
    /// Live macro values (`0..1`) — host performance state, not in the flavour.
    macros: [f32; MACRO_SLOTS],
    /// Resolved vector stale → recompute at the next trig.
    dirty: bool,
    sample_rate: f32,

    // ── cooked coefficients ──
    noise_decay: f32,
    tone_decay: f32,
    snap_coef: f32,
    /// TPT state-variable filter coefficients (bandpass on the noise sum).
    a1: f32,
    a2: f32,
    a3: f32,
    /// Rounded tap count and spacing in samples (cooked).
    tap_count_n: f32,
    tap_spacing_samps: f32,

    // ── engine-level filter state (mono, on the summed noise) ──
    ic1eq: f32,
    ic2eq: f32,
    /// Shared xorshift noise state.
    rng: u32,

    // ── per-voice SoA state ──
    noise_env: [f32; LANES],
    tone_env: [f32; LANES],
    snap_env: [f32; LANES],
    tone_phase: [u32; LANES],
    tone_inc: [u32; LANES],
    peak: [f32; LANES],
    /// Multi-tap gate: bursts still to fire, and the sample countdown to the next.
    tap_left: [f32; LANES],
    tap_timer: [f32; LANES],
    active: [bool; LANES],
    next: usize,
}

impl Noise {
    /// Build from a flavour; live macros seed from the flavour's shipped defaults.
    pub fn from_flavour(sample_rate: f32, flavour: Flavour) -> Self {
        let macros = flavour.macro_defaults;
        let mut e = Self {
            patch: NoisePatch::default(),
            flavour,
            macros,
            dirty: false,
            sample_rate,
            noise_decay: 0.0,
            tone_decay: 0.0,
            snap_coef: 0.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            tap_count_n: 1.0,
            tap_spacing_samps: 0.0,
            ic1eq: 0.0,
            ic2eq: 0.0,
            rng: 0x1234_5678,
            noise_env: [0.0; LANES],
            tone_env: [0.0; LANES],
            snap_env: [0.0; LANES],
            tone_phase: [0; LANES],
            tone_inc: [0; LANES],
            peak: [0.0; LANES],
            tap_left: [0.0; LANES],
            tap_timer: [0.0; LANES],
            active: [false; LANES],
            next: 0,
        };
        e.resolve_patch();
        e
    }

    pub fn with_default_patch(sample_rate: f32) -> Self {
        Self::from_flavour(sample_rate, noise_default_flavour())
    }

    /// Resolve the flavour + live macros into the effective patch and re-cook.
    /// Allocation-free (stack scratch); runs at a trig boundary, never per sample.
    fn resolve_patch(&mut self) {
        let mut r = [0.0_f32; NOISE_P];
        crate::flavour::resolve(&NOISE_PARAMS, &self.flavour.base, &self.flavour.bindings, &self.macros, &mut r);
        self.patch.noise_decay_s = r[P_NOISE_DECAY];
        self.patch.tone_decay_s = r[P_TONE_DECAY];
        self.patch.tone_mix = r[P_TONE_MIX];
        self.patch.band_freq = r[P_BAND_FREQ];
        self.patch.band_q = r[P_BAND_Q];
        self.patch.snap = r[P_SNAP];
        self.patch.tap_count = r[P_TAP_COUNT];
        self.patch.tap_spacing_s = r[P_TAP_SPACING];
        self.cook();
        self.dirty = false;
    }

    fn cook(&mut self) {
        self.noise_decay = decay_coef(self.patch.noise_decay_s, self.sample_rate);
        self.tone_decay = decay_coef(self.patch.tone_decay_s, self.sample_rate);
        self.snap_coef = decay_coef(SNAP_DECAY_S, self.sample_rate);
        // TPT state-variable filter (Cytomic): g = tan(π·fc/fs), k = 1/Q.
        let g = (std::f32::consts::PI * self.patch.band_freq / self.sample_rate).tan();
        let k = 1.0 / self.patch.band_q.max(0.05);
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
        self.tap_count_n = self.patch.tap_count.round().clamp(1.0, 4.0);
        self.tap_spacing_samps = (self.patch.tap_spacing_s * self.sample_rate).max(1.0);
    }

    /// One TPT-SVF bandpass sample on the (mono) noise sum. Updates filter state.
    #[inline]
    fn bandpass(&mut self, x: f32) -> f32 {
        let v3 = x - self.ic2eq;
        let v1 = self.a1 * self.ic1eq + self.a2 * v3;
        let v2 = self.ic2eq + self.a2 * self.ic1eq + self.a3 * v3;
        self.ic1eq = 2.0 * v1 - self.ic1eq;
        self.ic2eq = 2.0 * v2 - self.ic2eq;
        v1 // bandpass output
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

    fn alloc_lane(&mut self) -> usize {
        if let Some(k) = (0..LANES).find(|&k| !self.active[k]) {
            return k;
        }
        let k = self.next;
        self.next = (self.next + 1) % LANES;
        k
    }
}

impl TrackEngine for Noise {
    fn render(&mut self, out: &mut [f32]) {
        let nd = self.noise_decay;
        let td = self.tone_decay;
        let sc = self.snap_coef;
        let mix = self.patch.tone_mix;
        let spacing = self.tap_spacing_samps;

        for s in out.iter_mut() {
            let n = self.white();
            let mut noise_sum = 0.0_f32;
            let mut tone_sum = 0.0_f32;
            let mut snap_sum = 0.0_f32;
            for k in 0..LANES {
                // Multi-tap burst gate — branchless: re-seed the noise env to 1.0 when
                // the timer expires and taps remain (compares → 0.0/1.0 masks).
                let due = (self.tap_timer[k] <= 0.0) as i32 as f32;
                let has = (self.tap_left[k] > 0.5) as i32 as f32;
                let fire = due * has;
                self.noise_env[k] = self.noise_env[k] * (1.0 - fire) + fire;
                self.tap_left[k] -= fire;
                self.tap_timer[k] = fire * spacing + (1.0 - fire) * (self.tap_timer[k] - 1.0);

                // Envelopes.
                self.noise_env[k] *= nd;
                self.tone_env[k] *= td;
                self.snap_env[k] *= sc;
                self.tone_phase[k] = self.tone_phase[k].wrapping_add(self.tone_inc[k]);

                let tone = fast_sine_q32(self.tone_phase[k]) * self.tone_env[k];
                noise_sum += n * self.noise_env[k] * self.peak[k];
                tone_sum += tone * self.peak[k];
                snap_sum += n * self.snap_env[k] * self.peak[k];
            }
            // Bandpass shapes only the noise; the tuned body + bright snap pass through.
            let bp = self.bandpass(noise_sum);
            *s = bp * (1.0 - mix) + tone_sum * mix + snap_sum;
        }

        for k in 0..LANES {
            if self.active[k]
                && self.noise_env[k] < SILENCE_EPS
                && self.tone_env[k] < SILENCE_EPS
                && self.snap_env[k] < SILENCE_EPS
                && self.tap_left[k] < 0.5
            {
                self.active[k] = false;
            }
        }
    }

    fn on_trig(&mut self, note: f32, velocity: f32) {
        if self.dirty {
            self.resolve_patch();
        }
        let k = self.alloc_lane();
        self.noise_env[k] = 1.0;
        self.tone_env[k] = 1.0;
        self.snap_env[k] = self.patch.snap; // 0 when the flavour has no snap
        self.tone_phase[k] = 0;
        self.tone_inc[k] = phase_inc_hz(note_to_freq(note), self.sample_rate) as u32;
        self.peak[k] = velocity;
        self.tap_left[k] = self.tap_count_n - 1.0; // first burst is this trig
        self.tap_timer[k] = self.tap_spacing_samps;
        self.active[k] = true;
    }

    fn reset(&mut self) {
        self.noise_env = [0.0; LANES];
        self.tone_env = [0.0; LANES];
        self.snap_env = [0.0; LANES];
        self.peak = [0.0; LANES];
        self.tap_left = [0.0; LANES];
        self.tap_timer = [0.0; LANES];
        self.active = [false; LANES];
        self.ic1eq = 0.0;
        self.ic2eq = 0.0;
        self.next = 0;
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.cook();
    }

    fn kind(&self) -> EngineKind {
        EngineKind::Noise
    }

    fn set_macro(&mut self, slot: usize, value: f32) {
        if slot < MACRO_SLOTS && self.macros[slot] != value {
            self.macros[slot] = value;
            self.dirty = true;
        }
    }

    fn family_params(&self) -> &'static [ParamMeta] {
        &NOISE_PARAMS
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
        if let Some(flavour) = Flavour::deserialize(bytes, NOISE_P)? {
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

    fn render(flav: Flavour, note: f32, n: usize) -> Vec<f32> {
        let mut e = Noise::with_default_patch(48_000.0);
        e.apply_flavour(flav);
        let mut buf = vec![0.0_f32; n];
        e.on_trig(note, 1.0);
        e.render(&mut buf);
        buf
    }

    #[test]
    fn idle_is_silent() {
        let mut e = Noise::with_default_patch(48_000.0);
        let mut buf = [1.0_f32; 256];
        e.render(&mut buf);
        assert!(rms(&buf[64..]) < 1e-3, "idle → silence");
    }

    #[test]
    fn trig_produces_perc_then_decays() {
        let mut e = Noise::with_default_patch(48_000.0);
        e.on_trig(60.0, 1.0);
        let mut body = vec![0.0_f32; 2_400];
        e.render(&mut body);
        assert!(rms(&body) > 0.02, "perc hit audible, rms={}", rms(&body));
        assert!(body.iter().all(|x| x.is_finite()));

        let mut decay = vec![0.0_f32; 48_000]; // 1 s ≫ the decay
        e.render(&mut decay);
        let mut tail = vec![0.0_f32; 9_600];
        e.render(&mut tail);
        assert!(rms(&tail) < 1e-4, "fully decayed, rms={}", rms(&tail));
        assert!(!e.active.iter().any(|&a| a), "lane freed");
    }

    #[test]
    fn voices_are_independent() {
        let mut e = Noise::with_default_patch(48_000.0);
        for _ in 0..LANES {
            e.on_trig(60.0, 1.0);
        }
        assert_eq!(e.active.iter().filter(|&&a| a).count(), LANES);
    }

    // ── Enriched Noise family (0182) ─────────────────────────────────────────

    /// Snap adds bright onset energy. Two renders are deterministic and identical but
    /// for snap (shared `rng` advances the same), so `b − a` isolates the snap.
    #[test]
    fn snap_adds_onset_energy() {
        let no_snap = [0.15, 0.10, 0.4, 2000.0, 1.0, 0.0, 1.0, 0.012];
        let with_snap = [0.15, 0.10, 0.4, 2000.0, 1.0, 0.9, 1.0, 0.012];
        let a = render(noise_flavour(no_snap, [0.0; MACRO_SLOTS]), 60.0, 480);
        let b = render(noise_flavour(with_snap, [0.0; MACRO_SLOTS]), 60.0, 480);
        let diff: Vec<f32> = a.iter().zip(&b).map(|(x, y)| y - x).collect();
        let onset = rms(&diff[..96]); // 2 ms
        let tail = rms(&diff[192..]);
        assert!(onset > 1e-3, "snap added no onset energy: {onset}");
        assert!(onset > tail * 4.0, "snap not concentrated at onset: {onset} vs {tail}");
    }

    /// The multi-tap gate re-fires the noise burst: a 4-tap clap carries far more energy
    /// in the 25–45 ms window (where taps 3–4 land) than a single burst of the same decay.
    #[test]
    fn multitap_refires_the_burst() {
        let single = [0.06, 0.02, 0.0, 1000.0, 2.0, 0.0, 1.0, 0.011];
        let quad = [0.06, 0.02, 0.0, 1000.0, 2.0, 0.0, 4.0, 0.011];
        let a = render(noise_flavour(single, [0.0; MACRO_SLOTS]), 60.0, 2_400); // 50 ms
        let b = render(noise_flavour(quad, [0.0; MACRO_SLOTS]), 60.0, 2_400);
        // 25–45 ms window (samples 1200..2160): the single burst has long decayed here.
        let w = 1_200..2_160;
        let single_late = rms(&a[w.clone()]);
        let quad_late = rms(&b[w]);
        assert!(quad_late > single_late * 2.0, "taps did not refire: {single_late} vs {quad_late}");
    }

    /// The bandpass shapes noise colour: a high-centre flavour is HF-richer than a
    /// low-centre one (same everything else).
    #[test]
    fn bandpass_shapes_noise_colour() {
        let low = [0.2, 0.1, 0.0, 500.0, 1.5, 0.0, 1.0, 0.012];
        let high = [0.2, 0.1, 0.0, 6000.0, 1.5, 0.0, 1.0, 0.012];
        let lo = render(noise_flavour(low, [0.0; MACRO_SLOTS]), 60.0, 4_800);
        let hi = render(noise_flavour(high, [0.0; MACRO_SLOTS]), 60.0, 4_800);
        assert!(
            hf_fraction(&hi) > hf_fraction(&lo) * 1.3,
            "band centre did not shape colour: {} vs {}",
            hf_fraction(&lo),
            hf_fraction(&hi)
        );
    }

    /// Every authored flavour is audibly distinct (pairwise), via the registry.
    #[test]
    fn noise_flavours_are_distinct() {
        let flavs = noise_flavours();
        let rendered: Vec<Vec<f32>> = flavs.iter().map(|(_, f)| render(f.clone(), 60.0, 9_600)).collect();
        for i in 0..rendered.len() {
            for j in (i + 1)..rendered.len() {
                assert_ne!(rendered[i], rendered[j], "'{}' and '{}' identical", flavs[i].0, flavs[j].0);
            }
        }
    }

    #[test]
    fn family_params_are_queryable() {
        let e = Noise::with_default_patch(48_000.0);
        let p = e.family_params();
        assert_eq!(p.len(), NOISE_P);
        assert_eq!(p[P_BAND_FREQ].name, "Band");
        assert_eq!(p[P_TAP_COUNT].name, "Taps");
    }
}
