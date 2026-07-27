//! VXN1b engine (ticket 0202, step 3): the matrix-modulated synth over two
//! 8-wide [`RenderBank`]s.
//!
//! Owns the param table, matrix topology, the 16-voice [`Voices`] coordinator,
//! two render banks (lanes 0–7 / 8–15), and the global LFO 2. Host blocks are
//! split into `CONTROL_BLOCK`-sample control blocks so modulation resolves at
//! VXN1's granularity (sr/32); each control block builds a mod-agnostic
//! [`BlockCtx`] from the params and renders both banks.
//!
//! **Scope (this step).** 1× oversampling only — the OS/decimation stage and the
//! FX section are shared/deferred code (E037), so a bit-exact render-parity
//! comparison configures VXN1 with oversampling and FX **off** (step 4). Master
//! volume is applied; the limiter (off by default) is not yet wired.

use vxn_dsp::{CONTROL_BLOCK, LfoCore};

use crate::bank::{BlockCtx, RenderBank};
use crate::matrix::MatrixTable;
use crate::params::{CrossModType, MATRIX_SLOTS, ParamId, Params};
use crate::voice::Voices;

/// Per-bank RNG seeds (distinct so the two banks' noise/drift streams
/// decorrelate, as VXN1's two layers do).
const BANK_SEEDS: [u64; 2] = [0x1b_0000_0001, 0x1b_0000_0002];

/// LFO 2 core seed.
const LFO2_SEED: u64 = 0x1b_0000_00f2;

/// The full VXN1b engine.
pub struct Engine {
    sample_rate: f32,
    max_frames: usize,
    params: Params,
    matrix: MatrixTable,
    voices: Voices,
    banks: [RenderBank; 2],
    /// Global LFO 2, ticked once per control block and broadcast to both banks.
    lfo2: LfoCore,
    /// Host pitch-bend in `[-1, 1]` — the hardwired bend term (ADR §3) *and* the
    /// PitchWheel matrix source.
    pitch_bend: f32,
    /// Mod wheel `[0, 1]` — the ModWheel matrix source.
    mod_wheel: f32,
}

impl Engine {
    pub fn new(sample_rate: f32, max_frames: usize) -> Self {
        let control_rate = sample_rate / CONTROL_BLOCK as f32;
        let mut engine = Self {
            sample_rate,
            max_frames,
            params: Params::default(),
            matrix: crate::matrix::default_patch(),
            voices: Voices::new(),
            banks: [
                RenderBank::new(sample_rate, BANK_SEEDS[0]),
                RenderBank::new(sample_rate, BANK_SEEDS[1]),
            ],
            lfo2: LfoCore::new(control_rate, LFO2_SEED),
            pitch_bend: 0.0,
            mod_wheel: 0.0,
        };
        // Depth authority is the param table (ADR 0001 §5). The default patch
        // authors its seed depths in the matrix, so seed the matching depth
        // params from it once — after this the two are kept in lock-step by
        // `set_param` (0205).
        for slot in 0..MATRIX_SLOTS {
            if let Some(p) = ParamId::slot_depth(slot) {
                engine.params.set(p, engine.matrix.slots[slot].depth);
            }
        }
        engine.apply_envelopes();
        engine
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn max_frames(&self) -> usize {
        self.max_frames
    }

    /// Mutable access to the patch topology (for preset load / tests).
    pub fn matrix_mut(&mut self) -> &mut MatrixTable {
        &mut self.matrix
    }

    /// Set a CLAP-id param (identity map). Envelope params re-cook the banks;
    /// slot-depth params mirror into the matrix the evaluator reads (0205).
    pub fn set_param(&mut self, id: usize, value: f32) {
        self.params.set_index(id, value);
        if is_envelope_param(id) {
            self.apply_envelopes();
        }
        if let Some(slot) = ParamId::slot_depth_index(id) {
            // Read back the clamped value so the mirror can't drift from the param.
            self.matrix.slots[slot].depth = self.params.slot_depth(slot);
        }
    }

    pub fn set_pitch_bend(&mut self, bend: f32) {
        self.pitch_bend = bend.clamp(-1.0, 1.0);
    }

    pub fn set_mod_wheel(&mut self, w: f32) {
        self.mod_wheel = w.clamp(0.0, 1.0);
    }

    /// Note-on: allocate a voice (MPE channel threaded) and trigger its DSP lane.
    pub fn note_on(&mut self, channel: u8, note: u8, velocity: f32) -> usize {
        let v = self.voices.note_on(channel, note, velocity);
        let (bank, lane) = (v / RenderBank::LANES, v % RenderBank::LANES);
        let shape = self.params.lfo1_shape();
        let free_run = self.params.bool(ParamId::Lfo1FreeRun);
        self.banks[bank].trigger_lane(lane, shape, free_run);
        v
    }

    pub fn note_off(&mut self, channel: u8, note: u8) {
        self.voices.note_off(channel, note);
    }

    pub fn poly_pressure(&mut self, channel: u8, note: u8, value: f32) {
        self.voices.poly_pressure(channel, note, value);
    }

    pub fn channel_pressure(&mut self, channel: u8, value: f32) {
        self.voices.channel_pressure(channel, value);
    }

    pub fn reset(&mut self) {
        self.voices.reset();
        for b in &mut self.banks {
            b.reset();
        }
    }

    /// Render one host block, splitting it into `CONTROL_BLOCK`-sample control
    /// blocks. Buffers are overwritten (not accumulated).
    pub fn process_block(&mut self, left: &mut [f32], right: &mut [f32]) {
        let master = self.params.get(ParamId::MasterVolume);
        let mut off = 0;
        while off < left.len() {
            let n = (left.len() - off).min(CONTROL_BLOCK);
            self.render_control_block(&mut left[off..off + n], &mut right[off..off + n], master);
            off += n;
        }
    }

    /// Render one ≤`CONTROL_BLOCK` control block. Ticks LFO 2, builds the block
    /// context, renders both banks, applies master volume.
    fn render_control_block(&mut self, l: &mut [f32], r: &mut [f32], master: f32) {
        l.fill(0.0);
        r.fill(0.0);

        // LFO 2: one tick per control block, broadcast to both banks.
        self.lfo2
            .set_rate(self.params.get(ParamId::Lfo2Rate));
        let lfo2_val = self.lfo2.next(self.params.lfo2_shape());

        let ctx = build_ctx(
            &self.params,
            &self.matrix,
            self.sample_rate,
            self.pitch_bend,
            self.mod_wheel,
            lfo2_val,
        );

        let view = self.voices.render_view();
        let lanes = RenderBank::LANES;
        let (a0, a1) = view.active.split_at_mut(lanes);
        self.banks[0].render(
            &ctx,
            &view.note[..lanes],
            &view.gate[..lanes],
            a0,
            &view.velocity[..lanes],
            &view.pressure[..lanes],
            &view.note_random[..lanes],
            l,
            r,
        );
        self.banks[1].render(
            &ctx,
            &view.note[lanes..],
            &view.gate[lanes..],
            a1,
            &view.velocity[lanes..],
            &view.pressure[lanes..],
            &view.note_random[lanes..],
            l,
            r,
        );

        for s in l.iter_mut().chain(r.iter_mut()) {
            *s *= master;
        }
    }

    fn apply_envelopes(&mut self) {
        let p = &self.params;
        let env1 = (
            p.get(ParamId::Env1Attack),
            p.get(ParamId::Env1Decay),
            p.get(ParamId::Env1Sustain),
            p.get(ParamId::Env1Release),
        );
        let env2 = (
            p.get(ParamId::Env2Attack),
            p.get(ParamId::Env2Decay),
            p.get(ParamId::Env2Sustain),
            p.get(ParamId::Env2Release),
        );
        let (s1, s2) = (p.env1_shape(), p.env2_shape());
        for b in &mut self.banks {
            b.set_envelopes(env1, s1, env2, s2);
        }
    }
}

/// Assemble the mod-agnostic block context from the current params. A free
/// function (not a `&self` method) so the returned [`BlockCtx`] borrows **only**
/// `matrix` — every scalar is copied out of `params` — leaving `voices` and
/// `banks` independently mutable during render. `os = 1` (oversampling deferred),
/// so `os_sample_rate == sample_rate`.
fn build_ctx<'a>(
    p: &Params,
    matrix: &'a MatrixTable,
    sample_rate: f32,
    pitch_bend: f32,
    mod_wheel: f32,
    lfo2_val: f32,
) -> BlockCtx<'a> {
    let (sync, pm_index, ring_mode) = match p.cross_mod_type() {
        CrossModType::Off => (false, 0.0, false),
        CrossModType::Sync => (true, 0.0, false),
        CrossModType::Pm => (false, p.get(ParamId::CrossModAmount), false),
        CrossModType::Ring => (false, 0.0, true),
    };
    // Hardwired pitch bend (ADR §3): global pitch += bend × range.
    let base_semis = p.get(ParamId::MasterTune) + pitch_bend * p.get(ParamId::PitchBendRange);
    BlockCtx {
        os_sample_rate: sample_rate,
        os: 1,
        osc1_wave: p.osc_wave(ParamId::Osc1Wave),
        osc2_wave: p.osc_wave(ParamId::Osc2Wave),
        osc1_level: p.get(ParamId::Osc1Level),
        osc2_level: p.get(ParamId::Osc2Level),
        sub_level: p.get(ParamId::SubLevel),
        noise_level: p.get(ParamId::NoiseLevel),
        noise_color: p.noise_color(),
        osc1_pw: p.get(ParamId::Osc1PulseWidth),
        osc2_pw: p.get(ParamId::Osc2PulseWidth),
        osc1_semi: osc_semis(p, ParamId::Osc1Octave, ParamId::Osc1Coarse, ParamId::Osc1Fine),
        osc2_semi: osc_semis(p, ParamId::Osc2Octave, ParamId::Osc2Coarse, ParamId::Osc2Fine),
        sync,
        pm_index,
        ring_mode,
        cross_mod_type: p.cross_mod_type(),
        cutoff: p.get(ParamId::Cutoff),
        hpf_cutoff: p.get(ParamId::HpfCutoff),
        resonance: p.get(ParamId::Resonance),
        drive: p.get(ParamId::Drive),
        filter_mode: p.filter_mode(),
        filter_slope: p.filter_slope(),
        base_semis,
        lfo1_shape: p.lfo1_shape(),
        lfo1_rate_hz: p.get(ParamId::Lfo1Rate),
        lfo1_delay_time: p.get(ParamId::Lfo1DelayTime),
        lfo1_fade: p.get(ParamId::Lfo1Fade),
        lfo2_val,
        portamento_time: p.get(ParamId::PortamentoTime),
        amp_env_bypass: p.bool(ParamId::AmpEnvBypass),
        drift_amount: p.get(ParamId::MasterDrift),
        spread: p.get(ParamId::Spread),
        matrix,
        mod_wheel,
        pitch_wheel: pitch_bend,
    }
}

/// Quantised per-osc tuning in semitones (VXN1): `round(octave)·12 +
/// round(coarse) + fine/100`.
#[inline]
fn osc_semis(p: &Params, octave: ParamId, coarse: ParamId, fine: ParamId) -> f32 {
    p.get(octave).round() * 12.0 + p.get(coarse).round() + p.get(fine) / 100.0
}

/// Whether a CLAP id is one of the eight ADSR value/shape params (so a set
/// re-cooks the banks' envelopes).
fn is_envelope_param(id: usize) -> bool {
    matches!(
        ParamId::from_index(id),
        Some(
            ParamId::Env1Attack
                | ParamId::Env1Decay
                | ParamId::Env1Sustain
                | ParamId::Env1Release
                | ParamId::Env1Shape
                | ParamId::Env2Attack
                | ParamId::Env2Decay
                | ParamId::Env2Sustain
                | ParamId::Env2Release
                | ParamId::Env2Shape
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silent_by_default_until_a_note() {
        let mut e = Engine::new(48_000.0, 512);
        let mut l = vec![1.0; 128];
        let mut r = vec![1.0; 128];
        e.process_block(&mut l, &mut r);
        assert!(l.iter().chain(r.iter()).all(|&s| s == 0.0), "no notes → silence");
    }

    #[test]
    fn a_held_note_makes_sound() {
        let mut e = Engine::new(48_000.0, 512);
        // Fast attack so the VCA opens within the first blocks.
        e.set_param(ParamId::Env2Attack as usize, 0.001);
        e.note_on(0, 60, 1.0);
        let mut l = vec![0.0; 512];
        let mut r = vec![0.0; 512];
        e.process_block(&mut l, &mut r);
        let peak = l.iter().fold(0.0f32, |a, &s| a.max(s.abs()));
        assert!(peak > 0.0, "a held note with the default patch must sound");
    }

    #[test]
    fn seventeenth_note_steals_across_banks() {
        // 16 held notes fill both banks; the 17th steals voice 0 (bank 0).
        let mut e = Engine::new(48_000.0, 512);
        for i in 0..16 {
            e.note_on(0, 48 + i as u8, 1.0);
        }
        let v = e.note_on(0, 90, 1.0);
        assert_eq!(v, 0, "17th note steals the oldest (voice 0)");
    }

    #[test]
    fn fresh_engine_params_match_matrix_depths() {
        // 0205: the param table and the matrix agree on every slot depth at
        // construction — no startup mismatch.
        let e = Engine::new(48_000.0, 512);
        for slot in 0..MATRIX_SLOTS {
            assert_eq!(
                e.params.slot_depth(slot),
                e.matrix.slots[slot].depth,
                "slot {slot} param/matrix depth disagree"
            );
        }
    }

    #[test]
    fn set_param_mirrors_slot_depth_into_matrix() {
        // 0205: a depth edit reaches the copy the evaluator reads.
        let mut e = Engine::new(48_000.0, 512);
        e.set_param(ParamId::MatrixSlot2Depth as usize, -0.5);
        assert_eq!(e.matrix.slots[2].depth, -0.5);
        // Clamp is honoured on the mirror too (params clamp to [-1, 1]).
        e.set_param(ParamId::MatrixSlot2Depth as usize, 9.0);
        assert_eq!(e.matrix.slots[2].depth, 1.0);
    }

    #[test]
    fn zeroing_amp_slot_depth_via_param_silences_note() {
        // 0205: depth automation is live — zeroing the default Env2→Amp slot
        // depth kills the VCA route the evaluator/bank reads, so the note is
        // silent. Proves the param → matrix → DSP path end-to-end.
        let mut e = Engine::new(48_000.0, 512);
        e.set_param(ParamId::Env2Attack as usize, 0.001);
        e.set_param(ParamId::MatrixSlot0Depth as usize, 0.0);
        e.note_on(0, 60, 1.0);
        let mut l = vec![0.0; 512];
        let mut r = vec![0.0; 512];
        e.process_block(&mut l, &mut r);
        let peak = l.iter().chain(r.iter()).fold(0.0f32, |a, &s| a.max(s.abs()));
        assert_eq!(peak, 0.0, "zeroing the amp slot depth must silence the voice");
    }

    #[test]
    fn master_volume_scales_output() {
        let mut e = Engine::new(48_000.0, 512);
        e.set_param(ParamId::Env2Attack as usize, 0.001);
        e.note_on(0, 60, 1.0);
        let mut l = vec![0.0; 512];
        let mut r = vec![0.0; 512];
        e.process_block(&mut l, &mut r);
        let loud = l.iter().fold(0.0f32, |a, &s| a.max(s.abs()));

        let mut e2 = Engine::new(48_000.0, 512);
        e2.set_param(ParamId::Env2Attack as usize, 0.001);
        e2.set_param(ParamId::MasterVolume as usize, 0.35); // half of default 0.7
        e2.note_on(0, 60, 1.0);
        let mut l2 = vec![0.0; 512];
        let mut r2 = vec![0.0; 512];
        e2.process_block(&mut l2, &mut r2);
        let quiet = l2.iter().fold(0.0f32, |a, &s| a.max(s.abs()));

        assert!(quiet < loud, "half master volume should be quieter ({quiet} vs {loud})");
    }
}
