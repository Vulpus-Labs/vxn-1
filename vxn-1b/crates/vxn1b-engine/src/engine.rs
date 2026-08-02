//! VXN1b engine — the **global block** (0214, ADR 0002 §1). Holds **2 ×
//! [`Synth`]** + the one global FX chain and master. Each `Synth` is a fully
//! independent unit (own voice pool, allocator, matrix, per-layer LFO 2 —
//! [`crate::synth`]); the global block sums their voices, runs the single serial
//! FX chain, and applies master volume.
//!
//! **MIDI demux / KeyMode (0215, ADR 0002 §2–§3).** A thin demux sits in front
//! of the two synths and routes events by the derived [`KeyMode`]:
//!
//! - **Single** (layer 2 off): all events → synth 1; synth 2 bypassed.
//! - **Dual** (layer 2 on, split off): every event fanned to both synths.
//! - **Split** (layer 2 on, split on): note-**ons** routed by pitch vs the split
//!   point (below → Lower / synth 2, at/above → Upper / synth 1 — VXN1's
//!   convention); CC / wheels / pressure fanned to both.
//!
//! **Note-offs are always broadcast to both synths, in every mode.** The owning
//! synth releases; the other no-ops on the unmatched pitch. This fixes the
//! split-move stuck-note bug (note-on routed at press time; split point moves;
//! a routed note-off would reach the wrong synth) with no per-note owner map and
//! no cut held notes — they ring out on their origin synth.
//!
//! **Single-mode bypass.** Layer 2 is off by default; while off, synth 2 is
//! neither driven nor ticked, so single mode is **byte-for-byte today's output
//! at today's CPU**.
//!
//! **Scope.** 1× oversampling only — the OS/decimation and FX section are
//! shared/deferred (E037). FX + master are read transitionally from layer 1's
//! param table; a dedicated global param block lands in 0220. The [`KeyState`]
//! (layer-2 toggle + split enable + point) is non-automatable domain state; its
//! serialisation into the two-layer `clap.state` blob lands in 0221 — this crate
//! owns the record shape ([`KeyState::write`]/[`KeyState::read`]).

use crate::fx::{FxChain, FxParams};
use crate::matrix::MatrixTable;
use crate::params::{ClapRef, Layer, ParamId, clap_ref};
use crate::state::{LayerState, PluginState};
use crate::synth::{Synth, SynthSeeds};
use std::io::{self, Read, Write};

use vxn_dsp::CONTROL_BLOCK;

/// Default split point (MIDI note) — middle C, matching VXN1
/// ([`vxn-app` domain `DEFAULT_SPLIT_POINT`](../../../vxn-1/crates/vxn-app/src/domain.rs)).
pub const DEFAULT_SPLIT_POINT: u8 = 60;

/// Keyboard routing mode. **Derived** from the layer-2 on/off toggle and the
/// split-enable flag (ADR 0002 §3), never stored directly — the two toggles are
/// the single source of truth so the UI can't desync `KeyMode` from them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyMode {
    /// Layer 2 off: synth 2 bypassed, all events → synth 1.
    Single,
    /// Layer 2 on, split off: every event fanned to both synths (full range).
    Dual,
    /// Layer 2 on, split on: note-ons partitioned at the split point.
    Split,
}

/// A UI-originated edit to the non-automatable keyboard state (0219). Parsed
/// from the faceplate's `set_key_mode` / `set_split_point` opcodes (ui-web's
/// `parse_custom_ui`), boxed as a `UiEvent::Custom` payload, and applied to the
/// shared [`KeyState`] channel on the controller tick (clap) — the audio thread
/// then re-syncs the engine. This is the non-param-state → engine wire that the
/// matrix topology edits (0210) will share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyOp {
    /// Derived KeyMode index: 0 = Single, 1 = Dual, 2 = Split.
    SetKeyMode(u8),
    /// Split point (MIDI note).
    SetSplitPoint(u8),
}

/// Which topology field of a matrix slot a UI edit targets (0219, absorbing
/// 0210). Depth is a CLAP param (`matrix_slot{n}_depth`) and does **not** travel
/// here — it rides the normal automatable-param path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatrixField {
    Source,
    Dest,
    Curve,
    ScaleSrc,
}

/// A UI edit to one matrix slot's topology on one layer (0219). `value` is the
/// wire `u8` (a `SourceId` / `DestId` / `Curve` discriminant); the store decodes
/// it via `from_u8`. Carried as a `UiEvent::Custom` payload alongside [`KeyOp`],
/// applied to the shared per-layer matrix channel + a reload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatrixEdit {
    pub layer: Layer,
    pub slot: u8,
    pub field: MatrixField,
    pub value: u8,
}

/// The global keyboard-routing state: the two toggles plus the split point.
/// Non-automatable (ADR 0002 §3) — it rides the plugin-state blob, not the CLAP
/// param table. `KeyMode` is derived from it. Kept a self-contained record so
/// the two-layer `clap.state` format (0221) can serialise it directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyState {
    /// Layer 2 active. Off → [`KeyMode::Single`] (synth 2 bypassed).
    pub layer2_on: bool,
    /// Split enabled (only meaningful when `layer2_on`): on → [`KeyMode::Split`],
    /// off → [`KeyMode::Dual`].
    pub split_enabled: bool,
    /// Split point (MIDI note): note-ons **below** go to Lower (synth 2), at or
    /// above go to Upper (synth 1).
    pub split_point: u8,
}

impl Default for KeyState {
    fn default() -> Self {
        Self { layer2_on: false, split_enabled: false, split_point: DEFAULT_SPLIT_POINT }
    }
}

impl KeyState {
    /// Apply a UI key-op (0219). A KeyMode index maps back to the two toggles
    /// (Single → layer 2 off; Dual → on, split off; Split → on, split on),
    /// preserving the split point; a SetSplitPoint sets the point.
    pub fn apply(&mut self, op: KeyOp) {
        match op {
            KeyOp::SetKeyMode(0) => self.layer2_on = false,
            KeyOp::SetKeyMode(1) => {
                self.layer2_on = true;
                self.split_enabled = false;
            }
            KeyOp::SetKeyMode(2) => {
                self.layer2_on = true;
                self.split_enabled = true;
            }
            KeyOp::SetKeyMode(_) => {}
            KeyOp::SetSplitPoint(n) => self.split_point = n,
        }
    }

    /// Derive the routing mode (ADR 0002 §3).
    #[inline]
    pub fn key_mode(&self) -> KeyMode {
        match (self.layer2_on, self.split_enabled) {
            (false, _) => KeyMode::Single,
            (true, false) => KeyMode::Dual,
            (true, true) => KeyMode::Split,
        }
    }

    /// Write the 3-byte record `[layer2_on, split_enabled, split_point]`.
    pub fn write(&self, w: &mut impl Write) -> io::Result<()> {
        w.write_all(&[self.layer2_on as u8, self.split_enabled as u8, self.split_point])
    }

    /// Read a 3-byte record. A short read is a hard error (corruption).
    pub fn read(r: &mut impl Read) -> io::Result<Self> {
        let mut b = [0u8; 3];
        r.read_exact(&mut b)?;
        Ok(Self { layer2_on: b[0] != 0, split_enabled: b[1] != 0, split_point: b[2] })
    }
}

/// The full VXN1b engine: the global block over two synths.
pub struct Engine {
    sample_rate: f32,
    max_frames: usize,
    /// The two independent synths. Index 0 is Upper (synth 1, always on); index
    /// 1 is Lower (synth 2, gated by [`KeyState::layer2_on`]).
    synths: [Synth; 2],
    /// Keyboard routing state: layer-2 toggle + split enable + split point. The
    /// derived [`KeyMode`] drives the demux. Defaults to single mode.
    key: KeyState,
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
                Synth::new(sample_rate, LayerState::factory_default(), &SynthSeeds::LAYER1),
                Synth::new(sample_rate, LayerState::factory_default(), &SynthSeeds::LAYER2),
            ],
            key: KeyState::default(),
            fx: FxChain::new(sample_rate),
        }
    }

    /// Overwrite **both layers'** patches from a decoded [`PluginState`] — the
    /// CLAP state-load / preset path (0216). The KeyMode / split state is applied
    /// separately via [`Self::set_key_state`] (0221).
    pub fn load_state(&mut self, state: PluginState) {
        let [l1, l2] = state.layers;
        self.synths[0].load_state(l1);
        self.synths[1].load_state(l2);
    }

    /// Read a CLAP-id param value (0216 two-layer map): Layer-1 ids read synth 0,
    /// Layer-2 ids read synth 1, globals read synth 0 (both hold the same value).
    #[inline]
    pub fn param(&self, clap_id: usize) -> f32 {
        match clap_ref(clap_id) {
            Some(ClapRef::Patch(Layer::L2, p)) => self.synths[1].param(p.index()),
            Some(r) => self.synths[0].param(r.inner().index()),
            None => 0.0,
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn max_frames(&self) -> usize {
        self.max_frames
    }

    /// Mutable access to a layer's matrix topology (for preset load / tests).
    pub fn matrix_mut(&mut self, layer: Layer) -> &mut MatrixTable {
        self.synths[layer as usize].matrix_mut()
    }

    /// Set a CLAP-id param (0216 two-layer map). A Layer-1/Layer-2 patch id routes
    /// to that synth; a global id is applied to **both** synths so their shared
    /// FX/master reads stay consistent (globals are single-instance, ADR §7).
    pub fn set_param(&mut self, clap_id: usize, value: f32) {
        match clap_ref(clap_id) {
            Some(ClapRef::Patch(Layer::L1, p)) => self.synths[0].set_param(p.index(), value),
            Some(ClapRef::Patch(Layer::L2, p)) => self.synths[1].set_param(p.index(), value),
            Some(ClapRef::Global(p)) => {
                let inner = p.index();
                self.synths[0].set_param(inner, value);
                self.synths[1].set_param(inner, value);
            }
            None => {}
        }
    }

    /// The current derived keyboard routing mode (ADR 0002 §3).
    #[inline]
    pub fn key_mode(&self) -> KeyMode {
        self.key.key_mode()
    }

    /// The keyboard-routing state (for the two-layer state blob, 0221).
    pub fn key_state(&self) -> KeyState {
        self.key
    }

    /// Replace the keyboard-routing state wholesale (state / preset load, 0221).
    pub fn set_key_state(&mut self, key: KeyState) {
        self.key = key;
    }

    /// Turn layer 2 on/off — the Single↔Dual/Split gate (ADR 0002 §3).
    pub fn set_layer2_on(&mut self, on: bool) {
        self.key.layer2_on = on;
    }

    /// Enable/disable the keyboard split (only meaningful with layer 2 on).
    pub fn set_split_enabled(&mut self, on: bool) {
        self.key.split_enabled = on;
    }

    /// Set the split point (MIDI note). Held notes are unaffected — routing is
    /// fixed at press time and note-offs broadcast, so moving the point never
    /// strands a held voice.
    pub fn set_split_point(&mut self, note: u8) {
        self.key.split_point = note;
    }

    pub fn set_pitch_bend(&mut self, bend: f32) {
        self.synths[0].set_pitch_bend(bend);
        if self.key.layer2_on {
            self.synths[1].set_pitch_bend(bend);
        }
    }

    pub fn set_mod_wheel(&mut self, w: f32) {
        self.synths[0].set_mod_wheel(w);
        if self.key.layer2_on {
            self.synths[1].set_mod_wheel(w);
        }
    }

    /// Note-on, demuxed by the current [`KeyMode`] (ADR 0002 §2): Single → synth
    /// 1; Dual → both; Split → Lower (synth 2) below the split point, Upper
    /// (synth 1) at/above. Returns the owning synth's allocated voice.
    pub fn note_on(&mut self, channel: u8, note: u8, velocity: f32) -> usize {
        match self.key.key_mode() {
            KeyMode::Single => self.synths[0].note_on(channel, note, velocity),
            KeyMode::Dual => {
                let v = self.synths[0].note_on(channel, note, velocity);
                self.synths[1].note_on(channel, note, velocity);
                v
            }
            KeyMode::Split => {
                if note < self.key.split_point {
                    self.synths[1].note_on(channel, note, velocity)
                } else {
                    self.synths[0].note_on(channel, note, velocity)
                }
            }
        }
    }

    /// Note-off — **always broadcast to both synths, in every mode** (ADR 0002
    /// §2). The synth that started the note releases it; the other has no
    /// matching held voice and no-ops. This is the split-move stuck-note fix.
    pub fn note_off(&mut self, channel: u8, note: u8) {
        self.synths[0].note_off(channel, note);
        self.synths[1].note_off(channel, note);
    }

    /// Poly pressure → the matching voice on both synths when layer 2 is on
    /// (fanned; ADR 0002 §2). The synth without that pitch held no-ops.
    pub fn poly_pressure(&mut self, channel: u8, note: u8, value: f32) {
        self.synths[0].poly_pressure(channel, note, value);
        if self.key.layer2_on {
            self.synths[1].poly_pressure(channel, note, value);
        }
    }

    pub fn channel_pressure(&mut self, channel: u8, value: f32) {
        self.synths[0].channel_pressure(channel, value);
        if self.key.layer2_on {
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
        if self.key.layer2_on {
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
    use crate::params::{MATRIX_SLOTS, TOTAL_PARAMS, clap_id_of, desc_for_clap_id};

    /// The Layer-1 CLAP id for an inner param — engine `set_param`/`param` take
    /// CLAP ids, so tests that mean "layer 1's X" resolve it through the map.
    fn l1(p: ParamId) -> usize {
        clap_id_of(Layer::L1, p)
    }

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
        let mut e = Engine::new(48_000.0, 512);
        e.set_layer2_on(true); // fuzz both synths, not just layer 1
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
        // Sweep the whole CLAP surface (both layers + globals) through extremes.
        for id in 0..TOTAL_PARAMS {
            let d = desc_for_clap_id(id).unwrap();
            for v in [d.min, d.max, d.default, d.min - 10.0, d.max + 10.0] {
                e.set_param(id, v);
                e.note_off((id % 20) as u8, (id * 3 % 128) as u8);
                e.process_block(&mut l, &mut r);
                assert!(
                    l.iter().chain(r.iter()).all(|s| s.is_finite()),
                    "non-finite output after clap param {id} ({}) = {v}",
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
        // Layer 2 starts at the factory depth for this slot — capture it to prove
        // a Layer-1 edit leaves it alone.
        let l2_default = e.synths[1].matrix().slots[2].depth;
        e.set_param(l1(ParamId::MatrixSlot2Depth), -0.5);
        assert_eq!(e.synths[0].matrix().slots[2].depth, -0.5);
        assert_eq!(e.synths[1].matrix().slots[2].depth, l2_default, "layer 2 untouched");
        // Clamp is honoured on the mirror too (params clamp to [-1, 1]).
        e.set_param(l1(ParamId::MatrixSlot2Depth), 9.0);
        assert_eq!(e.synths[0].matrix().slots[2].depth, 1.0);
        // A Layer-2 edit is private to layer 2.
        e.set_param(clap_id_of(Layer::L2, ParamId::MatrixSlot2Depth), 0.25);
        assert_eq!(e.synths[1].matrix().slots[2].depth, 0.25);
        assert_eq!(e.synths[0].matrix().slots[2].depth, 1.0, "layer 1 unchanged");
    }

    #[test]
    fn zeroing_amp_slot_depth_via_param_silences_note() {
        // 0205: depth automation is live — zeroing the default Env2→Amp slot
        // depth kills the VCA route the evaluator/bank reads, so the note is
        // silent. Proves the param → matrix → DSP path end-to-end.
        let mut e = Engine::new(48_000.0, 512);
        e.set_param(l1(ParamId::Env2Attack), 0.001);
        e.set_param(l1(ParamId::MatrixSlot0Depth), 0.0);
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
        e2.set_param(l1(ParamId::Env2Attack), 0.001);
        e2.set_param(l1(ParamId::MasterVolume), 0.35); // half of default 0.7 (global)
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
        dual.set_layer2_on(true);
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

    #[test]
    fn key_mode_is_derived_from_toggles() {
        let mut e = Engine::new(48_000.0, 512);
        assert_eq!(e.key_mode(), KeyMode::Single, "layer 2 off → Single");
        e.set_layer2_on(true);
        assert_eq!(e.key_mode(), KeyMode::Dual, "layer 2 on, split off → Dual");
        e.set_split_enabled(true);
        assert_eq!(e.key_mode(), KeyMode::Split, "layer 2 on, split on → Split");
        // Split-enable is inert while layer 2 is off — Single dominates.
        e.set_layer2_on(false);
        assert_eq!(e.key_mode(), KeyMode::Single, "split-enable ignored with layer 2 off");
    }

    #[test]
    fn single_mode_leaves_synth2_silent() {
        // Single: a note reaches synth 1 only. Synth 2, given a loud fast-attack
        // patch, must stay silent because the demux never routes to it.
        let mut e = Engine::new(48_000.0, 512);
        e.synths[1].set_param(ParamId::Env2Attack as usize, 0.001);
        e.note_on(0, 60, 1.0);
        // Synth 2 holds no voice → tick it in isolation and it is silent.
        let mut l = vec![0.0; 512];
        let mut r = vec![0.0; 512];
        e.synths[1].render_control_block(&mut l, &mut r);
        let peak = l.iter().chain(r.iter()).fold(0.0f32, |a, &s| a.max(s.abs()));
        assert_eq!(peak, 0.0, "single mode must not route notes to synth 2");
    }

    #[test]
    fn dual_fans_note_on_to_both_synths() {
        let mut e = Engine::new(48_000.0, 512);
        e.set_layer2_on(true);
        e.synths[0].set_param(ParamId::Env2Attack as usize, 0.001);
        e.synths[1].set_param(ParamId::Env2Attack as usize, 0.001);
        e.note_on(0, 60, 1.0);
        for s in 0..2 {
            let mut l = vec![0.0; 512];
            let mut r = vec![0.0; 512];
            e.synths[s].render_control_block(&mut l, &mut r);
            let peak = l.iter().chain(r.iter()).fold(0.0f32, |a, &x| a.max(x.abs()));
            assert!(peak > 0.0, "dual must drive synth {s}");
        }
    }

    #[test]
    fn split_routes_note_on_by_pitch() {
        // Below the split → Lower (synth 2); at/above → Upper (synth 1).
        let mut e = Engine::new(48_000.0, 512);
        e.set_layer2_on(true);
        e.set_split_enabled(true);
        e.set_split_point(60);
        for s in 0..2 {
            e.synths[s].set_param(ParamId::Env2Attack as usize, 0.001);
        }
        e.note_on(0, 48, 1.0); // below → synth 2
        e.note_on(0, 72, 1.0); // above → synth 1

        let peak = |e: &mut Engine, s: usize| {
            let mut l = vec![0.0; 512];
            let mut r = vec![0.0; 512];
            e.synths[s].render_control_block(&mut l, &mut r);
            l.iter().chain(r.iter()).fold(0.0f32, |a, &x| a.max(x.abs()))
        };
        assert!(peak(&mut e, 0) > 0.0, "note at/above split must sound on synth 1");
        assert!(peak(&mut e, 1) > 0.0, "note below split must sound on synth 2");

        // The at-split boundary note (== split point) is Upper, not Lower.
        let mut e2 = Engine::new(48_000.0, 512);
        e2.set_layer2_on(true);
        e2.set_split_enabled(true);
        e2.set_split_point(60);
        for s in 0..2 {
            e2.synths[s].set_param(ParamId::Env2Attack as usize, 0.001);
        }
        e2.note_on(0, 60, 1.0);
        assert!(peak(&mut e2, 0) > 0.0, "the split-point note itself is Upper");
        assert_eq!(peak(&mut e2, 1), 0.0, "the split-point note must not reach Lower");
    }

    #[test]
    fn split_move_does_not_strand_a_held_note() {
        // The bug this fixes: hold a note above the split, move the split above
        // it, release. The note-on routed to Upper (synth 1) at press time; the
        // note-off broadcasts to both, so synth 1 releases it even though the
        // note is now "below" the moved split. A second held note rings out.
        let mut e = Engine::new(48_000.0, 512);
        e.set_layer2_on(true);
        e.set_split_enabled(true);
        e.set_split_point(60);
        for s in 0..2 {
            e.synths[s].set_param(ParamId::Env2Attack as usize, 0.001);
            // Long release so a stranded voice would still be ringing at the check.
            e.synths[s].set_param(ParamId::Env2Release as usize, 5.0);
        }
        e.note_on(0, 64, 1.0); // above split → synth 1 (Upper)
        e.note_on(0, 72, 1.0); // another Upper note, stays held throughout

        // Move the split above the first held note.
        e.set_split_point(70);

        // Release the first note. If routing followed the *current* split it
        // would go to Lower and miss the held Upper voice — broadcast avoids that.
        e.note_off(0, 64);

        // Let the release run out on note 64's voice.
        let mut l = vec![0.0; 48_000];
        let mut r = vec![0.0; 48_000];
        e.process_block(&mut l, &mut r);

        // Note 64 must have been released — its voice is idle. Note 72 (still
        // held) keeps voicing, so the layer isn't range-killed.
        assert!(!e.synths[0].voices_holding(64), "released note must not be stuck");
        assert!(e.synths[0].voices_holding(72), "the still-held note must ring on");
    }

    #[test]
    fn note_off_broadcasts_in_single_mode() {
        // Even in single mode a note-off reaches synth 2 (harmless no-op) — the
        // "always broadcast" contract holds regardless of mode.
        let mut e = Engine::new(48_000.0, 512);
        e.note_on(0, 60, 1.0);
        e.note_off(0, 60); // must not panic / must be a clean no-op on synth 2
        assert!(!e.synths[0].voices_holding(60), "synth 1 released the note");
    }

    #[test]
    fn key_state_round_trips_through_blob() {
        let ks = KeyState { layer2_on: true, split_enabled: true, split_point: 48 };
        let mut buf = Vec::new();
        ks.write(&mut buf).unwrap();
        assert_eq!(buf.len(), 3, "key state is a fixed 3-byte record");
        let back = KeyState::read(&mut &buf[..]).unwrap();
        assert_eq!(back, ks);
        assert_eq!(back.key_mode(), KeyMode::Split);

        // A short read is corruption, not a default.
        assert!(KeyState::read(&mut &buf[..2]).is_err());
    }

    #[test]
    fn key_op_maps_mode_to_toggles() {
        let mut k = KeyState::default();
        k.split_point = 48;
        k.apply(KeyOp::SetKeyMode(1)); // Dual
        assert_eq!(k.key_mode(), KeyMode::Dual);
        assert!(k.layer2_on && !k.split_enabled);
        assert_eq!(k.split_point, 48, "split point preserved across a mode change");
        k.apply(KeyOp::SetKeyMode(2)); // Split
        assert_eq!(k.key_mode(), KeyMode::Split);
        k.apply(KeyOp::SetKeyMode(0)); // Single
        assert_eq!(k.key_mode(), KeyMode::Single);
        assert!(!k.layer2_on);
        k.apply(KeyOp::SetSplitPoint(72));
        assert_eq!(k.split_point, 72);
    }

    #[test]
    fn default_key_state_is_single_middle_c() {
        let ks = KeyState::default();
        assert!(!ks.layer2_on);
        assert!(!ks.split_enabled);
        assert_eq!(ks.split_point, DEFAULT_SPLIT_POINT);
        assert_eq!(ks.split_point, 60);
        assert_eq!(ks.key_mode(), KeyMode::Single);
    }
}
