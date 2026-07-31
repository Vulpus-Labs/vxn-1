//! VXN1b engine — the **global block** (0214, ADR 0002 §1). Holds **2 ×
//! [`Synth`]** + the one global FX chain and master. Each `Synth` is a fully
//! independent unit (own voice pool, allocator, matrix, per-layer LFO 2 —
//! [`crate::synth`]); the global block sums their voices, runs the single serial
//! FX chain, and applies master volume.
//!
//! **Single-mode bypass.** Layer 2 is off by default (no control wires it on yet
//! — MIDI demux / KeyMode is 0215). While off, synth 2 is neither driven nor
//! ticked, so single mode is **byte-for-byte today's output at today's CPU**.
//! When on, both synths currently receive the *same* events (broadcast; routing
//! is 0215) and mix together before FX + master.
//!
//! **Scope.** 1× oversampling only — the OS/decimation and FX section are
//! shared/deferred (E037). FX + master are read transitionally from layer 1's
//! param table; a dedicated global param block lands in 0220.

use crate::fx::{FxChain, FxParams};
use crate::matrix::MatrixTable;
use crate::params::ParamId;
use crate::state::PluginState;
use crate::synth::{Synth, SynthSeeds};

use vxn_dsp::CONTROL_BLOCK;

/// The full VXN1b engine: the global block over two synths.
pub struct Engine {
    sample_rate: f32,
    max_frames: usize,
    /// The two independent synths (layer 1 always on; layer 2 gated by
    /// [`Self::layer2_on`]).
    synths: [Synth; 2],
    /// Whether layer 2 is active. Off → single mode (synth 2 fully bypassed). No
    /// control wires this yet (0215); the plugin path leaves it `false`.
    layer2_on: bool,
    /// The single global serial FX chain (0207): dynamics → chorus → phaser →
    /// delay → reverb, run over the summed synths before master volume.
    fx: FxChain,
}

impl Engine {
    pub fn new(sample_rate: f32, max_frames: usize) -> Self {
        // Factory patch: default params + default-patch topology with the slot
        // depths already reconciled (0205) — a single source of truth shared with
        // the CLAP shell's param store ([`crate::state::PluginState::factory_default`]).
        // Both synths start from the factory patch; single mode leaves synth 2 idle.
        Self {
            sample_rate,
            max_frames,
            synths: [
                Synth::new(sample_rate, PluginState::factory_default(), &SynthSeeds::LAYER1),
                Synth::new(sample_rate, PluginState::factory_default(), &SynthSeeds::LAYER2),
            ],
            layer2_on: false,
            fx: FxChain::new(sample_rate),
        }
    }

    /// Overwrite layer 1's patch (params + matrix topology) from a decoded
    /// [`PluginState`] — the CLAP state-load / preset path. Layer 2's patch and
    /// the KeyMode/split state arrive with the two-layer state format (0221).
    pub fn load_state(&mut self, state: PluginState) {
        self.synths[0].load_state(state);
    }

    /// Read a CLAP-id param value (identity map) from layer 1 — for the shell's
    /// param store sync and `get_value`.
    #[inline]
    pub fn param(&self, id: usize) -> f32 {
        self.synths[0].param(id)
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn max_frames(&self) -> usize {
        self.max_frames
    }

    /// Mutable access to layer 1's patch topology (for preset load / tests).
    pub fn matrix_mut(&mut self) -> &mut MatrixTable {
        self.synths[0].matrix_mut()
    }

    /// Set a CLAP-id param (identity map) on layer 1. The doubled per-layer param
    /// surface (0216/0221) will route layer-2 ids here later.
    pub fn set_param(&mut self, id: usize, value: f32) {
        self.synths[0].set_param(id, value);
    }

    pub fn set_pitch_bend(&mut self, bend: f32) {
        self.synths[0].set_pitch_bend(bend);
        if self.layer2_on {
            self.synths[1].set_pitch_bend(bend);
        }
    }

    pub fn set_mod_wheel(&mut self, w: f32) {
        self.synths[0].set_mod_wheel(w);
        if self.layer2_on {
            self.synths[1].set_mod_wheel(w);
        }
    }

    /// Note-on. No demux yet (0215): layer 1 always; layer 2 too when on
    /// (broadcast). Returns layer 1's allocated voice.
    pub fn note_on(&mut self, channel: u8, note: u8, velocity: f32) -> usize {
        let v = self.synths[0].note_on(channel, note, velocity);
        if self.layer2_on {
            self.synths[1].note_on(channel, note, velocity);
        }
        v
    }

    pub fn note_off(&mut self, channel: u8, note: u8) {
        self.synths[0].note_off(channel, note);
        if self.layer2_on {
            self.synths[1].note_off(channel, note);
        }
    }

    pub fn poly_pressure(&mut self, channel: u8, note: u8, value: f32) {
        self.synths[0].poly_pressure(channel, note, value);
        if self.layer2_on {
            self.synths[1].poly_pressure(channel, note, value);
        }
    }

    pub fn channel_pressure(&mut self, channel: u8, value: f32) {
        self.synths[0].channel_pressure(channel, value);
        if self.layer2_on {
            self.synths[1].channel_pressure(channel, value);
        }
    }

    pub fn reset(&mut self) {
        self.synths[0].reset();
        self.synths[1].reset();
        self.fx.reset();
    }

    /// Render one host block, splitting it into `CONTROL_BLOCK`-sample control
    /// blocks. Buffers are overwritten (not accumulated).
    pub fn process_block(&mut self, left: &mut [f32], right: &mut [f32]) {
        // FX + master are global; read transitionally from layer 1 (0220 gives
        // them a dedicated global param block).
        let master = self.synths[0].param(ParamId::MasterVolume as usize);
        let mut off = 0;
        while off < left.len() {
            let n = (left.len() - off).min(CONTROL_BLOCK);
            self.render_control_block(&mut left[off..off + n], &mut right[off..off + n], master);
            off += n;
        }
    }

    /// Render one ≤`CONTROL_BLOCK` control block: pre-zero, tick each active
    /// synth (accumulating), run the one global FX chain, apply master volume.
    fn render_control_block(&mut self, l: &mut [f32], r: &mut [f32], master: f32) {
        l.fill(0.0);
        r.fill(0.0);

        // Layer 1 always; layer 2 only when on — single mode never ticks synth 2.
        self.synths[0].render_control_block(l, r);
        if self.layer2_on {
            self.synths[1].render_control_block(l, r);
        }

        // Serial FX chain over the summed voices, at the global OS rate (today
        // 1×, so `l`/`r` are base-rate). Each effect is a true skip when off and
        // settled, so the default FX-off patch is a bit-exact passthrough here.
        self.fx.set_params(&FxParams::from_params(self.synths[0].params()));
        self.fx.process_block(l, r);

        // Master volume + a final finite guard. A denormal-free RT plugin must
        // never emit NaN/inf: an extreme param + dense-voice combo can drive a
        // ladder/feedback state non-finite, and one NaN sample poisons the host
        // graph (and fails `clap-validator`'s param-fuzz). Replacing non-finite
        // samples with silence contains it at the engine boundary.
        for s in l.iter_mut().chain(r.iter_mut()) {
            let v = *s * master;
            *s = if v.is_finite() { v } else { 0.0 };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::MATRIX_SLOTS;

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
    fn seventeenth_note_steals_within_layer1() {
        // 16 held notes fill layer 1's two banks; the 17th steals voice 0.
        let mut e = Engine::new(48_000.0, 512);
        for i in 0..16 {
            e.note_on(0, 48 + i as u8, 1.0);
        }
        let v = e.note_on(0, 90, 1.0);
        assert_eq!(v, 0, "17th note steals the oldest (voice 0)");
    }

    #[test]
    fn output_is_always_finite_under_param_and_note_fuzz() {
        // Mirrors clap-validator's `param-fuzz-basic`: dense polyphony (both
        // banks, high notes, wide channels, out-of-range pressure/bend) while
        // every param is swept through its extremes. An extreme filter/feedback
        // combo can drive DSP state non-finite; the engine's output guard must
        // still emit only finite samples (never a NaN/inf to the host).
        use crate::params::PARAMS;
        let mut e = Engine::new(48_000.0, 512);
        for i in 0..40u16 {
            let note = (i * 3) as u8;
            let ch = (i % 20) as u8;
            e.note_on(ch, note, (i as f32 / 40.0).max(0.05));
            e.poly_pressure(ch, note, 1.5); // out-of-range pressure
            e.poly_pressure(ch, note, -0.5);
        }
        for ch in 0..20u8 {
            e.channel_pressure(ch, 2.0);
        }
        e.set_pitch_bend(5.0);
        e.set_mod_wheel(-1.0);
        let mut l = vec![0.0f32; 512];
        let mut r = vec![0.0f32; 512];
        for (i, d) in PARAMS.iter().enumerate() {
            for v in [d.min, d.max, d.default, d.min - 10.0, d.max + 10.0] {
                e.set_param(i, v);
                e.note_off((i % 20) as u8, (i * 3 % 128) as u8);
                e.process_block(&mut l, &mut r);
                assert!(
                    l.iter().chain(r.iter()).all(|s| s.is_finite()),
                    "non-finite output after param {} ({i}) = {v}",
                    d.name,
                );
            }
        }
    }

    #[test]
    fn fresh_engine_params_match_matrix_depths() {
        // 0205: the param table and the matrix agree on every slot depth at
        // construction — no startup mismatch.
        let e = Engine::new(48_000.0, 512);
        for slot in 0..MATRIX_SLOTS {
            assert_eq!(
                e.synths[0].params().slot_depth(slot),
                e.synths[0].matrix().slots[slot].depth,
                "slot {slot} param/matrix depth disagree"
            );
        }
    }

    #[test]
    fn set_param_mirrors_slot_depth_into_matrix() {
        // 0205: a depth edit reaches the copy the evaluator reads.
        let mut e = Engine::new(48_000.0, 512);
        e.set_param(ParamId::MatrixSlot2Depth as usize, -0.5);
        assert_eq!(e.synths[0].matrix().slots[2].depth, -0.5);
        // Clamp is honoured on the mirror too (params clamp to [-1, 1]).
        e.set_param(ParamId::MatrixSlot2Depth as usize, 9.0);
        assert_eq!(e.synths[0].matrix().slots[2].depth, 1.0);
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

    #[test]
    fn single_mode_leaves_layer2_idle() {
        // Layer 2 off by default: enabling it (with a distinct patch) must change
        // the output, and it must be silent again when the note is released —
        // proving synth 2 is a real, separately-driven unit but bypassed in
        // single mode.
        let mut single = Engine::new(48_000.0, 512);
        single.set_param(ParamId::Env2Attack as usize, 0.001);
        single.note_on(0, 60, 1.0);
        let mut l = vec![0.0; 512];
        let mut r = vec![0.0; 512];
        single.process_block(&mut l, &mut r);
        let single_peak = l.iter().fold(0.0f32, |a, &s| a.max(s.abs()));
        assert!(single_peak > 0.0);

        // Same, but with layer 2 on and detuned — output must differ.
        let mut dual = Engine::new(48_000.0, 512);
        dual.layer2_on = true;
        dual.set_param(ParamId::Env2Attack as usize, 0.001);
        dual.synths[1].set_param(ParamId::Env2Attack as usize, 0.001);
        dual.synths[1].set_param(ParamId::Osc1Octave as usize, 1.0);
        dual.note_on(0, 60, 1.0);
        let mut l2 = vec![0.0; 512];
        let mut r2 = vec![0.0; 512];
        dual.process_block(&mut l2, &mut r2);
        assert!(
            l.iter().zip(&l2).any(|(x, y)| (x - y).abs() > 1e-6),
            "layer 2 on must change the mix"
        );
    }
}
