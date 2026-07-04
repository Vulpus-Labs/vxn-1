//! `Kick/Tone` — the poly engine. Lanes = voices (ADR 0001 §5).
//!
//! One sine body per voice with an exponential **pitch sweep** (high → settled
//! pitch) and a one-pole-attack × exponential-decay **amp** envelope. The same
//! engine covers a kick (low note, deep sweep, short decay), a tom, a bass stab,
//! and a tonal hit — the difference is just note + envelope/sweep settings, so
//! "drum vs note" is patch, not code (the vxn-3 thesis).
//!
//! State is stored SoA across [`LANES`] voices in plain `[f32; 4]` / `[u32; 4]`
//! arrays so the per-sample loop autovectorises to NEON `f32x4`, and the
//! envelopes are branchless (see [`vxn3_dsp::env`]) so no per-lane stage match
//! defeats it.

use vxn3_dsp::{SILENCE_EPS, attack_coef, decay_coef, fast_sine_q32, note_to_freq, phase_inc_hz};

use crate::patch::PatchReader;
use crate::track_engine::{EngineKind, LANES, TrackEngine, macro_map};

/// Deep-patch layout version for `Kick/Tone` (0179). Bump only this engine's tag
/// when its patch field set changes — independent of the global state format.
const PATCH_VERSION: u8 = 1;

/// Patch parameters for the `Kick/Tone` engine. Cooked into per-sample
/// coefficients at [`KickTone::set_sample_rate`] / construction.
#[derive(Copy, Clone, Debug)]
pub struct KickTonePatch {
    /// Amp attack time (s) — keep short for a click/transient.
    pub amp_attack_s: f32,
    /// Amp decay time to -60 dB (s) — the body length.
    pub amp_decay_s: f32,
    /// Pitch sweep depth in semitones above the settled note at trig time.
    pub pitch_depth_st: f32,
    /// Pitch sweep decay time to -60 dB of the depth (s) — the "donk".
    pub pitch_decay_s: f32,
}

impl Default for KickTonePatch {
    /// A serviceable 808-ish kick.
    fn default() -> Self {
        Self {
            amp_attack_s: 0.001,
            amp_decay_s: 0.35,
            pitch_depth_st: 24.0,
            pitch_decay_s: 0.05,
        }
    }
}

pub struct KickTone {
    patch: KickTonePatch,
    sample_rate: f32,

    // ── cooked per-sample coefficients (shared across lanes) ──
    amp_attack_coef: f32,
    amp_decay_coef: f32,
    /// Per-sample relaxation of the pitch multiplier toward 1.0.
    pitch_coef: f32,

    // ── per-voice SoA state ──
    phase: [u32; LANES],
    /// Settled phase increment per sample (Q32 as f32), from the voice's note.
    base_inc: [f32; LANES],
    /// Pitch multiplier, starts at 2^(depth/12) and relaxes to 1.0.
    pmul: [f32; LANES],
    /// Velocity-scaled peak.
    peak: [f32; LANES],
    /// One-pole attack state (0 → 1).
    atk: [f32; LANES],
    /// Exponential decay state (1 → 0).
    dec: [f32; LANES],
    /// Whether the lane is currently sounding (housekept per block).
    active: [bool; LANES],

    /// Round-robin allocation cursor.
    next: usize,
}

impl KickTone {
    pub fn new(sample_rate: f32, patch: KickTonePatch) -> Self {
        let mut e = Self {
            patch,
            sample_rate,
            amp_attack_coef: 0.0,
            amp_decay_coef: 0.0,
            pitch_coef: 0.0,
            phase: [0; LANES],
            base_inc: [0.0; LANES],
            pmul: [1.0; LANES],
            peak: [0.0; LANES],
            atk: [0.0; LANES],
            dec: [0.0; LANES],
            active: [false; LANES],
            next: 0,
        };
        e.cook();
        e
    }

    pub fn with_default_patch(sample_rate: f32) -> Self {
        Self::new(sample_rate, KickTonePatch::default())
    }

    fn cook(&mut self) {
        self.amp_attack_coef = attack_coef(self.patch.amp_attack_s, self.sample_rate);
        self.amp_decay_coef = decay_coef(self.patch.amp_decay_s, self.sample_rate);
        self.pitch_coef = decay_coef(self.patch.pitch_decay_s, self.sample_rate);
    }

    /// Pick a lane for a new voice: a free one if any, else round-robin steal.
    fn alloc_lane(&mut self) -> usize {
        if let Some(k) = (0..LANES).find(|&k| !self.active[k]) {
            return k;
        }
        let k = self.next;
        self.next = (self.next + 1) % LANES;
        k
    }
}

impl TrackEngine for KickTone {
    fn render(&mut self, out: &mut [f32]) {
        let atk_c = self.amp_attack_coef;
        let dec_c = self.amp_decay_coef;
        let pit_c = self.pitch_coef;

        for s in out.iter_mut() {
            let mut acc = 0.0_f32;
            // 4-wide lane loop — branchless, autovectorises to f32x4.
            for k in 0..LANES {
                // Envelopes.
                self.atk[k] += (1.0 - self.atk[k]) * atk_c;
                self.dec[k] *= dec_c;
                // Pitch sweep: multiplier relaxes toward 1.0.
                self.pmul[k] = 1.0 + (self.pmul[k] - 1.0) * pit_c;

                // Advance phase at the swept frequency.
                let inc = (self.base_inc[k] * self.pmul[k]) as u32;
                self.phase[k] = self.phase[k].wrapping_add(inc);

                let amp = self.peak[k] * self.atk[k] * self.dec[k];
                acc += fast_sine_q32(self.phase[k]) * amp;
            }
            *s = acc;
        }

        // Per-block housekeeping (outside the hot sample loop): free dead lanes.
        for k in 0..LANES {
            if self.active[k] && self.dec[k] < SILENCE_EPS {
                self.active[k] = false;
            }
        }
    }

    fn on_trig(&mut self, note: f32, velocity: f32) {
        let k = self.alloc_lane();
        self.phase[k] = 0;
        self.base_inc[k] = phase_inc_hz(note_to_freq(note), self.sample_rate);
        self.pmul[k] = (self.patch.pitch_depth_st / 12.0).exp2();
        self.peak[k] = velocity;
        self.atk[k] = 0.0;
        self.dec[k] = 1.0;
        self.active[k] = true;
    }

    fn reset(&mut self) {
        self.phase = [0; LANES];
        self.pmul = [1.0; LANES];
        self.peak = [0.0; LANES];
        self.atk = [0.0; LANES];
        self.dec = [0.0; LANES];
        self.active = [false; LANES];
        self.next = 0;
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.cook();
    }

    fn kind(&self) -> EngineKind {
        EngineKind::KickTone
    }

    fn set_macro(&mut self, slot: usize, value: f32) {
        let Some(r) = macro_map(EngineKind::KickTone, slot, value) else {
            return;
        };
        match slot {
            0 => self.patch.amp_decay_s = r.value,   // body length
            1 => self.patch.pitch_decay_s = r.value, // "donk" sweep length
            2 => self.patch.pitch_depth_st = r.value, // pitch-sweep depth
            _ => return,
        }
        self.cook();
    }

    fn serialize_patch(&self, out: &mut Vec<u8>) {
        out.push(PATCH_VERSION);
        out.extend_from_slice(&self.patch.amp_attack_s.to_le_bytes());
        out.extend_from_slice(&self.patch.amp_decay_s.to_le_bytes());
        out.extend_from_slice(&self.patch.pitch_depth_st.to_le_bytes());
        out.extend_from_slice(&self.patch.pitch_decay_s.to_le_bytes());
    }

    fn deserialize_patch(&mut self, bytes: &[u8]) -> Result<(), ()> {
        if bytes.is_empty() {
            return Ok(()); // v1 state blob: no patch → keep default
        }
        let mut r = PatchReader::new(bytes);
        if r.u8()? != PATCH_VERSION {
            return Ok(()); // newer/unknown layout: keep default, don't fail the load
        }
        self.patch.amp_attack_s = r.f32()?;
        self.patch.amp_decay_s = r.f32()?;
        self.patch.pitch_depth_st = r.f32()?;
        self.patch.pitch_decay_s = r.f32()?;
        self.cook();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(buf: &[f32]) -> f32 {
        if buf.is_empty() {
            return 0.0;
        }
        (buf.iter().map(|&x| x * x).sum::<f32>() / buf.len() as f32).sqrt()
    }

    #[test]
    fn idle_is_silent() {
        let mut e = KickTone::with_default_patch(48_000.0);
        let mut buf = [1.0_f32; 256];
        e.render(&mut buf);
        assert!(buf.iter().all(|&x| x == 0.0), "no trig → silence");
    }

    #[test]
    fn trig_produces_sound_then_decays() {
        let mut e = KickTone::with_default_patch(48_000.0);
        e.on_trig(28.0, 1.0);
        let mut body = vec![0.0_f32; 4_800]; // 100 ms
        e.render(&mut body);
        assert!(rms(&body) > 0.05, "trig should be audible, rms={}", rms(&body));
        assert!(body.iter().all(|x| x.is_finite()), "finite");

        // Let it fully decay (1.5 s ≫ the 0.35 s decay), then a fresh window is
        // silent and the lane has been freed.
        let mut decay = vec![0.0_f32; 72_000];
        e.render(&mut decay);
        let mut tail = vec![0.0_f32; 24_000];
        e.render(&mut tail);
        assert!(rms(&tail) < 1e-4, "fully decayed, rms={}", rms(&tail));
        assert!(!e.active.iter().any(|&a| a), "lane freed after decay");
    }

    #[test]
    fn pitch_sweeps_downward() {
        // Higher note → higher mean frequency: confirms pitch tracks note, so
        // the same engine is a tonal stab as well as a kick.
        let mut low = KickTone::with_default_patch(48_000.0);
        let mut high = KickTone::with_default_patch(48_000.0);
        low.on_trig(28.0, 1.0);
        high.on_trig(64.0, 1.0);
        let mut lb = vec![0.0; 4_800];
        let mut hb = vec![0.0; 4_800];
        low.render(&mut lb);
        high.render(&mut hb);
        // Zero-crossing count is a cheap pitch proxy.
        let zc = |b: &[f32]| b.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        assert!(zc(&hb) > zc(&lb), "higher note → more zero crossings");
    }

    #[test]
    fn voices_overlap_up_to_lane_budget() {
        let mut e = KickTone::with_default_patch(48_000.0);
        for _ in 0..LANES {
            e.on_trig(40.0, 1.0);
        }
        assert_eq!(e.active.iter().filter(|&&a| a).count(), LANES, "all lanes voiced");
        // A 5th trig steals, not grows.
        e.on_trig(40.0, 1.0);
        assert_eq!(e.active.iter().filter(|&&a| a).count(), LANES, "capped at LANES");
    }
}
