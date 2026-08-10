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
//! **Layer mix (0220, 0248, ADR 0002 §7).** Each synth carries its own
//! `layer_level` + `layer_mute` + `layer_pan` (patch params, so a preset holds
//! its own balance and placement), applied as a smoothed per-layer, per-channel
//! gain before the sum: level and pan are multiplied together and the *product*
//! is what smooths, so a fader move, a mute and a pan sweep are all the same
//! kind of short fade. Pan uses a constant-power law normalised to unity at
//! centre ([`pan_gains`]). Layer 1 renders into the output and
//! is scaled in place; layer 2 renders into scratch so the two can take
//! different gains before summing. Mute folds into that same gain rather than
//! gating the render, so a muted layer keeps its voices running — unmuting
//! resumes mid-note and never strands a held note. Post-fader meter taps (0240)
//! sit on each layer's contribution.
//!
//! **Scope.** 1× oversampling only — the OS/decimation and FX section are
//! shared/deferred (E037). FX + master read the global param block, which both
//! synths mirror, via layer 1's table. The [`KeyState`]
//! (layer-2 toggle + split enable + point + the LFO 2 link) is non-automatable
//! domain state; its serialisation into the two-layer `clap.state` blob lands in
//! 0221 — this crate owns the record shape
//! ([`KeyState::write`]/[`KeyState::read`]).
//!
//! **Cross-layer LFO 2 link (0217, ADR 0002 §5).** The one coupling between the
//! two otherwise-independent synths: with [`KeyState::lfo2_link`] set, layer 2's
//! LFO 2 adopts layer 1's phase each control block (rate + phase lock) instead
//! of running its own accumulator, so both layers' LFO2-driven matrix routes
//! move together. Layer 1 ticks first, so the master phase is always current.

use crate::fx::{FxChain, FxParams};
use crate::output::OutputStage;
use crate::matrix::MatrixTable;
use crate::params::{ClapRef, Layer, ParamId, clap_ref};
use crate::state::{LayerState, PluginState};
use crate::synth::{Synth, SynthSeeds};
use std::io::{self, Read, Write};
use std::sync::Arc;

use vxn_core_utils::{MeterBus, MeterTap};
use vxn_dsp::smoothing::Smoothed;
use vxn_dsp::{CONTROL_BLOCK, MAX_OVERSAMPLE, StereoLimiter};

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
    /// Cross-layer LFO 2 link (0217): Layer 2's LFO 2 slaves to Layer 1's.
    /// Named *link*, not sync — `lfo2_sync` is the (per-layer, automatable)
    /// tempo-sync param.
    SetLfo2Link(bool),
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

/// The global **non-automatable domain state**: the two keyboard toggles, the
/// split point, and the one cross-layer link (LFO 2, ADR 0002 §5). Not in the
/// CLAP param table (ADR 0002 §3) — it rides the plugin-state blob. `KeyMode` is
/// derived from it. Kept a self-contained record so the two-layer `clap.state`
/// format (0221) can serialise it directly.
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
    /// **Cross-layer LFO 2 link** (0217, ADR 0002 §5): when set, Layer 2's LFO 2
    /// slaves to Layer 1's — rate *and* phase lock, Layer 2's own `lfo2_rate` is
    /// ignored while linked (its shape is not). Only meaningful when `layer2_on`.
    /// Distinct from the per-layer `lfo2_sync` **param**, which is tempo sync.
    pub lfo2_link: bool,
}

impl Default for KeyState {
    fn default() -> Self {
        Self {
            layer2_on: false,
            split_enabled: false,
            split_point: DEFAULT_SPLIT_POINT,
            lfo2_link: false,
        }
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
            KeyOp::SetLfo2Link(on) => self.lfo2_link = on,
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

    /// Write the 4-byte record `[layer2_on, split_enabled, split_point,
    /// lfo2_link]`.
    pub fn write(&self, w: &mut impl Write) -> io::Result<()> {
        w.write_all(&[
            self.layer2_on as u8,
            self.split_enabled as u8,
            self.split_point,
            self.lfo2_link as u8,
        ])
    }

    /// Read a 4-byte record. A short read is a hard error (corruption).
    pub fn read(r: &mut impl Read) -> io::Result<Self> {
        let mut b = [0u8; 4];
        r.read_exact(&mut b)?;
        Ok(Self {
            layer2_on: b[0] != 0,
            split_enabled: b[1] != 0,
            split_point: b[2],
            lfo2_link: b[3] != 0,
        })
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
    /// Lock-free meter publish target (0240). Owned so a bare `Engine` (tests,
    /// the web build) meters without ceremony; the CLAP shell swaps in the
    /// plugin-lifetime bus via [`Self::set_meters`] at activate so the main
    /// thread's drain handle survives deactivate/reactivate cycles.
    meters: Arc<MeterBus>,
    /// Per-layer mix gain (0220, 0248), one `Smoothed` **per channel** per
    /// synth: `[layer][0] = L`, `[layer][1] = R`. Targets are
    /// `(layer_mute ? 0 : layer_level) × pan_gains(layer_pan)`, so a mute is a
    /// short fade rather than a hard gate — the layer keeps rendering
    /// underneath, so unmuting resumes mid-note without a click and never
    /// strands a held voice.
    ///
    /// Smoothing the *product* rather than the pan position is what makes one
    /// smoother per channel enough: a fader move, a mute and a pan sweep all
    /// arrive as a change in the same two targets, and each is a fade rather
    /// than a step.
    layer_gain: [[Smoothed; 2]; 2],
    /// Scratch for layer 2's control block, at the **oversampled** rate: layer 1
    /// renders into the OS bus and is scaled in place; layer 2 needs its own
    /// buffer so the two can take different gains before they sum. Sized for the
    /// largest factor and allocated once at construction — never on the audio
    /// thread.
    mix_scratch: [[f32; CONTROL_BLOCK * MAX_OVERSAMPLE]; 2],
    /// The oversampled L/R synthesis buses both layers sum into, decimated to
    /// the base rate by [`OutputStage`] before FX (0251).
    os_bus: [[f32; CONTROL_BLOCK * MAX_OVERSAMPLE]; 2],
    /// Decimators + the OS-change / silence / mono bookkeeping.
    output: OutputStage,
    /// Master brickwall limiter, last in the signal path — *after* master volume
    /// (0251), so a master boost can't push past the ceiling. Bypassed unless
    /// `limiter_on`, with a short dry↔limited crossfade on the toggle and a
    /// lookahead reset on the off→on edge.
    limiter: StereoLimiter,
    limiter_fade: Smoothed,
    limiter_on: bool,
    /// Whether the limiter's on-state has been adopted since construction /
    /// reset. The *first* block must snap the fade to the current setting, not
    /// ramp into it: a patch that loads with Limit already on would otherwise
    /// pass its first 10 ms — the note attack, i.e. exactly the peak the limiter
    /// exists to catch — through dry. Only a later toggle crossfades.
    limiter_primed: bool,
}

/// Layer level/mute fade, ms. Matches the FX chain's bypass fade — long enough
/// to mask a click, short enough that a mute feels immediate.
const LAYER_FADE_MS: f32 = 10.0;

/// Constant-power pan gains for a position in `[-1, 1]` (0248).
///
/// `gl = √2·cos(θ)`, `gr = √2·sin(θ)` with `θ = (pos + 1)·π/4`, so
/// `gl² + gr²` is constant across the whole sweep — the point of the law: a
/// layer keeps its apparent loudness as it crosses the image, which a linear
/// (equal-sum) law does not give.
///
/// The `√2` normalises **centre to unity** rather than the textbook `0.707`.
/// That is still constant power — just referenced to the centre instead of to
/// the total — and it means a centred patch renders exactly as it did before
/// pan existed. The cost lands at the extremes: a hard-panned layer puts
/// `1.414 ×` the centre amplitude into one channel. That extra 3 dB of peak is
/// inherent to holding power constant, not a bug in the normalisation.
#[inline]
fn pan_gains(pos: f32) -> (f32, f32) {
    let theta = (pos.clamp(-1.0, 1.0) + 1.0) * (core::f32::consts::FRAC_PI_4);
    let (sin, cos) = theta.sin_cos();
    (core::f32::consts::SQRT_2 * cos, core::f32::consts::SQRT_2 * sin)
}

impl Engine {
    pub fn new(sample_rate: f32, max_frames: usize) -> Self {
        // Factory patch: default params + default-patch topology with the slot
        // depths already reconciled (0205) — a single source of truth shared with
        // the CLAP shell's param store ([`crate::state::PluginState::factory_default`]).
        // Both synths start from the factory patch; single mode leaves synth 2 idle.
        //
        // The FX chain publishes the dynamics taps itself, so it takes a handle
        // to the same bus here — otherwise a bare `Engine` would meter master
        // and layers but silently not dynamics.
        let meters = Arc::new(MeterBus::new());
        let mut fx = FxChain::new(sample_rate);
        fx.set_meters(meters.clone());
        Self {
            sample_rate,
            max_frames,
            synths: [
                Synth::new(sample_rate, LayerState::factory_default(), &SynthSeeds::LAYER1),
                Synth::new(sample_rate, LayerState::factory_default(), &SynthSeeds::LAYER2),
            ],
            key: KeyState::default(),
            fx,
            meters,
            // Start at the factory unity level, not 0 — a fade-in on the first
            // block would clip the attack of a note that arrives immediately.
            layer_gain: [
                [
                    Smoothed::new(1.0, LAYER_FADE_MS, sample_rate),
                    Smoothed::new(1.0, LAYER_FADE_MS, sample_rate),
                ],
                [
                    Smoothed::new(1.0, LAYER_FADE_MS, sample_rate),
                    Smoothed::new(1.0, LAYER_FADE_MS, sample_rate),
                ],
            ],
            mix_scratch: [[0.0; CONTROL_BLOCK * MAX_OVERSAMPLE]; 2],
            os_bus: [[0.0; CONTROL_BLOCK * MAX_OVERSAMPLE]; 2],
            output: OutputStage::new(sample_rate),
            limiter: StereoLimiter::new(sample_rate),
            limiter_fade: Smoothed::new(0.0, LAYER_FADE_MS, sample_rate),
            limiter_on: false,
            limiter_primed: false,
        }
    }

    /// Adopt a caller-owned meter bus (0240). The CLAP shell holds one for the
    /// plugin's whole lifetime in its `Shared` state and hands it here at
    /// `activate`, so the main-thread drain keeps reading the same slots across
    /// a deactivate/reactivate (which rebuilds the `Engine`).
    pub fn set_meters(&mut self, meters: Arc<MeterBus>) {
        // The FX chain publishes the dynamics taps itself (it owns the kernel
        // whose reduction is being read), so it needs the same bus.
        self.fx.set_meters(meters.clone());
        self.meters = meters;
    }

    /// The meter bus this engine publishes into.
    pub fn meters(&self) -> &Arc<MeterBus> {
        &self.meters
    }

    /// Overwrite **both layers'** patches from a decoded [`PluginState`] — the
    /// CLAP state-load / preset path (0216). The KeyMode / split state is applied
    /// separately via [`Self::set_key_state`] (0221).
    pub fn load_state(&mut self, state: PluginState) {
        let [l1, l2] = state.layers;
        self.synths[0].load_state(l1);
        self.synths[1].load_state(l2);
        // The blob carries the keyboard record too (0221), so a state load
        // restores the routing mode along with the patches. The CLAP shell also
        // pushes it down its own key channel (that channel exists for plain UI
        // edits, which carry no state blob); applying it here as well is
        // idempotent and keeps `load_state` a complete restore on its own.
        self.key = state.key;
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

    /// Turn the cross-layer LFO 2 link on/off (0217, ADR 0002 §5). On → Layer 2's
    /// LFO 2 mirrors Layer 1's phase each control block (rate + phase lock);
    /// off → it free-runs from Layer 2's own patch.
    pub fn set_lfo2_link(&mut self, on: bool) {
        self.key.lfo2_link = on;
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
        // Decimator + limiter state is transport state: a stale FIR history or
        // lookahead window would leak the pre-reset signal into the first block
        // after the transport restarts (0251).
        self.output.reset();
        self.limiter.reset();
        self.limiter_fade.snap(0.0);
        self.limiter_on = false;
        self.limiter_primed = false;
        // Drop any pending peaks so a re-started transport doesn't paint a
        // meter from before the reset.
        self.meters.clear();
    }

    /// Render one host block, splitting it into `CONTROL_BLOCK`-sample control
    /// blocks. Buffers are overwritten (not accumulated).
    pub fn process_block(&mut self, left: &mut [f32], right: &mut [f32]) {
        // FX + master are global params: `set_param` writes a global to BOTH
        // synths, so either table holds the same value and layer 1's is read
        // here by convention.
        let master = self.synths[0].param(ParamId::MasterVolume as usize);
        // Oversampling factor for this call. Resolved once (not per control
        // block) so a change lands on a call boundary, and handed to the output
        // stage first so the decimator reset + crossfade are armed before any
        // audio is produced at the new rate.
        let os = self.synths[0].params().oversample_factor();
        self.output.on_os_change(os);
        let limiter_on = self.synths[0].params().bool(ParamId::LimiterOn);
        let mut off = 0;
        while off < left.len() {
            let n = (left.len() - off).min(CONTROL_BLOCK);
            self.render_control_block(
                &mut left[off..off + n],
                &mut right[off..off + n],
                master,
                os,
                limiter_on,
            );
            off += n;
        }
    }

    /// The `(L, R)` mix gains a layer should fade toward: its level (or zero
    /// when muted) placed by [`pan_gains`].
    ///
    /// Mute folds into the same smoothed gain rather than gating the render, so
    /// a muted layer keeps running its voices and envelopes — unmuting resumes
    /// mid-note instead of restarting, and a held note is never stranded.
    #[inline]
    fn layer_gain_target(&self, layer: usize) -> [f32; 2] {
        let p = self.synths[layer].params();
        let level = if p.bool(ParamId::LayerMute) { 0.0 } else { p.get(ParamId::LayerLevel) };
        let (gl, gr) = pan_gains(p.get(ParamId::LayerPan));
        [level * gl, level * gr]
    }

    /// Render one ≤`CONTROL_BLOCK` control block: pre-zero, tick each active
    /// synth, apply its per-layer mix gain, sum, run the one global FX chain,
    /// apply master volume.
    fn render_control_block(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        master: f32,
        os: usize,
        limiter_on: bool,
    ) {
        let n = l.len();
        // Voices render into the oversampled buses; `l`/`r` receive the
        // decimated result further down (0251). At os = 1 the decimator is a
        // pass-through, so the OS-off render is bit-identical to the pre-0251
        // path.
        let os_n = n * os;
        // Gain targets read `self.synths`, so resolve them before the `os_bus`
        // borrow starts — the split below holds `self` mutably for the rest of
        // the render.
        let gain_target = [self.layer_gain_target(0), self.layer_gain_target(1)];
        // Same reason: the decimator's silence hint is a voice read.
        let both_silent =
            self.synths[0].is_silent() && (!self.key.layer2_on || self.synths[1].is_silent());
        let (bus_l, bus_r) = self.os_bus.split_at_mut(1);
        let (bus_l, bus_r) = (&mut bus_l[0][..os_n], &mut bus_r[0][..os_n]);
        bus_l.fill(0.0);
        bus_r.fill(0.0);

        // Layer 1 always; layer 2 only when on — single mode never ticks synth 2.
        // Layer 1 ticks first, so its just-advanced LFO 2 phase is this block's
        // master when the cross-layer link is on (0217): layer 2 adopts it
        // instead of running its own accumulator. Link off → `None`, the
        // free-running path, at no cost.
        //
        // Layer 1 renders straight into the output and is scaled in place; layer
        // 2 renders into scratch so the two can take different gains before they
        // sum (0220). Both gains ramp per sample, so a fader move, a mute or a
        // pan sweep is a short fade, not a step.
        self.synths[0].render_control_block(bus_l, bus_r, None, os);
        self.layer_gain[0][0].set_target(gain_target[0][0]);
        self.layer_gain[0][1].set_target(gain_target[0][1]);
        // The gain smoothers tick once per BASE frame and are held across that
        // frame's OS sub-samples: a fader or pan move must take the same
        // wall-clock time to land at 8x as at 1x (0251).
        for i in 0..n {
            let (gl, gr) = (self.layer_gain[0][0].tick(), self.layer_gain[0][1].tick());
            for k in 0..os {
                bus_l[i * os + k] *= gl;
                bus_r[i * os + k] *= gr;
            }
        }
        // Post-fader tap (0220): what this layer actually contributes to the
        // mix, so a muted or pulled-down layer reads zero — which is what a
        // mixer strip should show. Post-*pan* too (0248), so a hard-panned layer
        // reads on one channel only.
        self.meters.publish_block_peak(MeterTap::Layer1L, bus_l, bus_r);

        if self.key.layer2_on {
            let lfo2_master = self.key.lfo2_link.then(|| self.synths[0].lfo2_phase());
            self.layer_gain[1][0].set_target(gain_target[1][0]);
            self.layer_gain[1][1].set_target(gain_target[1][1]);
            // Split the scratch borrow so both halves are live at once.
            let (scratch_l, scratch_r) = self.mix_scratch.split_at_mut(1);
            let (s_l, s_r) = (&mut scratch_l[0][..os_n], &mut scratch_r[0][..os_n]);
            s_l.fill(0.0);
            s_r.fill(0.0);
            self.synths[1].render_control_block(s_l, s_r, lfo2_master, os);

            for i in 0..n {
                let (gl, gr) = (self.layer_gain[1][0].tick(), self.layer_gain[1][1].tick());
                for k in 0..os {
                    s_l[i * os + k] *= gl;
                    s_r[i * os + k] *= gr;
                }
            }
            self.meters.publish_block_peak(MeterTap::Layer2L, s_l, s_r);
            for i in 0..os_n {
                bus_l[i] += s_l[i];
                bus_r[i] += s_r[i];
            }
        } else {
            // Synth 2 is bypassed, so its gain must not sit part-way through a
            // fade waiting to be resumed — snap it, and let its meter rest.
            self.layer_gain[1][0].snap(gain_target[1][0]);
            self.layer_gain[1][1].snap(gain_target[1][1]);
        }

        // Decimate the oversampled buses down to the base rate. Both channels
        // always decimate (0262 dropped the spread-0 mono skip — pan makes it
        // unanswerable at block rate); both synths silent ⇒ the drain-skip can
        // eventually zero-fill. That bookkeeping lives in `OutputStage` (0251).
        self.output.decimate_block(bus_l, bus_r, l, r, os, both_silent);

        // Serial FX chain over the summed voices, at the base rate. Each effect
        // is a true skip when off and settled, so the default FX-off patch is a
        // bit-exact passthrough here.
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

        // Master limiter, genuinely last (0251): after master volume, so raising
        // the master can't push the output past the ceiling. Re-engaging clears
        // the lookahead first, or a stale transient leaks into the first block;
        // the fade crossfades dry↔limited so the toggle can't step level. Off
        // and settled it is a true skip, like the FX slots.
        if limiter_on && !self.limiter_on {
            self.limiter.reset();
        }
        self.limiter_on = limiter_on;
        let target = if limiter_on { 1.0 } else { 0.0 };
        if self.limiter_primed {
            self.limiter_fade.set_target(target);
        } else {
            // First block since construction / reset: adopt the setting whole.
            self.limiter_fade.snap(target);
            self.limiter_primed = true;
        }
        if limiter_on || self.limiter_fade.current() != 0.0 {
            for (ls, rs) in l.iter_mut().zip(r.iter_mut()) {
                let (dl, dr) = (*ls, *rs);
                let (wl, wr) = self.limiter.process(dl, dr);
                let w = self.limiter_fade.tick();
                *ls = dl + (wl - dl) * w;
                *rs = dr + (wr - dr) * w;
            }
        }

        // Master-out meter tap (0240) — after master volume, the limiter and the
        // finite guard, so it reports exactly what leaves the plugin. The bus
        // accumulates the max across control blocks, so the UI's ~60 Hz drain
        // sees the loudest sample since its last frame regardless of how many
        // control blocks fell between the two.
        self.meters.publish_block_peak(MeterTap::MasterL, l, r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meters::MeterFrame;
    use crate::params::{
        MATRIX_SLOTS, TOTAL_PARAMS, clap_id_of, desc_for_clap_id, global_clap_id,
    };

    /// The Layer-1 CLAP id for an inner param — engine `set_param`/`param` take
    /// CLAP ids, so tests that mean "layer 1's X" resolve it through the map.
    /// The CLAP id of a global param — FX / master live in the shared block.
    fn global_id(p: ParamId) -> usize {
        global_clap_id(p).expect("global param")
    }

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

    /// 0250: `CutoffTuned` is a UI display mode that persists with the patch —
    /// the engine must never read it. Rendered blocks with the toggle off and on
    /// have to be **bit-identical**, or the "Tuned" strip would quietly change
    /// the sound of a preset that happened to be saved with it on.
    #[test]
    fn cutoff_tuned_never_reaches_the_engine() {
        let render = |tuned: f32| {
            let mut e = Engine::new(48_000.0, 512);
            e.set_param(l1(ParamId::Env2Attack), 0.001);
            e.set_param(l1(ParamId::CutoffTuned), tuned);
            e.note_on(0, 60, 1.0);
            let mut l = vec![0.0; 512];
            let mut r = vec![0.0; 512];
            e.process_block(&mut l, &mut r);
            (l, r)
        };
        let (off_l, off_r) = render(0.0);
        let (on_l, on_r) = render(1.0);
        assert!(
            off_l.iter().fold(0.0f32, |a, &s| a.max(s.abs())) > 0.0,
            "the probe must actually sound, or this proves nothing"
        );
        assert_eq!(off_l, on_l, "Tuned changed the left channel");
        assert_eq!(off_r, on_r, "Tuned changed the right channel");
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

    /// 0218: one global `MasterDrift` drives **both** synths' voices. Each
    /// synth is rendered in isolation (as the demux tests do) so the assertion
    /// is per layer, not on the mix: drift > 0 changes that layer's voices,
    /// drift = 0 renders identically every time.
    #[test]
    fn global_drift_reaches_both_layers() {
        let render = |drift: f32, s: usize| -> Vec<f32> {
            let mut e = Engine::new(48_000.0, 512);
            e.set_layer2_on(true);
            for i in 0..2 {
                e.synths[i].set_param(ParamId::Env2Attack as usize, 0.001);
            }
            // One control, set once: the global block broadcasts it to both.
            e.set_param(global_clap_id(ParamId::MasterDrift).unwrap(), drift);
            for n in [60, 64, 67, 71] {
                e.note_on(0, n, 1.0);
            }
            // Chunked pre-zeroed accumulate — what the global block does.
            let mut out = vec![0.0f32; 4096];
            let mut r = vec![0.0f32; 4096];
            let mut off = 0;
            while off < out.len() {
                let n = (out.len() - off).min(CONTROL_BLOCK);
                out[off..off + n].fill(0.0);
                r[off..off + n].fill(0.0);
                e.synths[s].render_control_block(&mut out[off..off + n], &mut r[off..off + n], None, 1);
                off += n;
            }
            out
        };

        for s in 0..2 {
            let dry = render(0.0, s);
            assert!(dry.iter().any(|&x| x != 0.0), "layer {s} must sound");
            assert_eq!(dry, render(0.0, s), "drift 0 must be bit-identical, layer {s}");
            let drifted = render(0.9, s);
            assert!(
                dry.iter().zip(&drifted).any(|(x, y)| (x - y).abs() > 1e-9),
                "drift must reach layer {s}'s voices"
            );
        }
    }

    /// 0240: the master-out tap publishes what actually leaves the plugin, and
    /// the drain clears it. Silence in ⇒ a resting bus, so the view's decay
    /// starts falling rather than a stale peak latching.
    #[test]
    fn master_meter_tracks_output_and_clears_on_drain() {
        use crate::meters::MeterFrame;

        let mut e = Engine::new(48_000.0, 512);
        e.set_param(l1(ParamId::Env2Attack), 0.001);
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);

        // Idle: nothing rendered above zero, so nothing is published.
        e.process_block(&mut l, &mut r);
        assert!(MeterFrame::drain(e.meters()).is_silent(), "idle must not meter");

        e.note_on(0, 60, 1.0);
        e.process_block(&mut l, &mut r);
        let frame = MeterFrame::drain(e.meters());
        let block_peak = l.iter().fold(0.0f32, |a, &s| a.max(s.abs()));
        assert!(block_peak > 0.0, "a held note must produce output");
        // The tap sits after master volume + the finite guard, so it reports the
        // buffer exactly — not the pre-master sum.
        assert_eq!(frame.master.0, block_peak, "master tap must equal the output peak");

        // Read-and-clear: a second drain with no render in between is silent.
        assert!(MeterFrame::drain(e.meters()).is_silent(), "drain must clear");
    }

    /// The tap accumulates the **max** across the control blocks that make up
    /// one host block, so a transient in an early control block still reaches a
    /// UI frame that arrives many blocks later. This is the property a plain
    /// "store the last block's peak" publish would lose.
    #[test]
    fn master_meter_holds_the_peak_across_blocks_between_drains() {
        use crate::meters::MeterFrame;

        let mut e = Engine::new(48_000.0, 512);
        e.set_param(l1(ParamId::Env2Attack), 0.001);
        // Short decay to silence, so the loud transient is early and the later
        // blocks are quiet — the case that distinguishes hold from last-wins.
        e.set_param(l1(ParamId::Env2Decay), 0.01);
        e.set_param(l1(ParamId::Env2Sustain), 0.0);
        e.note_on(0, 60, 1.0);

        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        let mut loudest = 0.0f32;
        // Several host blocks, no drain in between — as when the audio thread
        // outruns the ~60 Hz editor tick.
        for _ in 0..8 {
            e.process_block(&mut l, &mut r);
            loudest = loudest.max(l.iter().fold(0.0f32, |a, &s| a.max(s.abs())));
        }
        let last_block_peak = l.iter().fold(0.0f32, |a, &s| a.max(s.abs()));
        assert!(loudest > last_block_peak, "test needs a decaying signal");
        assert_eq!(
            MeterFrame::drain(e.meters()).master.0,
            loudest,
            "the frame must carry the loudest sample since the last drain"
        );
    }

    /// A caller-owned bus survives the engine: the CLAP shell keeps one for the
    /// plugin's lifetime and re-attaches it to each freshly-activated `Engine`,
    /// so the main thread's drain handle never goes stale.
    #[test]
    fn an_adopted_meter_bus_outlives_the_engine() {
        use crate::meters::MeterFrame;

        let bus = Arc::new(MeterBus::new());
        {
            let mut e = Engine::new(48_000.0, 512);
            e.set_meters(bus.clone());
            e.set_param(l1(ParamId::Env2Attack), 0.001);
            e.note_on(0, 60, 1.0);
            let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
            e.process_block(&mut l, &mut r);
        }
        assert!(!MeterFrame::drain(&bus).is_silent(), "the shared bus keeps the reading");
    }

    /// 0220: the per-layer level fader scales that layer's contribution to the
    /// mix, and only that layer's.
    #[test]
    fn layer_level_scales_only_its_own_layer() {
        let peak_with = |l1_level: f32, l2_level: f32| -> (f32, f32) {
            let mut e = Engine::new(48_000.0, 512);
            e.set_layer2_on(true);
            for i in 0..2 {
                e.synths[i].set_param(ParamId::Env2Attack as usize, 0.001);
            }
            e.set_param(clap_id_of(Layer::L1, ParamId::LayerLevel), l1_level);
            e.set_param(clap_id_of(Layer::L2, ParamId::LayerLevel), l2_level);
            let (mut l, mut r) = (vec![0.0; 4096], vec![0.0; 4096]);
            // Settle the 10 ms level fade on silence first. Without this the
            // note's attack lands mid-ramp and the peak reports the gain the
            // fader was moving *from*, not the one it settled at.
            e.process_block(&mut l, &mut r);
            let _ = MeterFrame::drain(e.meters());
            e.note_on(0, 60, 1.0);
            e.process_block(&mut l, &mut r);
            let f = MeterFrame::drain(e.meters());
            (f.layer1.0, f.layer2.0)
        };

        let (full1, full2) = peak_with(1.0, 1.0);
        assert!(full1 > 0.0 && full2 > 0.0, "both layers must sound");

        // Pulling layer 1 down halves its post-fader peak and leaves layer 2 be.
        let (half1, still2) = peak_with(0.5, 1.0);
        assert!(
            (half1 / full1 - 0.5).abs() < 0.02,
            "layer 1 at 0.5 should be ~half: {half1} vs {full1}"
        );
        assert!((still2 - full2).abs() < 1e-6, "layer 2 must be untouched");
    }

    /// Mute silences a layer's contribution without stopping its voices — so
    /// unmuting resumes the held note mid-flight rather than restarting it.
    #[test]
    fn layer_mute_silences_the_layer_but_keeps_it_running() {
        let mut e = Engine::new(48_000.0, 512);
        e.set_layer2_on(true);
        for i in 0..2 {
            e.synths[i].set_param(ParamId::Env2Attack as usize, 0.001);
        }
        // Long sustain so the note is still held throughout.
        e.set_param(clap_id_of(Layer::L2, ParamId::Env2Sustain), 1.0);
        e.note_on(0, 60, 1.0);

        let (mut l, mut r) = (vec![0.0; 4096], vec![0.0; 4096]);
        e.process_block(&mut l, &mut r);
        assert!(MeterFrame::drain(e.meters()).layer2.0 > 0.0, "layer 2 must sound first");

        e.set_param(clap_id_of(Layer::L2, ParamId::LayerMute), 1.0);
        // First block still carries the 10 ms fade-out; the second is fully muted.
        e.process_block(&mut l, &mut r);
        e.process_block(&mut l, &mut r);
        let _ = MeterFrame::drain(e.meters());
        e.process_block(&mut l, &mut r);
        assert_eq!(
            MeterFrame::drain(e.meters()).layer2.0,
            0.0,
            "a muted layer must contribute silence"
        );

        // Unmute: the voice is still held, so sound returns without a new note.
        e.set_param(clap_id_of(Layer::L2, ParamId::LayerMute), 0.0);
        e.process_block(&mut l, &mut r);
        e.process_block(&mut l, &mut r);
        assert!(
            MeterFrame::drain(e.meters()).layer2.0 > 0.0,
            "unmuting must resume the held note — the layer kept rendering"
        );
    }

    /// A mute is a short fade, not a gate: no sample-to-sample step big enough
    /// to click. Guards the reason mute folds into the smoothed gain.
    #[test]
    fn muting_a_layer_does_not_step_the_output() {
        let mut e = Engine::new(48_000.0, 512);
        e.set_param(l1(ParamId::Env2Attack), 0.001);
        e.set_param(l1(ParamId::Env2Sustain), 1.0);
        e.note_on(0, 60, 1.0);
        let (mut l, mut r) = (vec![0.0; 2048], vec![0.0; 2048]);
        e.process_block(&mut l, &mut r);

        e.set_param(l1(ParamId::LayerMute), 1.0);
        e.process_block(&mut l, &mut r);
        let max_step = l.windows(2).fold(0.0f32, |a, w| a.max((w[1] - w[0]).abs()));
        // The signal itself moves sample to sample; the bar is that the mute
        // adds no discontinuity beyond ordinary waveform slew.
        let pre_step = {
            let mut e2 = Engine::new(48_000.0, 512);
            e2.set_param(l1(ParamId::Env2Attack), 0.001);
            e2.set_param(l1(ParamId::Env2Sustain), 1.0);
            e2.note_on(0, 60, 1.0);
            let (mut a, mut b) = (vec![0.0; 2048], vec![0.0; 2048]);
            e2.process_block(&mut a, &mut b);
            e2.process_block(&mut a, &mut b);
            a.windows(2).fold(0.0f32, |x, w| x.max((w[1] - w[0]).abs()))
        };
        assert!(
            max_step <= pre_step * 1.5 + 1e-6,
            "mute stepped the output: {max_step} vs unmuted {pre_step}"
        );
    }

    /// Layer 2's meter rests in single mode — synth 2 is never ticked, so
    /// nothing publishes and the strip reads empty rather than stale.
    #[test]
    fn layer2_meter_is_silent_in_single_mode() {
        let mut e = Engine::new(48_000.0, 512);
        e.set_param(l1(ParamId::Env2Attack), 0.001);
        e.note_on(0, 60, 1.0);
        let (mut l, mut r) = (vec![0.0; 1024], vec![0.0; 1024]);
        e.process_block(&mut l, &mut r);
        let f = MeterFrame::drain(e.meters());
        assert!(f.layer1.0 > 0.0, "layer 1 must meter");
        assert_eq!(f.layer2, (0.0, 0.0), "layer 2 must not meter when bypassed");
    }

    /// 0241: the dynamics slot reports input, output and gain reduction.
    /// Reduction is negative only while the compressor is actually working.
    #[test]
    fn dynamics_meters_report_in_out_and_reduction() {
        let mut e = Engine::new(48_000.0, 512);
        e.set_param(l1(ParamId::Env2Attack), 0.001);
        e.set_param(l1(ParamId::Env2Sustain), 1.0);
        let (mut l, mut r) = (vec![0.0; 4096], vec![0.0; 4096]);

        // Compressor off: in and out both read, reduction rests at 0.
        e.note_on(0, 60, 1.0);
        e.process_block(&mut l, &mut r);
        let off = MeterFrame::drain(e.meters());
        assert!(off.dynamics_in.0 > 0.0, "input must meter");
        assert!(off.dynamics_out.0 > 0.0, "output must meter");
        assert_eq!(off.dynamics_gr, 0.0, "a bypassed compressor reduces nothing");

        // Hard compression on a signal well above threshold.
        e.set_param(global_id(ParamId::DynamicsOn), 1.0);
        e.set_param(global_id(ParamId::DynamicsThreshold), -50.0);
        e.set_param(global_id(ParamId::DynamicsRatio), 20.0);
        for _ in 0..4 {
            e.process_block(&mut l, &mut r);
        }
        let on = MeterFrame::drain(e.meters());
        assert!(on.dynamics_gr < 0.0, "compressor must report reduction: {}", on.dynamics_gr);
        // Reduction is real: the slot's output is pulled below its input.
        assert!(
            on.dynamics_out.0 < on.dynamics_in.0,
            "out {} should sit below in {}",
            on.dynamics_out.0,
            on.dynamics_in.0
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
    fn load_state_restores_the_keyboard_record() {
        // 0221: the blob carries KeyState, so a state load is a complete restore
        // — an engine that loads a split patch must come back routing split,
        // without depending on the shell's separate key channel.
        let mut e = Engine::new(48_000.0, 512);
        assert_eq!(e.key_mode(), KeyMode::Single);

        let mut st = PluginState::factory_default();
        st.key = KeyState {
            layer2_on: true,
            split_enabled: true,
            split_point: 55,
            lfo2_link: true,
        };
        e.load_state(st);

        assert_eq!(e.key_mode(), KeyMode::Split);
        assert_eq!(e.key_state().split_point, 55);
        assert!(e.key_state().lfo2_link);
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
        e.synths[1].render_control_block(&mut l, &mut r, None, 1);
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
            e.synths[s].render_control_block(&mut l, &mut r, None, 1);
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
            e.synths[s].render_control_block(&mut l, &mut r, None, 1);
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

    /// Wire `LFO 2 → Cutoff` at full depth on both layers and give them
    /// **different** LFO 2 rates, so nothing but the link can hold them together.
    fn dual_engine_with_lfo2_to_cutoff() -> Engine {
        use crate::matrix::{Curve, DestId, MatrixSlot, SourceId};
        let mut e = Engine::new(48_000.0, 512);
        e.set_layer2_on(true);
        for s in 0..2 {
            e.synths[s].matrix_mut().slots[1] = MatrixSlot {
                source: SourceId::Lfo2,
                dest: DestId::Cutoff,
                depth: 1.0,
                curve: Curve::Lin,
                scale_src: SourceId::None,
            };
            e.synths[s].set_param(ParamId::MatrixSlot1Depth as usize, 1.0);
            e.synths[s].set_param(ParamId::Env2Attack as usize, 0.001);
            e.synths[s].set_param(ParamId::Cutoff as usize, 1_000.0);
        }
        e.synths[0].set_param(ParamId::Lfo2Rate as usize, 3.0);
        e.synths[1].set_param(ParamId::Lfo2Rate as usize, 17.0);
        e
    }

    /// 0217 / ADR 0002 §5: with the link on, layer 2's LFO 2 mirrors layer 1's
    /// phase every control block despite a wildly different `lfo2_rate` — rate
    /// *and* phase lock. Same phase + same shape is exactly "both layers'
    /// LFO2-driven dests move in phase": the dest contribution is
    /// `depth · curve(shape(phase))`, so equal phase makes it equal. With the
    /// link off they free-run and diverge.
    #[test]
    fn lfo2_link_locks_layer2_phase_to_layer1() {
        let mut linked = dual_engine_with_lfo2_to_cutoff();
        linked.set_lfo2_link(true);
        assert_eq!(
            linked.synths[0].param(ParamId::Lfo2Shape as usize),
            linked.synths[1].param(ParamId::Lfo2Shape as usize),
            "matching shapes, so equal phase means equal modulation"
        );
        linked.note_on(0, 60, 1.0);

        let mut free = dual_engine_with_lfo2_to_cutoff();
        free.note_on(0, 60, 1.0);

        let (mut l, mut r) = (vec![0.0; CONTROL_BLOCK], vec![0.0; CONTROL_BLOCK]);
        let mut free_diverged = false;
        for block in 0..400 {
            linked.process_block(&mut l, &mut r);
            assert_eq!(
                linked.synths[1].lfo2_phase(),
                linked.synths[0].lfo2_phase(),
                "linked layer 2 must track layer 1's LFO 2 phase (block {block})"
            );
            free.process_block(&mut l, &mut r);
            let gap = free.synths[1].lfo2_phase() - free.synths[0].lfo2_phase();
            free_diverged |= gap.abs() > 0.1;
        }
        assert!(free_diverged, "with the link off the two LFO 2s must free-run apart");
    }

    /// The link reaches the DSP, not just the phase accumulator: the same note on
    /// the same two patches renders differently with the link on, because layer
    /// 2's LFO2→Cutoff route now follows layer 1's (slower) phase instead of its
    /// own rate.
    #[test]
    fn lfo2_link_changes_the_rendered_mix() {
        let render = |link: bool| {
            let mut e = dual_engine_with_lfo2_to_cutoff();
            e.set_lfo2_link(link);
            e.note_on(0, 60, 1.0);
            let (mut l, mut r) = (vec![0.0; 4096], vec![0.0; 4096]);
            e.process_block(&mut l, &mut r);
            l
        };
        let (linked, free) = (render(true), render(false));
        assert!(linked.iter().any(|&s| s != 0.0), "the test patch must sound");
        assert!(
            linked.iter().zip(&free).any(|(x, y)| (x - y).abs() > 1e-6),
            "the LFO 2 link must change layer 2's filter movement"
        );
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
        let ks = KeyState {
            layer2_on: true,
            split_enabled: true,
            split_point: 48,
            lfo2_link: true,
        };
        let mut buf = Vec::new();
        ks.write(&mut buf).unwrap();
        assert_eq!(buf.len(), 4, "key state is a fixed 4-byte record");
        let back = KeyState::read(&mut &buf[..]).unwrap();
        assert_eq!(back, ks);
        assert_eq!(back.key_mode(), KeyMode::Split);

        // A short read is corruption, not a default.
        assert!(KeyState::read(&mut &buf[..3]).is_err());
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
        // The cross-layer LFO 2 link rides the same op channel (0217) and is
        // orthogonal to the key mode.
        assert!(!k.lfo2_link, "link is off by default");
        k.apply(KeyOp::SetLfo2Link(true));
        assert!(k.lfo2_link);
        k.apply(KeyOp::SetKeyMode(1));
        assert!(k.lfo2_link, "a mode change leaves the link alone");
        k.apply(KeyOp::SetLfo2Link(false));
        assert!(!k.lfo2_link);
    }

    // ── Layer pan (0248) ────────────────────────────────────────────────────

    /// The law itself: unity at centre, constant power across the sweep.
    #[test]
    fn pan_law_is_constant_power_with_unity_at_centre() {
        let (cl, cr) = pan_gains(0.0);
        assert!((cl - 1.0).abs() < 1e-6, "centre L must be unity: {cl}");
        assert!((cr - 1.0).abs() < 1e-6, "centre R must be unity: {cr}");

        // `gl² + gr²` constant everywhere — the whole point of the law. Centre
        // is 2.0 because of the unity normalisation (√2 on each channel).
        for pos in [-1.0_f32, -0.5, -0.25, 0.0, 0.25, 0.5, 1.0] {
            let (gl, gr) = pan_gains(pos);
            let power = gl * gl + gr * gr;
            assert!((power - 2.0).abs() < 1e-5, "power at {pos} is {power}, not 2.0");
        }

        // Hard left silences R (and vice versa), and the extreme channel takes
        // the √2 peak that constant power implies.
        let (hl_l, hl_r) = pan_gains(-1.0);
        assert!(hl_r.abs() < 1e-6, "hard left must silence R: {hl_r}");
        assert!((hl_l - core::f32::consts::SQRT_2).abs() < 1e-6);
        let (hr_l, hr_r) = pan_gains(1.0);
        assert!(hr_l.abs() < 1e-6, "hard right must silence L: {hr_l}");
        assert!((hr_r - core::f32::consts::SQRT_2).abs() < 1e-6);

        // Out-of-range positions clamp rather than wrapping round the circle.
        assert_eq!(pan_gains(-4.0), pan_gains(-1.0));
        assert_eq!(pan_gains(4.0), pan_gains(1.0));
    }

    /// Two layers panned apart land in opposite channels. Also the case the
    /// mono fast path used to swallow (0262): spread is 0, so the only thing
    /// decorrelating L and R is the pan.
    #[test]
    fn layers_panned_apart_land_in_opposite_channels() {
        let mut e = Engine::new(48_000.0, 512);
        e.set_layer2_on(true);
        for i in 0..2 {
            e.synths[i].set_param(ParamId::Env2Attack as usize, 0.001);
            e.synths[i].set_param(ParamId::Env2Sustain as usize, 1.0);
            // Spread 0: every voice lane sits centre, so any L/R difference in
            // the output is this ticket's doing.
            e.synths[i].set_param(ParamId::Spread as usize, 0.0);
        }
        e.set_param(clap_id_of(Layer::L1, ParamId::LayerPan), -1.0);
        e.set_param(clap_id_of(Layer::L2, ParamId::LayerPan), 1.0);
        // Give layer 2 a different pitch. With identical patches the two layers
        // emit the *same* waveform, so hard-left + hard-right would sum to
        // L == R and the stereo check below would pass on a mono engine too.
        e.set_param(clap_id_of(Layer::L2, ParamId::Osc1Coarse), 7.0);

        let (mut l, mut r) = (vec![0.0; 4096], vec![0.0; 4096]);
        // Settle the gain fades on silence before the note, as the level test
        // does. The fade is a one-pole, so the off-side channel approaches zero
        // asymptotically — two blocks put it far below the audible floor, which
        // is what the ratios below assert.
        e.process_block(&mut l, &mut r);
        e.process_block(&mut l, &mut r);
        let _ = MeterFrame::drain(e.meters());
        e.note_on(0, 60, 1.0);
        e.process_block(&mut l, &mut r);
        let f = MeterFrame::drain(e.meters());

        // Layer 1 hard left: its post-fader tap reads L only.
        assert!(f.layer1.0 > 0.0, "layer 1 must sound in L");
        assert!(
            f.layer1.1 < f.layer1.0 * 1e-3,
            "layer 1 hard left must be silent in R: {} vs L {}",
            f.layer1.1,
            f.layer1.0
        );
        // Layer 2 hard right: R only.
        assert!(f.layer2.1 > 0.0, "layer 2 must sound in R");
        assert!(
            f.layer2.0 < f.layer2.1 * 1e-3,
            "layer 2 hard right must be silent in L: {} vs R {}",
            f.layer2.0,
            f.layer2.1
        );
        // And the summed output is genuinely stereo. Before 0262 it was not:
        // `OutputStage`'s `spread_zero` hint skipped the R decimator and copied
        // L whenever every layer's Spread was 0, throwing this pan away.
        assert!(
            l.iter().zip(r.iter()).any(|(a, b)| (a - b).abs() > 1e-6),
            "a panned layer must reach the output — the mono fast path is gone"
        );
    }

    /// Centre pan is a true no-op: bit-identical to the same patch before pan
    /// existed, which is what the unity normalisation buys.
    #[test]
    fn centre_pan_leaves_the_channels_identical() {
        let mut e = Engine::new(48_000.0, 512);
        e.set_param(l1(ParamId::Env2Attack), 0.001);
        e.set_param(l1(ParamId::Spread), 0.0);
        e.note_on(0, 60, 1.0);
        let (mut l, mut r) = (vec![0.0; 2048], vec![0.0; 2048]);
        e.process_block(&mut l, &mut r);
        assert!(l.iter().any(|&s| s != 0.0), "the note must sound");
        assert_eq!(l, r, "spread 0 + centre pan must stay bit-mono");
    }

    /// A pan move is a fade, not a step — the same discipline as the mute fade,
    /// and the reason the *product* is smoothed rather than the position.
    #[test]
    fn panning_a_layer_does_not_step_the_output() {
        let settled = |pan: f32| {
            let mut e = Engine::new(48_000.0, 512);
            e.set_param(l1(ParamId::Env2Attack), 0.001);
            e.set_param(l1(ParamId::Env2Sustain), 1.0);
            e.set_param(l1(ParamId::LayerPan), pan);
            e.note_on(0, 60, 1.0);
            let (mut l, mut r) = (vec![0.0; 2048], vec![0.0; 2048]);
            e.process_block(&mut l, &mut r);
            e.process_block(&mut l, &mut r);
            l.windows(2).fold(0.0f32, |a, w| a.max((w[1] - w[0]).abs()))
        };
        // Reference: the same patch already sitting hard left, no move at all.
        let steady_step = settled(-1.0);

        let mut e = Engine::new(48_000.0, 512);
        e.set_param(l1(ParamId::Env2Attack), 0.001);
        e.set_param(l1(ParamId::Env2Sustain), 1.0);
        e.note_on(0, 60, 1.0);
        let (mut l, mut r) = (vec![0.0; 2048], vec![0.0; 2048]);
        e.process_block(&mut l, &mut r);
        // Slam centre → hard left between blocks: the worst case for a step.
        e.set_param(l1(ParamId::LayerPan), -1.0);
        e.process_block(&mut l, &mut r);
        let moved_step = l.windows(2).fold(0.0f32, |a, w| a.max((w[1] - w[0]).abs()));
        assert!(
            moved_step < steady_step * 1.5 + 1e-4,
            "pan move stepped: {moved_step} vs steady {steady_step}"
        );
    }

    /// Pan rides on top of level and mute rather than replacing them: a muted
    /// layer contributes nothing wherever it is placed.
    #[test]
    fn a_muted_layer_is_silent_at_any_pan() {
        let mut e = Engine::new(48_000.0, 512);
        e.set_param(l1(ParamId::Env2Attack), 0.001);
        e.set_param(l1(ParamId::Env2Sustain), 1.0);
        e.set_param(l1(ParamId::LayerPan), -1.0);
        e.set_param(l1(ParamId::LayerMute), 1.0);
        e.note_on(0, 60, 1.0);
        let (mut l, mut r) = (vec![0.0; 4096], vec![0.0; 4096]);
        // Two blocks: the first still carries the fade out of unity.
        e.process_block(&mut l, &mut r);
        e.process_block(&mut l, &mut r);
        let _ = MeterFrame::drain(e.meters());
        e.process_block(&mut l, &mut r);
        let f = MeterFrame::drain(e.meters());
        assert_eq!((f.layer1.0, f.layer1.1), (0.0, 0.0), "muted layer must be silent in both channels");
    }

    // ── Layer detune (0263) ─────────────────────────────────────────────────

    /// Count zero crossings as a cheap pitch proxy: a detuned layer completes
    /// more (or fewer) cycles in the same window than an undetuned one.
    fn zero_crossings(buf: &[f32]) -> usize {
        buf.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count()
    }

    /// A layer detuned sharp really is sharp — and by enough to hear, not just
    /// enough to measure.
    #[test]
    fn layer_detune_shifts_the_layers_pitch() {
        let render = |cents: f32| {
            let mut e = Engine::new(48_000.0, 4096);
            e.set_param(l1(ParamId::Env2Attack), 0.001);
            e.set_param(l1(ParamId::Env2Sustain), 1.0);
            // One oscillator only, so the crossing count reads its period.
            e.set_param(l1(ParamId::Osc2Level), 0.0);
            e.set_param(l1(ParamId::LayerDetune), cents);
            e.note_on(0, 69, 1.0); // A4
            let (mut l, mut r) = (vec![0.0; 4096], vec![0.0; 4096]);
            e.process_block(&mut l, &mut r);
            e.process_block(&mut l, &mut r);
            zero_crossings(&l)
        };
        let flat = render(-50.0);
        let centre = render(0.0);
        let sharp = render(50.0);
        assert!(flat < centre, "−50 ct must be flatter: {flat} vs {centre}");
        assert!(sharp > centre, "+50 ct must be sharper: {sharp} vs {centre}");
    }

    /// Detune moves the layer, not one oscillator inside it — the distinction
    /// from `Osc2Fine`. Both oscillators shift by the same amount, so the two
    /// stay in the relationship the patch set.
    #[test]
    fn layer_detune_moves_both_oscillators_together() {
        // Osc 2 alone, at its own octave: if detune only reached osc 1 this
        // would be unchanged by the sweep.
        let render_osc2_only = |cents: f32| {
            let mut e = Engine::new(48_000.0, 4096);
            e.set_param(l1(ParamId::Env2Attack), 0.001);
            e.set_param(l1(ParamId::Env2Sustain), 1.0);
            e.set_param(l1(ParamId::Osc1Level), 0.0);
            e.set_param(l1(ParamId::Osc2Level), 1.0);
            e.set_param(l1(ParamId::Osc2Octave), 0.0);
            e.set_param(l1(ParamId::LayerDetune), cents);
            e.note_on(0, 69, 1.0);
            let (mut l, mut r) = (vec![0.0; 4096], vec![0.0; 4096]);
            e.process_block(&mut l, &mut r);
            e.process_block(&mut l, &mut r);
            zero_crossings(&l)
        };
        assert!(
            render_osc2_only(50.0) > render_osc2_only(-50.0),
            "osc 2 must follow the layer detune too"
        );
    }

    /// Each layer's detune is its own — the point of the control is beating
    /// *between* layers, which needs them to move independently.
    #[test]
    fn layer_detune_is_per_layer() {
        let mut e = Engine::new(48_000.0, 4096);
        e.set_layer2_on(true);
        for i in 0..2 {
            e.synths[i].set_param(ParamId::Env2Attack as usize, 0.001);
            e.synths[i].set_param(ParamId::Env2Sustain as usize, 1.0);
        }
        // Only layer 1 is detuned.
        e.set_param(clap_id_of(Layer::L1, ParamId::LayerDetune), 50.0);
        assert_eq!(
            e.synths[1].params().get(ParamId::LayerDetune),
            0.0,
            "detune must not leak to the other layer — it is a patch param, not a global"
        );
        assert_eq!(e.synths[0].params().get(ParamId::LayerDetune), 50.0);

        // And the two layers now beat against each other where they did not
        // before: the summed envelope of the same note is no longer steady.
        e.note_on(0, 69, 1.0);
        let (mut l, mut r) = (vec![0.0; 4096], vec![0.0; 4096]);
        e.process_block(&mut l, &mut r);
        e.process_block(&mut l, &mut r);
        let first = l[..1024].iter().fold(0.0f32, |a, &s| a.max(s.abs()));
        let later = l[3072..].iter().fold(0.0f32, |a, &s| a.max(s.abs()));
        assert!(
            (first - later).abs() > 1e-3,
            "two layers 50 ct apart must beat: {first} vs {later}"
        );
    }

    /// The detune taper is the reason the control is usable: half travel each
    /// way is ±20 ct, not the ±25 ct a linear slider would give.
    #[test]
    fn layer_detune_taper_puts_20_cents_at_half_travel() {
        let d = ParamId::LayerDetune.desc();
        assert_eq!(d.from_fader(0.5), 0.0);
        assert!((d.from_fader(0.75) - 20.0).abs() < 1e-3, "{}", d.from_fader(0.75));
        assert!((d.from_fader(0.25) + 20.0).abs() < 1e-3, "{}", d.from_fader(0.25));
        assert!((d.from_fader(1.0) - 50.0).abs() < 1e-3);
        assert!((d.from_fader(0.0) + 50.0).abs() < 1e-3);
    }

    #[test]
    fn default_key_state_is_single_middle_c() {
        let ks = KeyState::default();
        assert!(!ks.layer2_on);
        assert!(!ks.split_enabled);
        assert_eq!(ks.split_point, DEFAULT_SPLIT_POINT);
        assert_eq!(ks.split_point, 60);
        assert!(!ks.lfo2_link, "the cross-layer LFO 2 link is off by default");
        assert_eq!(ks.key_mode(), KeyMode::Single);
    }
}
