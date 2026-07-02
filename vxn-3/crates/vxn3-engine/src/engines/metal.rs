//! `Metal` — the modal resonator engine. Lanes = **modes**, not voices
//! (ADR 0001 §5).
//!
//! Eight inharmonic modal partials of *one* body. `on_trig` **injects
//! excitation** into the persistent modal state — a re-hit rides (and re-excites)
//! the still-decaying ring rather than spawning a parallel voice, the real
//! 909-hat behaviour. Choke is **damping**: a "closed" hit switches the shared
//! decay coefficient short, collapsing an in-progress "open" ring on the same
//! body (not a voice kill). Covers hats / ride / cymbal.
//!
//! Each mode is a unit-magnitude complex rotation scaled by the decay
//! coefficient each sample (a damped resonator). State is SoA across the modes
//! in plain `[f32; METAL_MODES]` arrays so the per-sample loop autovectorises;
//! `METAL_MODES = 8` is the engine-declared lane budget (≠ the poly cap of 4 —
//! lane budget is the engine's choice, ADR 0001 §5).

use vxn3_dsp::decay_coef;

use crate::track_engine::{EngineKind, TrackEngine, macro_map};

/// Modal partial count (the engine-declared lane budget). Two NEON `f32x4`.
pub const METAL_MODES: usize = 8;

/// Inharmonic partial ratios (cymbal/hat-like) and their relative amplitudes.
const RATIOS: [f32; METAL_MODES] = [1.0, 1.31, 1.83, 2.41, 2.74, 3.23, 3.78, 4.21];
const AMPS: [f32; METAL_MODES] = [1.0, 0.85, 0.72, 0.62, 0.54, 0.46, 0.40, 0.34];

#[derive(Copy, Clone, Debug)]
pub struct MetalPatch {
    /// Fundamental of the modal set in Hz (the body's pitch).
    pub base_hz: f32,
    /// Open-ring decay time to -60 dB (s).
    pub open_decay_s: f32,
    /// Closed/damped decay time to -60 dB (s) — the choke.
    pub closed_decay_s: f32,
    /// Notes strictly below this trigger a closed (damped) hit; at/above it, an
    /// open hit. Defaults to the GM split (42 closed / 46 open).
    pub closed_below: f32,
    /// Excitation gain per hit.
    pub excite: f32,
}

impl Default for MetalPatch {
    fn default() -> Self {
        Self {
            base_hz: 1_200.0,
            open_decay_s: 1.1,
            closed_decay_s: 0.08,
            closed_below: 44.0,
            excite: 0.5,
        }
    }
}

pub struct Metal {
    patch: MetalPatch,
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

    /// Output trim so the modal sum sits at a sensible level.
    out_gain: f32,
}

impl Metal {
    pub fn new(sample_rate: f32, patch: MetalPatch) -> Self {
        let mut e = Self {
            patch,
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
        };
        e.cook();
        e.cur_decay = e.open_decay;
        e
    }

    pub fn with_default_patch(sample_rate: f32) -> Self {
        Self::new(sample_rate, MetalPatch::default())
    }

    fn cook(&mut self) {
        let two_pi = std::f32::consts::TAU;
        for (k, &ratio) in RATIOS.iter().enumerate() {
            let f = (self.patch.base_hz * ratio).min(self.sample_rate * 0.49);
            let w = two_pi * f / self.sample_rate;
            self.cos_w[k] = w.cos();
            self.sin_w[k] = w.sin();
        }
        self.open_decay = decay_coef(self.patch.open_decay_s, self.sample_rate);
        self.closed_decay = decay_coef(self.patch.closed_decay_s, self.sample_rate);
    }
}

impl TrackEngine for Metal {
    fn render(&mut self, out: &mut [f32]) {
        let d = self.cur_decay;
        let g = self.out_gain;
        for s in out.iter_mut() {
            let mut acc = 0.0_f32;
            for k in 0..METAL_MODES {
                // Damped complex rotation: one resonator step per mode.
                let nr = d * (self.re[k] * self.cos_w[k] - self.im[k] * self.sin_w[k]);
                let ni = d * (self.re[k] * self.sin_w[k] + self.im[k] * self.cos_w[k]);
                self.re[k] = nr;
                self.im[k] = ni;
                acc += nr * self.amp[k];
            }
            *s = acc * g;
        }
    }

    fn on_trig(&mut self, note: f32, velocity: f32) {
        // Inject excitation: add energy to every mode (re-excites a live ring).
        let e = velocity * self.patch.excite;
        for k in 0..METAL_MODES {
            self.re[k] += e * self.amp[k];
        }
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
        let Some(r) = macro_map(EngineKind::Metal, slot, value) else {
            return;
        };
        match slot {
            0 => self.patch.open_decay_s = r.value, // open-ring length
            1 => self.patch.excite = r.value,       // excitation energy
            2 => self.patch.base_hz = r.value,      // body pitch
            _ => return,
        }
        self.cook();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(b: &[f32]) -> f32 {
        (b.iter().map(|&x| x * x).sum::<f32>() / b.len().max(1) as f32).sqrt()
    }

    const OPEN: f32 = 46.0;
    const CLOSED: f32 = 42.0;

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
        let mut warm = vec![0.0_f32; 9_600]; // 200 ms — let it decay a bit
        e.render(&mut warm);
        let before = rms(&warm[warm.len() - 480..]);

        e.on_trig(OPEN, 1.0); // re-hit
        let mut after = vec![0.0_f32; 480];
        e.render(&mut after);
        assert!(
            rms(&after) > before * 1.5,
            "re-hit should re-excite the ring: before={before}, after={}",
            rms(&after)
        );
    }

    #[test]
    fn closed_hit_chokes_open_ring_via_damping() {
        // A: open hit, then ring out. B: open hit, then a closed hit damps it.
        let mut a = Metal::with_default_patch(48_000.0);
        let mut b = Metal::with_default_patch(48_000.0);
        a.on_trig(OPEN, 1.0);
        b.on_trig(OPEN, 1.0);

        let mut a1 = vec![0.0_f32; 4_800];
        let mut b1 = vec![0.0_f32; 4_800];
        a.render(&mut a1);
        b.render(&mut b1);

        b.on_trig(CLOSED, 1.0); // choke

        let mut a2 = vec![0.0_f32; 9_600]; // 200 ms later
        let mut b2 = vec![0.0_f32; 9_600];
        a.render(&mut a2);
        b.render(&mut b2);

        let a_tail = rms(&a2[a2.len() - 2_400..]);
        let b_tail = rms(&b2[b2.len() - 2_400..]);
        assert!(
            b_tail < a_tail * 0.25,
            "closed hit should choke the open ring: open tail={a_tail}, choked tail={b_tail}"
        );
    }
}
