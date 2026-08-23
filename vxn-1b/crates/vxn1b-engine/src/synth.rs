//! The `Synth` — VXN1b's core synth as an **instantiable unit** (0214, ADR 0002
//! §1). One `Synth` owns its own voice pool, allocator/stealing, stack voicing,
//! patch params, mod matrix, per-layer LFO 2, and [`Synth::BANKS`] 8-wide
//! [`RenderBank`]s (32 lanes in 4 banks since 0264 — banks are an SoA split for
//! stereo decorrelation, *not* layers). The plugin holds
//! **2 × `Synth` + a global block** ([`crate::engine::Engine`]) that owns FX,
//! master, and (later) mixer/demux.
//!
//! Allocation and stealing are **private to each synth** — no shared pool, no
//! `param_source` indirection (contrast VXN1). A `Synth` renders its voices into
//! the caller's buffer **accumulating**; the global block pre-zeroes, ticks each
//! active synth, then runs the one global FX chain + master over the sum.

use vxn_dsp::{CONTROL_BLOCK, LfoCore};

use crate::bank::{BlockCtx, RenderBank, TriggerOpts};
use crate::matrix::MatrixTable;
use crate::params::{CrossModType, ParamId, Params};
use crate::state::LayerState;
use crate::voice::{Triggers, Voices, WIDE_GLIDE_SCALE};

/// Per-synth seed set: one RNG seed per render bank (distinct so a synth's
/// banks' noise/drift streams decorrelate) plus its LFO 2 seed. The two synths
/// get **distinct** sets so that two layers on the *same* patch still
/// decorrelate (they don't phase-lock).
pub(crate) struct SynthSeeds {
    pub banks: [u64; Synth::BANKS],
    pub lfo2: u64,
}

impl SynthSeeds {
    /// Layer 1 seeds — the first two are identical to the pre-dual-layer
    /// engine's, so lanes 0–15 stay byte-for-byte what they were before the
    /// 32-lane widening (0264).
    pub(crate) const LAYER1: SynthSeeds = SynthSeeds {
        banks: [0x1b_0000_0001, 0x1b_0000_0002, 0x1b_0000_0003, 0x1b_0000_0004],
        lfo2: 0x1b_0000_00f2,
    };
    /// Layer 2 seeds — distinct streams so a duplicated patch decorrelates.
    pub(crate) const LAYER2: SynthSeeds = SynthSeeds {
        banks: [0x1b_0000_0011, 0x1b_0000_0012, 0x1b_0000_0013, 0x1b_0000_0014],
        lfo2: 0x1b_0000_01f2,
    };
}

/// One instantiable VXN1b synth: voices + patch + matrix + per-layer LFO 2 over
/// [`Synth::BANKS`] render banks.
pub struct Synth {
    sample_rate: f32,
    params: Params,
    matrix: MatrixTable,
    voices: Voices,
    banks: [RenderBank; Synth::BANKS],
    /// This layer's LFO 2 (VXN1b has no *global* LFO — ADR 0002 §4), ticked once
    /// per control block and broadcast to every bank.
    lfo2: LfoCore,
    /// Host pitch-bend in `[-1, 1]` — the hardwired bend term (ADR §3) *and* the
    /// PitchWheel matrix source.
    pitch_bend: f32,
    /// Mod wheel `[0, 1]` — the ModWheel matrix source.
    mod_wheel: f32,
    /// Host tempo in BPM, pushed down from the engine each block (0267). Only
    /// read when an LFO's sync toggle is on; [`sync::DEFAULT_TEMPO_BPM`] until
    /// a host supplies one, so a synced LFO in a tempo-less host still runs.
    tempo_bpm: f32,
}

impl Synth {
    /// Render banks per synth. [`Voices::CAPACITY`] lanes split into
    /// [`RenderBank::LANES`]-wide SoA banks; the division is exact by
    /// construction and asserted in `tests::lane_pool_divides_into_banks`.
    pub const BANKS: usize = Voices::CAPACITY / RenderBank::LANES;

    /// Build a synth from a decoded patch (`params` + matrix topology) with the
    /// given per-layer seed set. Cooks envelopes.
    pub(crate) fn new(sample_rate: f32, state: LayerState, seeds: &SynthSeeds) -> Self {
        let control_rate = sample_rate / CONTROL_BLOCK as f32;
        let mut synth = Self {
            sample_rate,
            params: state.params,
            matrix: state.matrix,
            voices: Voices::new(),
            banks: core::array::from_fn(|b| RenderBank::new(sample_rate, seeds.banks[b])),
            lfo2: LfoCore::new(control_rate, seeds.lfo2),
            pitch_bend: 0.0,
            mod_wheel: 0.0,
            tempo_bpm: crate::sync::DEFAULT_TEMPO_BPM,
        };
        synth.apply_envelopes();
        synth
    }

    /// Overwrite the whole patch (params + matrix topology) from a decoded
    /// [`LayerState`]. Re-cooks envelopes. Depth stays param-authoritative: the
    /// topology's depths already mirror the params (the codec seeds them).
    pub(crate) fn load_state(&mut self, state: LayerState) {
        self.params = state.params;
        self.matrix = state.matrix;
        self.apply_envelopes();
    }

    /// Read a CLAP-id param value (identity map).
    #[inline]
    pub(crate) fn param(&self, id: usize) -> f32 {
        self.params.get_index(id)
    }

    /// This synth's param table (for the global block to read the transitionally
    /// shared FX / master params, and for tests).
    pub(crate) fn params(&self) -> &Params {
        &self.params
    }

    /// This synth's matrix topology (preset load / tests).
    pub(crate) fn matrix_mut(&mut self) -> &mut MatrixTable {
        &mut self.matrix
    }

    #[cfg(test)]
    pub(crate) fn matrix(&self) -> &MatrixTable {
        &self.matrix
    }

    /// Set a CLAP-id param (identity map). Envelope params re-cook the banks;
    /// slot-depth params mirror into the matrix the evaluator reads (0205).
    pub(crate) fn set_param(&mut self, id: usize, value: f32) {
        self.params.set_index(id, value);
        if recooks_envelopes(id) {
            self.apply_envelopes();
        }
        if let Some(slot) = ParamId::slot_depth_index(id) {
            // Read back the clamped value so the mirror can't drift from the param.
            self.matrix.slots[slot].depth = self.params.slot_depth(slot);
        }
    }

    pub(crate) fn set_pitch_bend(&mut self, bend: f32) {
        self.pitch_bend = bend.clamp(-1.0, 1.0);
    }

    pub(crate) fn set_mod_wheel(&mut self, w: f32) {
        self.mod_wheel = w.clamp(0.0, 1.0);
    }

    /// Host tempo for the synced LFO rates (0267). Non-finite / non-positive
    /// BPM is ignored — a host that reports garbage must not stall the LFOs.
    pub(crate) fn set_tempo(&mut self, bpm: f32) {
        if bpm.is_finite() && bpm > 0.0 {
            self.tempo_bpm = bpm;
        }
    }

    /// Transport stop→play: realign this layer's LFO 2 to the bar grid so a
    /// synced rhythmic shape locks to the host beat. Reset to the cycle
    /// boundary (phase 0), not the zero crossing — saw-down should hit its peak
    /// transient on the beat. No-op when LFO 2 isn't synced: a free LFO must not
    /// jump on play.
    pub(crate) fn on_transport_restart(&mut self) {
        if self.params.bool(ParamId::Lfo2Sync) {
            self.lfo2.reset();
        }
    }

    /// Note-on: allocate a `stack_width`-lane stack under the patch's voice
    /// mode (MPE channel threaded) and trigger every lane it placed.
    /// Allocation/stealing are private to this synth. Returns the voice the note
    /// sounds on: the first triggered lane, or lane 0 when a legato slide
    /// triggered none (a slide only happens in Solo, which pins lane 0).
    pub(crate) fn note_on(&mut self, channel: u8, note: u8, velocity: f32) -> usize {
        let width = self.params.stack_width().lanes();
        let mode = self.params.voice_mode();
        let detune = self.params.get(ParamId::UnisonDetune);
        let legato = self.params.bool(ParamId::Legato);
        let triggers =
            self.voices
                .note_on_stack(channel, note, velocity, width, mode, detune, legato);
        self.fire(&triggers);
        triggers.as_slice().first().map_or(0, |t| t.voice)
    }

    pub(crate) fn note_off(&mut self, channel: u8, note: u8) {
        let width = self.params.stack_width().lanes();
        let mode = self.params.voice_mode();
        let detune = self.params.get(ParamId::UnisonDetune);
        let legato = self.params.bool(ParamId::Legato);
        // Mono modes revert to the highest-priority note still held, which can
        // re-trigger the stack — hence a trigger list on note-*off* too.
        let triggers = self
            .voices
            .note_off_stack(channel, note, width, mode, detune, legato);
        self.fire(&triggers);
    }

    /// Trigger the DSP lanes an allocation asked for, routing each 16-voice
    /// index to its (bank, lane) pair.
    fn fire(&mut self, triggers: &Triggers) {
        let opts = TriggerOpts {
            lfo1_shape: self.params.lfo1_shape(),
            lfo1_free_run: self.params.bool(ParamId::Lfo1FreeRun),
            osc_free_run: [
                self.params.bool(ParamId::Osc1FreeRun),
                self.params.bool(ParamId::Osc2FreeRun),
            ],
        };
        for t in triggers.as_slice() {
            let (bank, lane) = (t.voice / RenderBank::LANES, t.voice % RenderBank::LANES);
            self.banks[bank].trigger_lane(lane, opts, t.start_phase);
        }
    }

    /// Test helper: is this synth still holding (pressed, un-released) `note`?
    /// The demux tests use it to check note-offs land on the owning synth (0215).
    #[cfg(test)]
    pub(crate) fn voices_holding(&self, note: u8) -> bool {
        self.voices.is_holding(note)
    }

    pub(crate) fn poly_pressure(&mut self, channel: u8, note: u8, value: f32) {
        self.voices.poly_pressure(channel, note, value);
    }

    pub(crate) fn channel_pressure(&mut self, channel: u8, value: f32) {
        self.voices.channel_pressure(channel, value);
    }

    pub(crate) fn reset(&mut self) {
        self.voices.reset();
        for b in &mut self.banks {
            b.reset();
        }
    }

    /// This layer's LFO 2 phase after the last control-block tick. The global
    /// block reads Layer 1's to drive Layer 2's LFO 2 link (0217, ADR 0002 §5).
    #[inline]
    /// Whether every voice in this synth is idle — the engine folds both
    /// synths' answers into the decimator's drain-skip (0249). Cheap: a scan of
    /// the 16 active flags, once per control block.
    pub(crate) fn is_silent(&self) -> bool {
        !(0..Voices::CAPACITY).any(|v| self.voices.is_active(v))
    }

    pub(crate) fn lfo2_phase(&self) -> f32 {
        self.lfo2.phase()
    }

    /// Render one ≤`CONTROL_BLOCK` control block, **accumulating** this synth's
    /// voices into `l`/`r` (the global block pre-zeroes and may tick a second
    /// synth on top). Ticks this layer's LFO 2, builds the block context, renders
    /// both banks. No FX or master here — those are global (ADR §7).
    ///
    /// `lfo2_link` is the **master LFO 2 phase** to slave to (0217): `Some(p)`
    /// makes this layer's LFO 2 adopt `p` instead of running its own accumulator
    /// — rate *and* phase lock — while its shape stays its own; `None` (always,
    /// for Layer 1) free-runs from this layer's own patch settings.
    /// `l`/`r` are the **oversampled** buses: `l.len() == base_frames · os`.
    /// The banks derive their base frame count from that length, so the caller
    /// owns the factor and this just passes it into the block context (0249).
    pub(crate) fn render_control_block(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        lfo2_link: Option<f32>,
        os: usize,
    ) {
        // LFO 2: one tick per control block, broadcast to both banks.
        let lfo2_val = match lfo2_link {
            Some(master_phase) => self.lfo2.sync_to(master_phase, self.params.lfo2_shape()),
            None => {
                self.lfo2.set_rate(crate::sync::lfo_rate_hz(
                    &self.params,
                    ParamId::Lfo2Rate,
                    ParamId::Lfo2Sync,
                    self.tempo_bpm,
                ));
                self.lfo2.next(self.params.lfo2_shape())
            }
        };

        let ctx = build_ctx(
            &self.params,
            &self.matrix,
            self.sample_rate,
            os,
            self.pitch_bend,
            self.mod_wheel,
            lfo2_val,
            self.voices.level_comp(),
            self.tempo_bpm,
        );

        // One pass per bank over `LANES`-wide slices of the render view. `active`
        // is the only `&mut` field, so it is chunked rather than sliced; every
        // bank sums into the same `l`/`r`. Banks with no live lane take
        // `RenderBank::render`'s `is_silent` early-out, so widening the pool
        // costs idle blocks nothing.
        let view = self.voices.render_view();
        let lanes = RenderBank::LANES;
        for (b, active) in view.active.chunks_mut(lanes).enumerate() {
            let s = b * lanes;
            let e = s + lanes;
            self.banks[b].render(
                &ctx,
                &view.note[s..e],
                &view.gate[s..e],
                active,
                &view.velocity[s..e],
                &view.pressure[s..e],
                &view.note_random[s..e],
                &view.detune_cents[s..e],
                &view.stack_pos[s..e],
                l,
                r,
            );
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
        // Drift scales the per-lane envelope trims (0218), so a drift change
        // re-cooks here just like an envelope param change.
        let drift = p.get(ParamId::MasterDrift);
        for b in &mut self.banks {
            b.set_envelopes(env1, s1, env2, s2, drift);
        }
    }
}

/// Assemble the mod-agnostic block context from the current params. A free
/// function (not a `&self` method) so the returned [`BlockCtx`] borrows **only**
/// `matrix` — every scalar is copied out of `params` — leaving `voices` and
/// `banks` independently mutable during render. `os` is the engine's global
/// oversampling factor (0249): the banks run their inner loop `os` times per base
/// frame at `os_sample_rate`, and the engine decimates the result.
fn build_ctx<'a>(
    p: &Params,
    matrix: &'a MatrixTable,
    sample_rate: f32,
    os: usize,
    pitch_bend: f32,
    mod_wheel: f32,
    lfo2_val: f32,
    level_comp: f32,
    tempo_bpm: f32,
) -> BlockCtx<'a> {
    let (sync, pm_index, ring_mode) = match p.cross_mod_type() {
        CrossModType::Off => (false, 0.0, false),
        CrossModType::Sync => (true, 0.0, false),
        CrossModType::Pm => (false, p.get(ParamId::CrossModAmount), false),
        CrossModType::Ring => (false, 0.0, true),
    };
    // Hardwired pitch bend (ADR §3): global pitch += bend × range. Layer detune
    // (0263) joins the same base, so it moves both oscillators and the sub as
    // one — a layer detuned against its partner, not an oscillator detuned
    // against its neighbour (that is `Osc2Fine`) or a lane against its own
    // voice (that is `UnisonDetune`).
    let base_semis = p.get(ParamId::MasterTune)
        + pitch_bend * p.get(ParamId::PitchBendRange)
        + p.get(ParamId::LayerDetune) * 0.01;
    BlockCtx {
        os_sample_rate: sample_rate * os as f32,
        os,
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
        filter_key_track: p.get(ParamId::FilterKeyTrack),
        hpf_cutoff: p.get(ParamId::HpfCutoff),
        resonance: p.get(ParamId::Resonance),
        drive: p.get(ParamId::Drive),
        filter_mode: p.filter_mode(),
        filter_slope: p.filter_slope(),
        base_semis,
        lfo1_shape: p.lfo1_shape(),
        lfo1_rate_hz: crate::sync::lfo_rate_hz(
            p,
            ParamId::Lfo1Rate,
            ParamId::Lfo1Sync,
            tempo_bpm,
        ),
        lfo1_delay_time: p.get(ParamId::Lfo1DelayTime),
        lfo1_fade: p.get(ParamId::Lfo1Fade),
        lfo2_val,
        // A detuned stack slides as one and reads far stronger than a single
        // Poly voice, so the stacked modes take a fraction of the knob's glide
        // time — a scoop rather than an audible portamento (VXN1).
        portamento_time: p.get(ParamId::PortamentoTime)
            * glide_scale(p.stack_width().lanes()),
        amp_env_bypass: p.bool(ParamId::AmpEnvBypass),
        drift_amount: p.get(ParamId::MasterDrift),
        spread: p.get(ParamId::Spread),
        level_comp,
        matrix,
        mod_wheel,
        pitch_wheel: pitch_bend,
    }
}

/// Portamento scaling for a stacked patch — see [`WIDE_GLIDE_SCALE`]. Keyed on
/// width rather than on a mode name: a stack slides as one body whichever way
/// the keyboard is being played, and a single lane has nothing to thicken.
#[inline]
fn glide_scale(width: usize) -> f32 {
    if width > 1 { WIDE_GLIDE_SCALE } else { 1.0 }
}

/// Quantised per-osc tuning in semitones (VXN1): `round(octave)·12 +
/// round(coarse) + fine/100`.
#[inline]
fn osc_semis(p: &Params, octave: ParamId, coarse: ParamId, fine: ParamId) -> f32 {
    p.get(octave).round() * 12.0 + p.get(coarse).round() + p.get(fine) / 100.0
}

/// Whether a CLAP id is one of the ten ADSR value/shape params — or
/// [`ParamId::MasterDrift`], which scales the per-lane envelope trims (0218) —
/// so a set re-cooks the banks' envelopes.
fn recooks_envelopes(id: usize) -> bool {
    matches!(
        ParamId::from_index(id),
        Some(
            ParamId::MasterDrift
                | ParamId::Env1Attack
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

    fn factory_synth(seeds: &SynthSeeds) -> Synth {
        Synth::new(48_000.0, LayerState::factory_default(), seeds)
    }

    /// Two `Synth`s with **distinct patches** render **distinct output** from the
    /// same note stream — proving per-synth independence (no shared pool / param
    /// source). Acceptance for 0214.
    #[test]
    fn two_synths_distinct_patches_render_distinct_output() {
        let mut a = factory_synth(&SynthSeeds::LAYER1);
        let mut b = factory_synth(&SynthSeeds::LAYER2);
        // Fast attack on both so the VCA opens inside the first block.
        a.set_param(ParamId::Env2Attack as usize, 0.001);
        b.set_param(ParamId::Env2Attack as usize, 0.001);
        // Distinct patch: shift synth B up two octaves + different cutoff.
        b.set_param(ParamId::Osc1Octave as usize, 2.0);
        b.set_param(ParamId::Cutoff as usize, 8_000.0);

        // Same note stream to both.
        a.note_on(0, 60, 1.0);
        b.note_on(0, 60, 1.0);

        let (mut la, mut ra) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        let (mut lb, mut rb) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        a.render_block(&mut la, &mut ra);
        b.render_block(&mut lb, &mut rb);

        assert!(la.iter().any(|&s| s != 0.0), "synth A must sound");
        assert!(lb.iter().any(|&s| s != 0.0), "synth B must sound");
        assert!(
            la.iter().zip(&lb).any(|(x, y)| (x - y).abs() > 1e-6),
            "distinct patches must render distinct output"
        );
    }

    /// Allocation and stealing are private to a synth: a full pool of held notes
    /// fills all of *this* synth's banks; the next steals its own voice 0.
    #[test]
    fn stealing_is_per_synth() {
        let mut s = factory_synth(&SynthSeeds::LAYER1);
        for i in 0..Voices::CAPACITY {
            s.note_on(0, 24 + i as u8, 1.0);
        }
        assert_eq!(
            s.note_on(0, 120, 1.0),
            0,
            "the note past capacity steals this synth's voice 0"
        );
    }

    /// Widening the pool must not cost idle blocks anything: with only lane 0
    /// sounding, banks 1–3 hold no active lane and take `RenderBank`'s
    /// `is_silent` early-out (0264).
    #[test]
    fn banks_past_the_sounding_one_stay_inactive() {
        let mut s = factory_synth(&SynthSeeds::LAYER1);
        s.note_on(0, 60, 1.0);
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        s.render_block(&mut l, &mut r);
        assert!(l.iter().any(|&x| x != 0.0), "the held note must sound");
        for v in RenderBank::LANES..Voices::CAPACITY {
            assert!(!s.voices.is_active(v), "lane {v} must be idle");
        }
    }

    /// Test helper: zero the buffer then render one control block (a `Synth`
    /// accumulates, so tests must pre-zero as the global block does).
    impl Synth {
        fn render_block(&mut self, l: &mut [f32], r: &mut [f32]) {
            let mut off = 0;
            while off < l.len() {
                let n = (l.len() - off).min(CONTROL_BLOCK);
                l[off..off + n].fill(0.0);
                r[off..off + n].fill(0.0);
                self.render_control_block(&mut l[off..off + n], &mut r[off..off + n], None, 1);
                off += n;
            }
        }
    }
}
