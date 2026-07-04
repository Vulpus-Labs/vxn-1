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

use crate::flavour::{Binding, Curve, Flavour, ParamMeta};
use crate::track_engine::{EngineKind, LANES, MACRO_SLOTS, MacroUnit, TrackEngine};

/// The **Driven** family's parameter space (ADR 0005 §Family): index → metadata. A
/// flavour's base vector and the resolved per-trig vector are addressed by these ids.
pub const P_AMP_ATTACK: usize = 0;
pub const P_AMP_DECAY: usize = 1;
pub const P_PITCH_DEPTH: usize = 2;
pub const P_PITCH_DECAY: usize = 3;
/// Driven param count `P`.
pub const DRIVEN_P: usize = 4;

/// Per-param metadata for the Driven family — queryable on the main thread by the
/// flavour editor (0185) and value-text (0172). Ranges track the old 0170 macro map.
pub static DRIVEN_PARAMS: [ParamMeta; DRIVEN_P] = [
    ParamMeta { name: "Attack", unit: MacroUnit::Seconds, min: 0.0001, max: 0.05, default: 0.001 },
    ParamMeta { name: "Decay", unit: MacroUnit::Seconds, min: 0.05, max: 1.5, default: 0.35 },
    ParamMeta { name: "Depth", unit: MacroUnit::Semitones, min: 0.0, max: 48.0, default: 24.0 },
    ParamMeta { name: "Donk", unit: MacroUnit::Seconds, min: 0.005, max: 0.2, default: 0.05 },
];

/// The default Driven flavour — a serviceable 808-ish kick, with the three host macros
/// bound to decay / donk / pitch-depth. This is the 0170 slot meaning re-expressed as
/// **editable additive bindings from base** (ADR 0005 replaces the fixed per-engine
/// map). At neutral macros (0.5) it reproduces a usable kick; a macro at 0 gives the
/// base value, at 1 gives base + depth (clamped).
pub fn driven_default_flavour() -> Flavour {
    Flavour {
        base: vec![0.001, 0.35, 24.0, 0.05],
        bindings: vec![
            Binding { slot: 0, param: P_AMP_DECAY as u8, curve: Curve::Linear, depth: 0.65 },
            Binding { slot: 1, param: P_PITCH_DECAY as u8, curve: Curve::Linear, depth: 0.10 },
            Binding { slot: 2, param: P_PITCH_DEPTH as u8, curve: Curve::Linear, depth: 12.0 },
        ],
        macro_defaults: [0.5; MACRO_SLOTS],
    }
}

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
    /// Resolved / cooked effective params for the current trig — filled from the
    /// flavour + live macros by [`KickTone::resolve_patch`].
    patch: KickTonePatch,
    /// The installed flavour: base vector + macro-binding table + shipped macro
    /// defaults (ADR 0005). Serialised as the deep patch (0179).
    flavour: Flavour,
    /// Live macro values (`0..1`) — performance/automation state, **not** part of the
    /// flavour. Driven by the host macro slots via [`TrackEngine::set_macro`].
    macros: [f32; MACRO_SLOTS],
    /// The resolved vector is stale (a flavour or macro changed) → recompute at the
    /// next trig so a sounding voice never glitches mid-decay.
    dirty: bool,
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
    /// Build from a flavour; live macros seed from the flavour's shipped defaults.
    pub fn from_flavour(sample_rate: f32, flavour: Flavour) -> Self {
        let macros = flavour.macro_defaults;
        let mut e = Self {
            patch: KickTonePatch::default(),
            flavour,
            macros,
            dirty: false,
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
        e.resolve_patch(); // fill `patch` from flavour + macros, then cook
        e
    }

    pub fn with_default_patch(sample_rate: f32) -> Self {
        Self::from_flavour(sample_rate, driven_default_flavour())
    }

    /// Resolve the flavour + live macros into the effective [`KickTonePatch`]
    /// (additive-from-base, clamped) and re-cook coefficients. Allocation-free — the
    /// resolved vector is a stack array. Runs at a trig boundary, never per sample.
    fn resolve_patch(&mut self) {
        let mut r = [0.0_f32; DRIVEN_P];
        crate::flavour::resolve(
            &DRIVEN_PARAMS,
            &self.flavour.base,
            &self.flavour.bindings,
            &self.macros,
            &mut r,
        );
        self.patch.amp_attack_s = r[P_AMP_ATTACK];
        self.patch.amp_decay_s = r[P_AMP_DECAY];
        self.patch.pitch_depth_st = r[P_PITCH_DEPTH];
        self.patch.pitch_decay_s = r[P_PITCH_DECAY];
        self.cook();
        self.dirty = false;
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
        // A flavour/macro change re-resolves here, at the trig boundary — so already
        // sounding voices keep their coefficients and don't glitch (ADR 0005).
        if self.dirty {
            self.resolve_patch();
        }
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
        // Update the live macro value (performance state) and mark the resolved vector
        // stale; it re-resolves at the next trig via the flavour binding table.
        if slot < MACRO_SLOTS && self.macros[slot] != value {
            self.macros[slot] = value;
            self.dirty = true;
        }
    }

    fn family_params(&self) -> &'static [ParamMeta] {
        &DRIVEN_PARAMS
    }

    fn apply_flavour(&mut self, flavour: Flavour) {
        // Keep the live macro values (performance state); only the base + bindings +
        // shipped defaults change. Re-resolve at the next trig.
        self.flavour = flavour;
        self.dirty = true;
    }

    fn serialize_patch(&self, out: &mut Vec<u8>) {
        // The deep patch *is* the flavour (ADR 0005): base vector + binding table +
        // shipped macro defaults. Live macro values are host state, not serialised here.
        self.flavour.serialize(out);
    }

    fn deserialize_patch(&mut self, bytes: &[u8]) -> Result<(), ()> {
        if bytes.is_empty() {
            return Ok(()); // v1 state blob / no patch → keep default flavour
        }
        // `?` rejects a truncated patch; `Ok(None)` (version/shape mismatch) keeps the
        // default flavour rather than failing the whole state load (0179 contract).
        if let Some(flavour) = Flavour::deserialize(bytes, DRIVEN_P)? {
            self.macros = flavour.macro_defaults; // restore performance starting point
            self.flavour = flavour;
            self.dirty = true;
        }
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

    // ── Flavour runtime (0180) ────────────────────────────────────────────────

    fn flavour_with(base: Vec<f32>, bindings: Vec<Binding>) -> Flavour {
        Flavour { base, bindings, macro_defaults: [0.0; MACRO_SLOTS] }
    }

    /// Two flavours of the Driven family differ audibly via the **base vector alone**
    /// (macros held equal) — the "Kick vs Tom is a point in the same space" thesis.
    #[test]
    fn two_flavours_differ_by_base_only() {
        let mut a = KickTone::with_default_patch(48_000.0);
        let mut b = KickTone::with_default_patch(48_000.0);
        // Same (empty) bindings, same macros; only base pitch-depth + decay differ.
        a.apply_flavour(flavour_with(vec![0.001, 0.6, 36.0, 0.08], vec![]));
        b.apply_flavour(flavour_with(vec![0.001, 0.15, 4.0, 0.03], vec![]));
        let mut ba = vec![0.0_f32; 9_600];
        let mut bb = vec![0.0_f32; 9_600];
        a.on_trig(40.0, 1.0);
        b.on_trig(40.0, 1.0);
        a.render(&mut ba);
        b.render(&mut bb);
        // Deeper/longer flavour rings louder over the window than the short one.
        assert!(rms(&ba) > rms(&bb) * 1.3, "base-only flavours indistinct: {} vs {}", rms(&ba), rms(&bb));
    }

    /// A **macro binding** makes a slot move the sound; without the binding the same
    /// slot is inert — proving the binding table (not a fixed map) is what gives a
    /// macro meaning (ADR 0005).
    #[test]
    fn macro_binding_drives_sound_only_when_bound() {
        let base = vec![0.001, 0.3, 12.0, 0.05];
        // Bound: slot 0 drives decay hard. Unbound: no bindings at all.
        let bound = flavour_with(base.clone(), vec![Binding {
            slot: 0, param: P_AMP_DECAY as u8, curve: Curve::Linear, depth: 1.0,
        }]);
        let unbound = flavour_with(base, vec![]);

        let sweep = |flav: Flavour, m: f32| {
            let mut e = KickTone::with_default_patch(48_000.0);
            e.apply_flavour(flav);
            e.set_macro(0, m);
            let mut buf = vec![0.0_f32; 24_000]; // 0.5 s window
            e.on_trig(40.0, 1.0);
            e.render(&mut buf);
            rms(&buf)
        };
        // Bound: higher macro → longer decay ⇒ more energy across the window.
        let bound_lo = sweep(bound.clone(), 0.0);
        let bound_hi = sweep(bound, 1.0);
        assert!(bound_hi > bound_lo * 1.3, "bound slot inert: {bound_lo} vs {bound_hi}");
        // Unbound: the same macro move does nothing.
        let unbound_lo = sweep(unbound.clone(), 0.0);
        let unbound_hi = sweep(unbound, 1.0);
        assert!((unbound_hi - unbound_lo).abs() < 1e-6, "unbound slot moved sound: {unbound_lo} vs {unbound_hi}");
    }

    /// A flavour/macro change re-resolves at the **next trig**, never mid-voice: a
    /// currently ringing voice keeps the coefficients it triggered with (no glitch).
    #[test]
    fn change_takes_effect_on_next_trig_not_mid_voice() {
        let short = || flavour_with(vec![0.001, 0.08, 0.0, 0.05], vec![]);
        let long = flavour_with(vec![0.001, 0.9, 0.0, 0.05], vec![]);

        // Control: trig short, render 0.5 s, no mid-flight change.
        let mut ctrl = KickTone::with_default_patch(48_000.0);
        ctrl.apply_flavour(short());
        ctrl.on_trig(40.0, 1.0);
        let mut c = vec![0.0_f32; 24_000];
        ctrl.render(&mut c);

        // Test: same trig, then swap to a long-decay flavour *without* a new trig.
        let mut test = KickTone::with_default_patch(48_000.0);
        test.apply_flavour(short());
        test.on_trig(40.0, 1.0);
        let mut t = vec![0.0_f32; 4_800]; // 0.1 s in — voice still ringing
        test.render(&mut t);
        test.apply_flavour(long.clone()); // marks dirty; must NOT affect the live voice
        let mut t2 = vec![0.0_f32; 19_200];
        test.render(&mut t2);
        t.extend_from_slice(&t2);
        assert_eq!(c, t, "mid-voice flavour change glitched the sounding voice");

        // The long flavour does take effect on the next trig.
        test.on_trig(40.0, 1.0);
        let mut n = vec![0.0_f32; 24_000];
        test.render(&mut n);
        assert!(rms(&n) > rms(&c) * 1.3, "next trig ignored the new flavour: {} vs {}", rms(&n), rms(&c));
    }

    /// The Driven family exposes its param-space metadata for the editor / value-text.
    #[test]
    fn family_params_are_queryable() {
        let e = KickTone::with_default_patch(48_000.0);
        let p = e.family_params();
        assert_eq!(p.len(), DRIVEN_P);
        assert_eq!(p[P_AMP_DECAY].name, "Decay");
    }
}
