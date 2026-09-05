//! Top-level engine: operator banks, the oversampling chain, and the limiter.
//!
//! ## Rate plan
//!
//! ```text
//!   operators  ──8x──▶  s8  ──4x──▶  limiter  ──4x──▶  s4 ──2x──▶ s2 ──1x──▶ out
//!              ──16x─▶  s16 ──8x──▶  s8  ──▶ (as above)
//! ```
//!
//! Generation runs at 8x or 16x ([`Quality`]); everything from the limiter down
//! runs at 4x, then decimates to 1x. Each [`HalfbandFir`] stage is named for its
//! *input* rate and only ever sees that rate — `s8` takes 8x in both qualities,
//! because at 16x the `s16` stage has already halved it. Sharing one stage
//! across two rates would leave its filter state incoherent across a quality
//! switch.
//!
//! The FX block the brief places at 4x is not here yet; the limiter occupies
//! that slot so the chain shape is real and FX can drop in beside it.
//!
//! ## Why banks rather than 20 independent voices
//!
//! The sizing bench found SIMD-across-voices to beat SIMD-across-operators by
//! 15–24%, and the win comes from lanes sharing a waveform table and adjacent
//! mips. So the 20 slots are three [`VoiceMajor`] banks of 8 lanes (24 lanes,
//! 4 unused), all running one patch — which is also why route gains can stay
//! broadcast scalars, with only per-operator *level* varying per lane. That is
//! exactly where the envelopes land.
//!
//! A bank is skipped wholesale when all 8 of its lanes are idle, so the common
//! case of a few notes held costs one bank, not three.

use vxn_core_utils::halfband::HalfbandFir;
use vxn_core_utils::limiter::StereoLimiter;

use vxn4_dsp::ops::{CompiledRouting, NOPS, SumBus, VoiceMajor};
use vxn4_dsp::wavetable::{ValueSlope, WaveBank};

use crate::alloc::{Action, Alloc, N_SLOTS, Phase};
use crate::patch::{Patch, patch};

/// Lanes per bank. 8 is what the sizing sweep found best; 4 and 16 both measured
/// worse (54.1 / 54.8 / 51.4 voices).
pub const LANES: usize = 8;

/// Banks needed to cover [`N_SLOTS`].
pub const N_BANKS: usize = N_SLOTS.div_ceil(LANES);

/// Control-rate period in samples at 1x. Envelopes tick once per control block.
pub const CONTROL_PERIOD: usize = 32;

/// Limiter ceiling. Well below unity, for three measured reasons.
///
/// The brief puts the limiter at 4x, so **two halfband stages run after it**.
/// That placement costs more than it looks like it should, and three separate
/// effects stack up — each measured here, none of them visible to a steady-tone
/// test:
///
/// 1. **The limiter overshoots its own threshold on complex material.**
///    `LimiterCore` smooths its gain with a one-pole, which lags a beating
///    waveform. Threshold 0.5 measured 0.582 out; threshold 0.89 measured
///    0.979 — 12–19% over. Against a constant-amplitude sine it holds its
///    threshold to four decimals, which is why this does not show up in the
///    limiter's own tests. Its docs are straight about the intent: a "safety
///    limiter, not a true-peak mastering meter", whose hard guarantee is the
///    `±1` clamp rather than the threshold.
///
/// 2. **Decimation exposes inter-sample peaks.** `StereoLimiter` detects
///    sample-peak only and explicitly leaves inter-sample overshoot downstream.
///    Limiting at 4x and then resampling to 1x lands samples nearer the true
///    continuous peak, so the peaks it declined to detect become real ones.
///
/// 3. **A hard onset clips before the gain converges.** `current_gain` starts
///    at 1.0 with a 2 ms one-pole attack, which cannot travel down to ~0.2
///    inside its 2 ms lookahead. A loud chord arriving in one sample is
///    therefore hard-clipped by the limiter's own `±1`, and the halfbands ring
///    on the squared edges to ~1.02 at 1x. This one is **independent of the
///    ceiling** — sweeping 0.70..0.89 moved the worst-case 1x peak only between
///    1.021 and 1.028 — which is what proves it is clipping and not gain
///    staging.
///
/// So the ceiling is not the whole answer and cannot be. The patches are gain
/// staged (see `Patch::gain`) so that ordinary playing never drives the limiter
/// hard enough for (3), the ceiling here absorbs (1) and (2), and
/// [`Engine::process`] clamps at 1x as a backstop for a fortissimo cluster.
///
/// **The architectural point stands**: a limiter upstream of a resampler cannot
/// be a brickwall. Either it moves to 1x, last in the chain — which is what
/// vxn-1b and vxn-2 do — or it gains true-peak detection. The brief wants FX at
/// 4x with the limiter after them, so this is a live design question.
const CEILING: f32 = 0.80;

/// Mip-0 wavetable length.
///
/// The sizing sweep found table length to have **no** measurable effect on
/// throughput across a 256..2048 span, so this is chosen purely for quality:
/// 2048 is the longest, costs nothing, and 129 KiB of bank is irrelevant when
/// it is shared across every voice.
pub const TABLE_LEN: usize = 2048;

/// Operator-block oversampling. Switchable at runtime; everything below the
/// limiter runs at 4x regardless.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Quality {
    /// 8x generation. ~109 voices of headroom on one core.
    #[default]
    X8,
    /// 16x generation. Half the headroom, for when 8x audibly aliases.
    X16,
}

impl Quality {
    pub const fn factor(self) -> usize {
        match self {
            Quality::X8 => 8,
            Quality::X16 => 16,
        }
    }

    /// Ticks of the operator block per 4x sample.
    const fn ticks_per_4x(self) -> usize {
        self.factor() / 4
    }
}

/// One decimating channel: the full 16x→1x cascade, with the 4x tap exposed.
///
/// Stages are named for their input rate and each is only ever fed that rate.
struct Chain {
    s16: HalfbandFir,
    s8: HalfbandFir,
    s4: HalfbandFir,
    s2: HalfbandFir,
}

impl Chain {
    fn new() -> Self {
        Self {
            s16: HalfbandFir::default(),
            s8: HalfbandFir::default(),
            s4: HalfbandFir::default(),
            s2: HalfbandFir::default(),
        }
    }

    fn reset(&mut self) {
        self.s16.reset();
        self.s8.reset();
        self.s4.reset();
        self.s2.reset();
    }

    /// Fold one 4x sample's worth of oversampled ticks down to 4x.
    ///
    /// Named `fold_*` rather than `to_*` because these consume filter state and
    /// mutate `self`; clippy reads a `to_*` on a non-`Copy` type as a cheap
    /// conversion, which this is the opposite of.
    #[inline]
    fn fold_to_4x(&mut self, ticks: &[f32], q: Quality) -> f32 {
        match q {
            Quality::X8 => self.s8.process(ticks[0], ticks[1]),
            Quality::X16 => {
                let a = self.s16.process(ticks[0], ticks[1]);
                let b = self.s16.process(ticks[2], ticks[3]);
                self.s8.process(a, b)
            }
        }
    }

    /// 4x → 1x. Consumes four 4x samples.
    #[inline]
    fn fold_to_1x(&mut self, x: [f32; 4]) -> f32 {
        let a = self.s4.process(x[0], x[1]);
        let b = self.s4.process(x[2], x[3]);
        self.s2.process(a, b)
    }
}

/// Base-rate latency of the decimation chain, in samples.
///
/// Each halfband contributes 16 samples of group delay *at its own input rate*,
/// so a stage running at Nx costs `16 / N` base-rate samples.
pub const fn latency_samples(q: Quality) -> u32 {
    // s8: 16/8 = 2, s4: 16/4 = 4, s2: 16/2 = 8.
    let base = 2 + 4 + 8;
    match q {
        // s16 adds 16/16 = 1.
        Quality::X16 => base + 1,
        Quality::X8 => base,
    }
}

pub struct Engine {
    sample_rate: f32,
    quality: Quality,
    patch_index: usize,
    patch: Patch,

    waves: WaveBank,
    banks: [VoiceMajor<LANES>; N_BANKS],
    routing: CompiledRouting,
    bus: SumBus,

    alloc: Alloc,

    left: Chain,
    right: Chain,
    limiter: StereoLimiter,

    /// 4x samples awaiting the final 4→1 fold.
    quad_l: [f32; 4],
    quad_r: [f32; 4],

    /// Samples until the next control tick.
    control_countdown: usize,
}

impl Engine {
    pub fn new(sample_rate: f32) -> Self {
        let p = patch(0);
        let waves = WaveBank::new(TABLE_LEN);
        let routing = CompiledRouting::compile(&p.routing);
        let bus = SumBus::new(&p.ops, &p.routing);
        let mut banks = [(); N_BANKS].map(|_| VoiceMajor::<LANES>::new());
        for b in banks.iter_mut() {
            b.set_waves(&p.ops);
        }
        // The limiter sits at 4x, so that is the rate it must be told about —
        // its lookahead and release are in samples.
        let mut limiter = StereoLimiter::new(sample_rate * 4.0);
        limiter.set_threshold(CEILING);
        Self {
            sample_rate,
            quality: Quality::default(),
            patch_index: 0,
            patch: p,
            waves,
            banks,
            routing,
            bus,
            alloc: Alloc::new(),
            left: Chain::new(),
            right: Chain::new(),
            limiter,
            quad_l: [0.0; 4],
            quad_r: [0.0; 4],
            control_countdown: 0,
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }

    pub fn patch_index(&self) -> usize {
        self.patch_index
    }

    pub fn patch_name(&self) -> &'static str {
        self.patch.name
    }

    pub fn active_voices(&self) -> usize {
        self.alloc.active_count()
    }

    pub fn latency_samples(&self) -> u32 {
        latency_samples(self.quality)
    }

    /// Switch oversampling. Resets only the stage that changes rate role.
    ///
    /// `s16` is idle at 8x, so entering 16x with stale state in it would splice
    /// a fragment of an older signal into the new one. The lower stages keep
    /// running at unchanged rates and keep their state.
    pub fn set_quality(&mut self, q: Quality) {
        if q == self.quality {
            return;
        }
        self.quality = q;
        self.left.s16.reset();
        self.right.s16.reset();

        // Re-cook every sounding lane. A phase increment is per *tick*, and
        // changing quality changes how many ticks there are per second, so an
        // increment cooked at the old rate plays an octave out at the new one.
        // Switching quality under a held note is the whole point of it being a
        // runtime control, so this cannot wait for the next note-on.
        let sr_os = self.sr_os();
        for slot in 0..N_SLOTS {
            if self.alloc.voices[slot].is_idle() {
                continue;
            }
            let pitch = self.alloc.voices[slot].pitch;
            let (bank, lane) = (slot / LANES, slot % LANES);
            self.banks[bank].cook_lane(&self.waves, &self.patch.ops, lane, pitch, sr_os);
        }
    }

    /// Select one of the five hardwired patches.
    ///
    /// Kills all sound: the patch defines the operator topology, so voices in
    /// flight are running a routing that is about to stop existing.
    pub fn set_patch(&mut self, index: usize) {
        let p = patch(index);
        self.patch_index = index % crate::patch::N_PATCHES;
        self.routing = CompiledRouting::compile(&p.routing);
        self.bus = SumBus::new(&p.ops, &p.routing);
        for b in self.banks.iter_mut() {
            b.set_waves(&p.ops);
        }
        self.patch = p;
        self.panic();
    }

    /// Silence everything immediately.
    pub fn panic(&mut self) {
        self.alloc.clear();
        for b in self.banks.iter_mut() {
            for lane in 0..LANES {
                b.reset_lane(lane, 0);
            }
        }
        self.left.reset();
        self.right.reset();
        self.limiter.reset();
        self.quad_l = [0.0; 4];
        self.quad_r = [0.0; 4];
        self.control_countdown = 0;
    }

    pub fn note_on(&mut self, note: u8, velocity: u8) {
        if velocity == 0 {
            self.note_off(note);
            return;
        }
        let action = self.alloc.note_on(&self.patch.eg, note, velocity);
        let (slot, fresh) = match action {
            Action::Start { slot } => (slot, true),
            Action::Reuse { slot } => (slot, false),
        };
        let (bank, lane) = (slot / LANES, slot % LANES);
        let pitch = self.alloc.voices[slot].pitch;
        if fresh {
            // Decorrelate by slot and note so two lanes on the same pitch do
            // not phase-lock into a doubled copy.
            let seed = (slot as u32).wrapping_mul(0x2545_F491) ^ (note as u32).wrapping_mul(0x9E37);
            self.banks[bank].reset_lane(lane, seed);
        }
        self.banks[bank].cook_lane(&self.waves, &self.patch.ops, lane, pitch, self.sr_os());
    }

    pub fn note_off(&mut self, note: u8) {
        self.alloc.note_off(note);
    }

    pub fn all_notes_off(&mut self) {
        self.alloc.all_notes_off();
    }

    /// Oversampled operator rate.
    fn sr_os(&self) -> f32 {
        self.sample_rate * self.quality.factor() as f32
    }

    /// Advance envelopes and push the resulting levels into the banks.
    fn control_tick(&mut self) {
        let dt = CONTROL_PERIOD as f32 / self.sample_rate;
        // Weight each operator's envelope by its sum-bus presence, so a pure
        // modulator cannot make a voice look loud to the steal heuristic.
        let mut weight = [0.0f32; NOPS];
        for (d, w) in weight.iter_mut().enumerate() {
            *w = self.bus.l[d].abs() + self.bus.r[d].abs();
        }
        let retired = self.alloc.control_tick(dt, &weight);

        for slot in 0..N_SLOTS {
            let (bank, lane) = (slot / LANES, slot % LANES);
            if retired & (1 << slot) != 0 {
                // Clear the history ring too — a retired lane that keeps its
                // last outputs would feed a stale tail into the next note
                // through the feedback diagonal.
                self.banks[bank].reset_lane(lane, slot as u32);
                continue;
            }
            let v = &self.alloc.voices[slot];
            if v.phase == Phase::Idle {
                continue;
            }
            for (d, eg) in v.eg.iter().enumerate() {
                self.banks[bank].set_lane_op_level(lane, d, self.patch.ops[d].level * eg.level);
            }
        }
    }

    /// True when every lane in `bank` is idle, so the bank can be skipped.
    fn bank_is_silent(&self, bank: usize) -> bool {
        (0..LANES).all(|lane| {
            let slot = bank * LANES + lane;
            slot >= N_SLOTS || self.alloc.voices[slot].is_idle()
        })
    }

    /// Render `out_l.len()` samples at the host rate.
    pub fn process(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        debug_assert_eq!(out_l.len(), out_r.len());
        let ticks_per_4x = self.quality.ticks_per_4x();
        let gain = self.patch.gain;
        let mut ticks = [0.0f32; 8];

        for i in 0..out_l.len() {
            if self.control_countdown == 0 {
                self.control_tick();
                self.control_countdown = CONTROL_PERIOD;
            }
            self.control_countdown -= 1;

            // Four 4x samples make one output sample.
            for q in 0..4 {
                for t in 0..ticks_per_4x {
                    let mut l = 0.0f32;
                    let mut r = 0.0f32;
                    for b in 0..N_BANKS {
                        if self.bank_is_silent(b) {
                            continue;
                        }
                        let (bl, br) =
                            self.banks[b].tick::<ValueSlope>(&self.waves, &self.routing, &self.bus);
                        l += bl;
                        r += br;
                    }
                    ticks[t] = l * gain;
                    ticks[t + 4] = r * gain;
                }
                // Interleaved into one scratch array so the two channels share a
                // loop; the halves never overlap because ticks_per_4x <= 4.
                self.quad_l[q] = self.left.fold_to_4x(&ticks[..ticks_per_4x], self.quality);
                self.quad_r[q] = self.right.fold_to_4x(&ticks[4..4 + ticks_per_4x], self.quality);
            }

            // Limiter at 4x — four samples per output sample.
            for q in 0..4 {
                let (l, r) = self.limiter.process(self.quad_l[q], self.quad_r[q]);
                self.quad_l[q] = l;
                self.quad_r[q] = r;
            }

            // Backstop only — see `CEILING`. The limiter runs two halfband
            // stages upstream of here and so cannot itself guarantee the 1x
            // output stays in range.
            // Backstop only — see `CEILING`. The limiter runs two halfband
            // stages upstream of here and so cannot itself guarantee the 1x
            // output stays in range.
            // Backstop only — see `CEILING`. Two halfband stages run downstream
            // of the limiter, so the limiter cannot itself bound this.
            out_l[i] = self.left.fold_to_1x(self.quad_l).clamp(-1.0, 1.0);
            out_r[i] = self.right.fold_to_1x(self.quad_r).clamp(-1.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn render(e: &mut Engine, samples: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (vec![0.0; samples], vec![0.0; samples]);
        e.process(&mut l, &mut r);
        (l, r)
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = Engine::new(SR);
        let (l, r) = render(&mut e, 1024);
        assert_eq!(peak(&l), 0.0);
        assert_eq!(peak(&r), 0.0);
    }

    #[test]
    fn a_note_makes_sound_on_every_patch_at_both_qualities() {
        for p in 0..crate::patch::N_PATCHES {
            for q in [Quality::X8, Quality::X16] {
                let mut e = Engine::new(SR);
                e.set_patch(p);
                e.set_quality(q);
                e.note_on(60, 100);
                let (l, r) = render(&mut e, 4096);
                let pk = peak(&l).max(peak(&r));
                assert!(
                    pk > 0.01,
                    "patch {} ({}) at {:?} produced peak {pk}",
                    p,
                    e.patch_name(),
                    q
                );
                assert!(pk <= 1.0, "patch {} exceeded full scale: {pk}", e.patch_name());
                assert!(l.iter().all(|s| s.is_finite()));
                assert!(r.iter().all(|s| s.is_finite()));
            }
        }
    }

    /// Under musical load the output must stay clear of full scale entirely —
    /// no clamping anywhere in the chain.
    #[test]
    fn a_musical_chord_never_approaches_full_scale() {
        for p in 0..crate::patch::N_PATCHES {
            for q in [Quality::X8, Quality::X16] {
                let mut e = Engine::new(SR);
                e.set_patch(p);
                e.set_quality(q);
                for n in [48u8, 55, 60, 63, 67, 70] {
                    e.note_on(n, 100);
                }
                let (l, r) = render(&mut e, 16_384);
                let pk = peak(&l).max(peak(&r));
                assert!(pk < 0.999, "patch {p} at {q:?} reached {pk}");
            }
        }
    }

    /// Twenty notes landing in a single sample is a ~14x step into the limiter,
    /// and its 2 ms lookahead cannot fully track a 5 ms attack, so its own
    /// internal clamp engages briefly at the onset. That is a safety limiter
    /// behaving correctly. What must hold is that it is confined to the onset:
    /// once the gain envelope has converged, the steady state has to sit under
    /// the ceiling rather than riding the clamp.
    #[test]
    fn an_extreme_onset_clamps_only_during_the_transient() {
        let settle = (SR * 0.05) as usize;
        for p in 0..crate::patch::N_PATCHES {
            for q in [Quality::X8, Quality::X16] {
                let mut e = Engine::new(SR);
                e.set_patch(p);
                e.set_quality(q);
                for n in 0..N_SLOTS {
                    e.note_on(40 + n as u8 * 2, 127);
                }
                let (l, r) = render(&mut e, 32_768);
                // The hard guarantee, everywhere.
                assert!(peak(&l).max(peak(&r)) <= 1.0, "patch {p} exceeded full scale");
                // The real check: steady state is limited, not clipped.
                let tail = peak(&l[settle..]).max(peak(&r[settle..]));
                assert!(
                    tail < 0.999,
                    "patch {p} at {q:?} still riding the clamp after settling ({tail})"
                );
            }
        }
    }

    #[test]
    fn output_stays_finite_and_bounded_under_a_dense_chord() {
        let mut e = Engine::new(SR);
        e.set_patch(4); // web — all 64 routes
        for n in 0..N_SLOTS {
            e.note_on(48 + n as u8, 127);
        }
        let (l, r) = render(&mut e, 8192);
        assert!(l.iter().chain(r.iter()).all(|s| s.is_finite()));
        assert!(peak(&l) <= 1.0 && peak(&r) <= 1.0, "limiter let it through");
    }

    #[test]
    fn a_released_note_decays_to_silence_and_frees_its_voice() {
        let mut e = Engine::new(SR);
        e.set_patch(1);
        e.note_on(60, 100);
        render(&mut e, 2048);
        assert_eq!(e.active_voices(), 1);
        e.note_off(60);
        let (l, _) = render(&mut e, (SR * 3.0) as usize);
        assert_eq!(e.active_voices(), 0, "voice never retired");
        // Tail of the render must be silent, not merely quiet.
        let tail = &l[l.len() - 512..];
        assert!(peak(tail) < 1e-5, "tail peak {}", peak(tail));
    }

    #[test]
    fn polyphony_caps_at_sixteen() {
        let mut e = Engine::new(SR);
        e.set_patch(1);
        for n in 0..24u8 {
            e.note_on(40 + n, 100);
            render(&mut e, 64);
        }
        assert_eq!(e.active_voices(), crate::alloc::N_ACTIVE);
    }

    #[test]
    fn panic_silences_everything() {
        let mut e = Engine::new(SR);
        e.set_patch(3);
        for n in 0..8u8 {
            e.note_on(50 + n, 110);
        }
        render(&mut e, 512);
        e.panic();
        let (l, r) = render(&mut e, 1024);
        assert_eq!(peak(&l), 0.0);
        assert_eq!(peak(&r), 0.0);
        assert_eq!(e.active_voices(), 0);
    }

    /// A quality switch must not splice stale filter state into the output.
    #[test]
    fn switching_quality_mid_note_does_not_glitch() {
        let mut e = Engine::new(SR);
        e.set_patch(0);
        e.note_on(60, 100);
        let (a, _) = render(&mut e, 2048);
        e.set_quality(Quality::X16);
        let (b, _) = render(&mut e, 2048);
        assert!(b.iter().all(|s| s.is_finite()));
        // The seam must not produce a sample far outside the signal's own range.
        let bound = peak(&a) * 2.0 + 0.05;
        assert!(peak(&b) < bound, "switch spiked to {} (bound {bound})", peak(&b));
    }

    /// Pitch must survive a quality switch under a held note.
    ///
    /// Increments are per tick, and quality changes the tick rate, so a lane
    /// that is not re-cooked plays an octave out. Measured by Goertzel at the
    /// note's own frequency rather than by peak level, because the level is
    /// unchanged by the bug — which is why the earlier glitch test passed
    /// straight through it.
    #[test]
    fn a_held_note_keeps_its_pitch_across_a_quality_switch() {
        let f0 = vxn4_dsp::ops::note_to_freq(69); // A440
        let energy_at = |x: &[f32], hz: f32| {
            let w = 2.0 * std::f32::consts::PI * hz / SR;
            let coeff = 2.0 * w.cos();
            let (mut s1, mut s2) = (0.0f32, 0.0f32);
            for &v in x {
                let s0 = v + coeff * s1 - s2;
                s2 = s1;
                s1 = s0;
            }
            (s1 * s1 + s2 * s2 - coeff * s1 * s2).abs() * 2.0 / x.len() as f32
        };

        let mut e = Engine::new(SR);
        e.set_patch(0);
        e.note_on(69, 100);
        render(&mut e, 4096);
        e.set_quality(Quality::X16);
        let (l, _) = render(&mut e, 8192);

        let total: f32 = l.iter().map(|s| s * s).sum::<f32>() / l.len() as f32;
        let fund = energy_at(&l, f0);
        let octave_up = energy_at(&l, f0 * 2.0);
        assert!(
            fund / total > 0.9,
            "fundamental holds only {:.3} of the energy after the switch",
            fund / total
        );
        assert!(
            octave_up < fund * 0.01,
            "energy appeared an octave up ({octave_up:.2e} vs {fund:.2e}) — \
             lanes were not re-cooked"
        );
    }

    #[test]
    fn latency_is_reported_per_quality() {
        assert_eq!(latency_samples(Quality::X8), 14);
        assert_eq!(latency_samples(Quality::X16), 15);
        let mut e = Engine::new(SR);
        assert_eq!(e.latency_samples(), 14);
        e.set_quality(Quality::X16);
        assert_eq!(e.latency_samples(), 15);
    }

    /// Block size must not change the output — the control-rate countdown has
    /// to survive a block boundary.
    #[test]
    fn block_size_does_not_change_the_render() {
        let render_with = |block: usize| {
            let mut e = Engine::new(SR);
            e.set_patch(2);
            e.note_on(64, 100);
            let mut out = Vec::new();
            let (mut l, mut r) = (vec![0.0; block], vec![0.0; block]);
            for _ in 0..(4096 / block) {
                e.process(&mut l, &mut r);
                out.extend_from_slice(&l);
            }
            out
        };
        let a = render_with(64);
        let b = render_with(128);
        assert_eq!(a.len(), b.len());
        assert_eq!(a, b, "render depends on block size");
    }

    /// Level must be even across the keyboard. Mip boundaries are crossed
    /// several times over five octaves, and a mip normalised against the wrong
    /// reference would show up here as a step at one specific pitch.
    ///
    /// One note at a time, with `panic` between, which matters: measuring this
    /// from a played chromatic run does not work. The patch has a 0.30 s
    /// release against 0.09 s between notes, so four notes overlap and beat,
    /// and the interference swamps the effect being looked for — that produced
    /// a 2.8x spread with nothing wrong with the tables at all.
    ///
    /// The per-mip normalisation guarantee this rests on is asserted directly
    /// in `vxn4_dsp::wavetable::tests::every_mip_is_normalised_and_bounded`.
    #[test]
    fn level_is_even_across_the_keyboard() {
        let mut e = Engine::new(SR);
        e.set_patch(0); // sine: level is purely the table read
        let settle = (SR * 0.02) as usize;
        let mut peaks = Vec::new();
        for note in 36..96u8 {
            e.panic();
            e.note_on(note, 100);
            let (l, _) = render(&mut e, (SR * 0.06) as usize);
            peaks.push(peak(&l[settle..]));
        }
        let lo = peaks.iter().cloned().fold(f32::MAX, f32::min);
        let hi = peaks.iter().cloned().fold(0.0f32, f32::max);
        assert!(lo > 0.05, "some notes were silent (min {lo})");
        assert!(
            hi / lo < 1.15,
            "level spread {:.3}x across five octaves (min {lo:.4}, max {hi:.4})",
            hi / lo
        );
        for (i, w) in peaks.windows(2).enumerate() {
            let ratio = w[1] / w[0];
            assert!(
                (0.92..1.09).contains(&ratio),
                "level stepped {ratio:.3}x at note {}",
                36 + i
            );
        }
    }

    /// The sine patch is the reference tone: one operator, nothing modulating.
    /// It must come out clean, which is really a test of the decimator.
    #[test]
    fn the_sine_patch_is_spectrally_clean() {
        let mut e = Engine::new(SR);
        e.set_patch(0);
        e.note_on(69, 127); // A440
        render(&mut e, 8192); // let the envelope settle
        let (l, _) = render(&mut e, 4096);

        // Goertzel at 440 Hz vs total energy: a clean sine puts nearly all of
        // its energy in the fundamental.
        let energy: f32 = l.iter().map(|s| s * s).sum();
        let w = 2.0 * std::f32::consts::PI * 440.0 / SR;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &x in &l {
            let s0 = x + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        let fund = (s1 * s1 + s2 * s2 - coeff * s1 * s2) * 2.0 / l.len() as f32;
        let ratio = fund / energy;
        assert!(ratio > 0.97, "only {ratio} of energy at the fundamental");
    }
}
