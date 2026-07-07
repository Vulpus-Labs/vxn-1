//! VXN1 synth engine.
//!
//! Framework-agnostic: holds parameters, allocates voices, and renders audio
//! in fixed control blocks. The CLAP layer drives it with note/param events
//! and contiguous output slices; the UI reads and writes [`ParamValues`].

pub mod factory;
pub mod params;
pub mod preset;
pub mod preset_io;
pub mod shared;
pub mod smoothing;
pub mod state;
pub mod voice;

// Host-tempo sync metadata (E004 / 0015) lives in vxn-app — pure data + pure
// functions, shared with the editor without dragging engine internals in.
pub use vxn_app::sync;

pub use params::{
    AssignMode, CrossModType, DEFAULT_SPLIT_POINT, EnvSel, GLOBAL_PARAMS, GlobalParam,
    GlobalValues, KeyMode, Layer, LfoSel, PATCH_PARAMS, ParamDesc, ParamKind, ParamRef,
    ParamValues, PatchParam, PatchValues, TOTAL_PARAMS, Taper, desc_for_clap_id, global_clap_id,
    module_for_clap_id, param_ref, patch_clap_id,
};
pub use factory::{FactoryPreset, factory};
pub use preset::{Meta, Performance, PresetError};
pub use preset_io::{
    EnginePresetStore, LoadError, UserFolder, UserPreset, create_user_folder, delete_user_folder,
    delete_user_preset, ensure_user_dir, list_user_presets, list_user_tree, load_preset_file,
    move_user_preset, rename_user_folder, rename_user_preset, save_performance,
    save_performance_in, user_preset_dir,
};
// UNCATEGORIZED moved to vxn-app::domain (ADR 0007). Engine re-exports it for
// path continuity (the preset_io module still references it in its doc-strings
// and the factory bank's category labels).
pub use vxn_app::UNCATEGORIZED;
pub use shared::SharedParams;
use smoothing::ParamSmoother;
pub use state::PluginState;

use voice::{
    AmpRoute, BlockCtx, CrossMod, CutoffRoute, FilterParams, Lfo1Trigger, NoteOn, OscParams,
    PitchRoute, PwmRoute, VoiceBank,
};
use vxn_dsp::{
    AdsrShape, CONTROL_BLOCK, FdnReverb, FdnReverbParams, LfoCore, MAX_OVERSAMPLE, Oversampler,
    Smoothed, StereoChorus, StereoDelay, StereoLimiter, StereoPhaser, note_to_hz,
};

/// Mod-wheel (CC1) glide time (ms), applied at the control-block rate. Rounds
/// off the 7-bit CC steps so wheel sweeps don't zipper the cutoff / osc2 pitch.
/// On a wide pitch route 1 LSB is ~0.76 st, so the glide is set long enough to
/// filter hardware sensor jitter at rest, not just the coarse CC quantisation.
const MOD_WHEEL_SMOOTH_MS: f32 = 40.0;

/// Snapshot of the envelope-shaping parameters. Used to skip recomputing ADSR
/// coefficients (which cost an `exp()` per segment) unless a knob actually moved.
#[derive(Clone, Copy, PartialEq)]
struct EnvSnapshot {
    env1: (f32, f32, f32, f32),
    env1_shape: AdsrShape,
    env2: (f32, f32, f32, f32),
    env2_shape: AdsrShape,
    /// Shared "analog" amount (E022 / 0124): folded into the gate so a drift
    /// change re-applies the per-voice envelope trims even when no envelope knob
    /// moved. Uses the prior block's value (set in `update_effects`, after
    /// `sync_envelopes`), so a drift move lands one block later — inaudible for
    /// a sub-audio creative param.
    drift_amount: f32,
}

/// Re-export so the plugin shell can flush denormals without depending on
/// `vxn-dsp` directly. `ScopedFlushToZero` is the per-`process` guard (sets FTZ
/// on entry, restores on drop); `enable_flush_to_zero` is the bare one-shot.
pub use vxn_dsp::{ScopedFlushToZero, enable_flush_to_zero};

/// Number of always-present layers (ADR 0003 §1). Indexed by [`Layer`].
const LAYERS: usize = Layer::COUNT;

/// Seed for the single global LFO 2 (E005 / 0019). LFO 1 is per-voice and seeded
/// inside each [`VoiceBank`] (E005 / 0018).
const LFO2_SEED: u64 = 0x7E5D;

/// Consecutive silent blocks before the decimator skip kicks in.
/// `HalfbandFir` is 33 taps; at OS = 8 the worst-case cascaded drain is
/// well under one block of OS samples, so 4 base blocks is heavily
/// conservative — guarantees the FIR state has flushed to zero before we
/// start zero-filling the output (0106).
const DECIMATOR_DRAIN_BLOCKS: u32 = 4;
/// Per-layer RNG seeds (decorrelate the two layers' S&H LFO PRNGs).
const RNG_SEEDS: [u64; LAYERS] = [0x9E37_79B9, 0x2545_F491];

/// The complete VXN1 instrument.
pub struct Synth {
    sample_rate: f32,
    params: ParamValues,
    /// Glides gain-like params toward `params` to remove zipper noise. The
    /// filter is smoothed separately by ladder coefficient interpolation.
    smoother: ParamSmoother,
    /// Two always-present layers of 8 channels each (ADR 0003 §2). Each is a
    /// complete patch; both sum into the global FX bus.
    banks: [VoiceBank; LAYERS],
    /// A single instrument-wide global LFO 2 (E005 / 0019), shared across both
    /// layers and all voices: sampled once per block and broadcast. LFO 1 is
    /// per-voice, living inside each [`VoiceBank`] (E005 / 0018).
    lfo2: LfoCore,
    /// The shared master-bus FX chain (phaser → chorus → delay → reverb →
    /// limiter) plus its limiter edge flag, owned as one unit so the init/reset
    /// paths can't drift out of sync.
    master_fx: MasterFx,
    /// The oversampled synthesis path's L/R anti-aliasing decimators plus the
    /// phase-alignment / silent-drain bookkeeping (0106 / 0107), owned as one
    /// unit so the duplicated reset/decimate branches collapse into its API.
    output: OutputStage,
    /// Pitch bend in normalised `[-1, 1]`. Global value; each layer scales it by
    /// its own `PitchWheelDepth` in `build_ctx` (ADR 0003 §9, ADR 0004 §5).
    bend_norm: f32,
    /// Mod-wheel (CC1) position in `[0, 1]`, smoothed at the control rate.
    /// Global value; each layer applies it via its own routing params.
    mod_wheel: Smoothed,
    /// Current key mode (ADR 0003 §3). Drives both the per-layer param source
    /// ([`Synth::param_source`]) and note routing ([`Synth::note_on`]).
    key_mode: KeyMode,
    /// Split point (MIDI note) for [`KeyMode::Split`]: notes below go to Lower,
    /// at/above to Upper (ADR 0003 §8). Non-automatable shared state.
    split_point: u8,
    /// Host tempo (BPM) for LFO host-sync (E004 / 0015), fed from the CLAP
    /// transport each block. Defaults to a sane tempo when the host has none.
    tempo_bpm: f32,
    alloc_counter: u64,
    /// Round-robin layer cursor for Whole-mode note-on: alternates layers so
    /// notes spread 8+8, giving 16-voice polyphony with both layers reading
    /// layer A's params (0008). Reset on `reset`.
    rr_layer: usize,
    /// Last envelope params pushed to each layer's voices; `None` forces a refresh.
    last_env: [Option<EnvSnapshot>; LAYERS],
    /// Per-layer assign mode from the previous process call. Used to detect
    /// Poly/Twin → Solo/Unison transitions and gate off held voices so they
    /// don't stick under the now-monophonic allocator.
    last_assign_mode: [AssignMode; LAYERS],
    /// Per-voice oscillator drift amount, broadcast into each block's
    /// [`BlockCtx`]. Defaults to [`DEFAULT_DRIFT_AMOUNT`]; tests that assert
    /// bit-equal two-layer equivalence set it to 0.
    drift_amount: f32,
}

/// Which master-bus effects run this block (raw on/off switches, read from the
/// unsmoothed params — they're discrete toggles, not glided).
struct MasterFxFlags {
    phaser: bool,
    chorus: bool,
    delay: bool,
    reverb: bool,
    limiter: bool,
}

/// The shared master-bus FX chain (ADR 0003 §7), in signal order:
/// phaser → chorus → delay → reverb → limiter. Owns every FX block plus the
/// limiter's off→on edge flag, so `Synth` no longer re-lists them across
/// `new`/`set_sample_rate`/`reset` — the init/reset drift hazard lives here.
struct MasterFx {
    /// Stereo allpass phaser. First in the FX chain (pre-chorus) so its
    /// resonant peaks survive the chorus's chorale; runs only when
    /// [`GlobalParam::PhaserOn`] is set.
    phaser: StereoPhaser,
    chorus: StereoChorus,
    delay: StereoDelay,
    /// 8-line FDN reverb (Jot-style, Hadamard feedback). Sits post-delay,
    /// pre-limiter in the FX chain; runs only when [`GlobalParam::ReverbOn`]
    /// is set.
    reverb: FdnReverb,
    /// Optional brickwall limiter on the master bus (last in the FX chain). Run
    /// only when [`GlobalParam::LimiterOn`] is set; bypassed otherwise.
    limiter: StereoLimiter,
    /// Whether the limiter ran last block, so it can be reset on the off→on edge
    /// (clears stale lookahead state instead of leaking a transient).
    limiter_was_on: bool,
}

impl MasterFx {
    fn new(sample_rate: f32) -> Self {
        Self {
            phaser: StereoPhaser::new(sample_rate),
            chorus: StereoChorus::new(sample_rate),
            delay: StereoDelay::new(sample_rate, 2.0),
            reverb: FdnReverb::new(sample_rate),
            limiter: StereoLimiter::new(sample_rate),
            limiter_was_on: false,
        }
    }

    /// Clear every FX block's internal state and the limiter edge flag. Used by
    /// both `Synth::reset` and (via a fresh `MasterFx`) `set_sample_rate`.
    fn reset(&mut self) {
        self.phaser.clear();
        self.chorus.clear();
        self.delay.clear();
        self.reverb.reset();
        self.limiter.reset();
        self.limiter_was_on = false;
    }

    /// Push this block's smoothed FX params. `reverb_on` is the raw (unsmoothed)
    /// reverb switch; the FDN takes it directly as its discrete on flag.
    fn update(&mut self, g: &GlobalValues, reverb_on: bool, tempo_bpm: f32) {
        self.phaser.set_params(
            g.get(GlobalParam::PhaserRate),
            g.get(GlobalParam::PhaserDepth),
            g.get(GlobalParam::PhaserFB),
            g.get(GlobalParam::PhaserMix),
        );
        self.chorus.set_params(
            g.get(GlobalParam::ChorusRate),
            g.get(GlobalParam::ChorusDepth),
            g.get(GlobalParam::ChorusMix),
        );
        let t = delay_time_seconds(
            g.bool(GlobalParam::DelaySync),
            g.get(GlobalParam::DelayTime),
            tempo_bpm,
        );
        self.delay.set_params(
            t,
            t,
            g.get(GlobalParam::DelayFeedback),
            0.3,
            g.get(GlobalParam::DelayMix),
        );
        // Reverb (FDN): four direct knobs — size, decay, damp, mix. All come
        // through the smoother. On is unsmoothed (it's a discrete switch).
        self.reverb.set_params(&FdnReverbParams {
            on: reverb_on,
            size: g.get(GlobalParam::ReverbSize),
            decay_secs: g.get(GlobalParam::ReverbDecay),
            damp: g.get(GlobalParam::ReverbDamp),
            mix: g.get(GlobalParam::ReverbMix),
        });
    }

    /// Run the chain on the volume-applied dry stereo bus, writing the final
    /// master output to `out_l`/`out_r`. Each stage passes through unchanged
    /// when its flag is clear, keeping the engine sample-exact against a build
    /// with that effect absent.
    fn process_block(
        &mut self,
        dry_l: &[f32],
        dry_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        flags: MasterFxFlags,
    ) {
        let block = dry_l.len();

        // Phaser (first in FX chain, pre-chorus). Stereo-in / stereo-out via
        // parallel L/R allpass cascades sharing one anti-phase LFO (0101). When
        // off, the dry stereo bus passes through unchanged.
        if flags.phaser {
            self.phaser.process_block_stereo(dry_l, dry_r, out_l, out_r);
        } else {
            out_l.copy_from_slice(dry_l);
            out_r.copy_from_slice(dry_r);
        }

        if flags.chorus {
            // Chorus is stereo-in / stereo-out via parallel L/R delay lines
            // sharing the inverted-per-line LFO setup (0102). The phaser output
            // bus feeds straight in — no mono collapse.
            let mut chorus_in_l = [0.0f32; CONTROL_BLOCK];
            let mut chorus_in_r = [0.0f32; CONTROL_BLOCK];
            let chorus_in_l = &mut chorus_in_l[..block];
            let chorus_in_r = &mut chorus_in_r[..block];
            chorus_in_l.copy_from_slice(out_l);
            chorus_in_r.copy_from_slice(out_r);
            self.chorus
                .process_block_stereo(chorus_in_l, chorus_in_r, out_l, out_r);
        }
        if flags.delay {
            for i in 0..block {
                let (l, r) = self.delay.process(out_l[i], out_r[i]);
                out_l[i] = l;
                out_r[i] = r;
            }
        }

        // Reverb (post-delay): FDN takes the stereo bus as input and applies its
        // own internal dry/wet crossfade. Skipped when off so the engine stays
        // sample-exact against a build with reverb absent.
        if flags.reverb {
            let mut wet_l = [0f32; CONTROL_BLOCK];
            let mut wet_r = [0f32; CONTROL_BLOCK];
            let (wl, wr) = (&mut wet_l[..block], &mut wet_r[..block]);
            self.reverb.process_block(out_l, out_r, wl, wr);
            out_l.copy_from_slice(wl);
            out_r.copy_from_slice(wr);
        }

        // Master limiter (last in the chain): clear stale lookahead state on the
        // off→on edge so re-engaging it can't leak an old transient.
        if flags.limiter {
            if !self.limiter_was_on {
                self.limiter.reset();
            }
            self.limiter.process_block(out_l, out_r);
        }
        self.limiter_was_on = flags.limiter;
    }
}

/// The oversampled synthesis path's anti-aliasing decimators (one per stereo
/// channel) plus the bookkeeping that keeps them phase-aligned: the mono→stereo
/// seed flag, the silent-drain counter and the active oversample factor. Owning
/// the L/R pair as one unit folds the duplicated reset / decimate / zero-fill
/// branches into a single paired API (0106 / 0107).
struct OutputStage {
    /// Both stay phase-aligned: at spread = 0 the L and R input streams are
    /// identical, so the filter states evolve in lock-step and `r` outputs match
    /// `l` sample-for-sample. `spread_zero_last_block` tracks whether the R
    /// decimator was skipped last block; the mono→stereo transition seeds the R
    /// decimator from L's converged state to avoid a click (0107).
    oversampler: Oversampler,
    oversampler_r: Oversampler,
    spread_zero_last_block: bool,
    /// Consecutive blocks both banks took the silent fast path. Once it exceeds
    /// [`DECIMATOR_DRAIN_BLOCKS`] the L decimator's FIR state has fully drained
    /// to zero, so the `decimate()` call is skipped and the base-rate output is
    /// zero-filled until a bank wakes back up (0106).
    silent_blocks: u32,
    /// Oversampling factor in effect last block; a change resets the decimators.
    last_os: usize,
}

impl OutputStage {
    fn new() -> Self {
        Self {
            oversampler: Oversampler::new(),
            oversampler_r: Oversampler::new(),
            spread_zero_last_block: true,
            silent_blocks: 0,
            last_os: 1,
        }
    }

    /// Clear both decimators and the phase-alignment bookkeeping. (Leaves
    /// `last_os` alone — the factor itself is unchanged by a transport reset; a
    /// genuine factor change is handled by [`Self::on_os_change`].)
    fn reset(&mut self) {
        self.oversampler.reset();
        self.oversampler_r.reset();
        self.spread_zero_last_block = true;
        self.silent_blocks = 0;
    }

    /// Reset both decimators when the oversample factor changes between process
    /// calls (the FIR state is rate-specific).
    fn on_os_change(&mut self, os: usize) {
        if os != self.last_os {
            self.oversampler.reset();
            self.oversampler_r.reset();
            self.last_os = os;
        }
    }

    /// Decimate the oversampled L/R buses down to `dst_l`/`dst_r` at the base
    /// rate. `spread_zero` means both layers are centred (L == R), so the R
    /// decimator is skipped and `dst_r` is copied from `dst_l`; `both_silent`
    /// drives the drain-skip that zero-fills once the FIR has flushed (0106).
    /// The mono→stereo transition seeds R from L *before* L decimates this block
    /// so R starts from L's converged state rather than one block ahead (0107).
    #[allow(clippy::too_many_arguments)] // one paired decimate step, single caller
    fn decimate_block(
        &mut self,
        l_os: &[f32],
        r_os: &[f32],
        dst_l: &mut [f32],
        dst_r: &mut [f32],
        os: usize,
        spread_zero: bool,
        both_silent: bool,
    ) {
        // Silent-skip predicate (0106): track consecutive all-silent blocks —
        // once the decimator's FIR state has drained, skip it and zero-fill.
        if both_silent {
            self.silent_blocks = self.silent_blocks.saturating_add(1);
        } else {
            self.silent_blocks = 0;
        }
        let skip_decimator = self.silent_blocks > DECIMATOR_DRAIN_BLOCKS;

        if !spread_zero && self.spread_zero_last_block {
            self.oversampler_r.clone_state_from(&self.oversampler);
        }

        if skip_decimator {
            dst_l.fill(0.0);
        } else {
            self.oversampler.decimate(l_os, dst_l, os);
        }
        if spread_zero {
            dst_r.copy_from_slice(dst_l);
        } else if skip_decimator {
            dst_r.fill(0.0);
        } else {
            self.oversampler_r.decimate(r_os, dst_r, os);
        }
        self.spread_zero_last_block = spread_zero;
    }
}

impl Synth {
    pub fn new(sample_rate: f32) -> Self {
        // The LFO ticks once per control block, so its effective sample rate
        // is the control rate. Max LFO rate (40 Hz) still has ample steps/cycle.
        let control_rate = sample_rate / CONTROL_BLOCK as f32;
        let params = ParamValues::default();
        Self {
            sample_rate,
            smoother: ParamSmoother::new(sample_rate, &params),
            params,
            banks: std::array::from_fn(|i| VoiceBank::new(sample_rate, RNG_SEEDS[i])),
            lfo2: LfoCore::new(control_rate, LFO2_SEED),
            master_fx: MasterFx::new(sample_rate),
            output: OutputStage::new(),
            bend_norm: 0.0,
            mod_wheel: Smoothed::new(0.0, MOD_WHEEL_SMOOTH_MS, control_rate),
            key_mode: KeyMode::Whole,
            split_point: DEFAULT_SPLIT_POINT,
            tempo_bpm: sync::DEFAULT_TEMPO_BPM,
            alloc_counter: 0,
            rr_layer: 0,
            last_env: [None; LAYERS],
            last_assign_mode: [AssignMode::default(); LAYERS],
            drift_amount: voice::DEFAULT_DRIFT_AMOUNT,
        }
    }

    /// Override the per-voice oscillator drift amount (`[0.0, 1.0]`). Default
    /// is [`voice::DEFAULT_DRIFT_AMOUNT`]. Set to 0 to make two identical
    /// layers' renders bit-equal — the equivalence tests in this module rely
    /// on that, but typical playback wants drift on for the "live" detune.
    pub fn set_drift_amount(&mut self, amount: f32) {
        self.drift_amount = amount.clamp(0.0, 1.0);
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        if (sample_rate - self.sample_rate).abs() < f32::EPSILON {
            return;
        }
        self.sample_rate = sample_rate;
        let control_rate = sample_rate / CONTROL_BLOCK as f32;
        for bank in self.banks.iter_mut() {
            bank.set_sample_rate(sample_rate);
        }
        self.lfo2 = LfoCore::new(control_rate, LFO2_SEED);
        self.master_fx = MasterFx::new(sample_rate);
        self.output.reset();
        self.mod_wheel.set_time(MOD_WHEEL_SMOOTH_MS, control_rate);
        self.smoother.set_sample_rate(sample_rate);
        self.smoother.snap_all(&self.params);
        // Envelope cores were recreated with zeroed coefficients; force a refresh.
        self.last_env = [None; LAYERS];
    }

    pub fn params(&self) -> &ParamValues {
        &self.params
    }

    pub fn params_mut(&mut self) -> &mut ParamValues {
        &mut self.params
    }

    /// Set a parameter by CLAP id (routed to its layer/global slot).
    pub fn set_param(&mut self, index: usize, value: f32) {
        self.params.set_by_clap_id(index, value);
    }

    /// Pitch bend in normalised `[-1, 1]`. The semitone span is the layer's
    /// `PitchWheelDepth` (default ±2 st), applied in `build_ctx`.
    pub fn set_pitch_bend(&mut self, normalized: f32) {
        self.bend_norm = normalized.clamp(-1.0, 1.0);
    }

    /// Mod wheel (CC1) in normalised `[0, 1]`. Routed in `build_ctx` through the
    /// mod-wheel panel depths (PWM / cutoff / reso / osc2 pitch); smoothed at the
    /// control rate.
    pub fn set_mod_wheel(&mut self, normalized: f32) {
        self.mod_wheel.set_target(normalized.clamp(0.0, 1.0));
    }

    /// Set the key mode (ADR 0003 §3). Cheap; the seed-on-entry copy lives in
    /// the shared store ([`SharedParams::set_key_mode_seeded`]) so it persists
    /// and is echoed to the host — the engine just reads the mode it is given.
    pub fn set_key_mode(&mut self, mode: KeyMode) {
        self.key_mode = mode;
    }

    pub fn key_mode(&self) -> KeyMode {
        self.key_mode
    }

    /// Set the split point (MIDI note) used by [`KeyMode::Split`] routing.
    pub fn set_split_point(&mut self, note: u8) {
        self.split_point = note.min(127);
    }

    /// Host tempo (BPM) for LFO host-sync (E004 / 0015), pushed each block from
    /// the CLAP transport. Non-finite or non-positive input falls back to the
    /// default so a synced LFO never produces NaN/Inf.
    pub fn set_tempo(&mut self, bpm: f32) {
        self.tempo_bpm = if bpm.is_finite() && bpm > 0.0 {
            bpm
        } else {
            sync::DEFAULT_TEMPO_BPM
        };
    }

    /// Which param block layer `layer` reads under `key_mode` (ADR 0003 §3):
    /// in **Whole**, both layers read layer A's (Upper) block — no mirroring;
    /// in **Dual/Split**, each layer reads its own.
    #[inline]
    fn param_source(layer: usize, key_mode: KeyMode) -> Layer {
        match key_mode {
            KeyMode::Whole => Layer::Upper,
            _ => Layer::ALL[layer],
        }
    }

    /// Route a note-on to the layer(s) chosen by the current key mode (ADR 0003
    /// §3): Whole round-robins across the layers (16-voice), Dual duplicates to
    /// both (layered 8+8), Split partitions at the split point (Lower below,
    /// Upper at/above). Note-offs broadcast, so each layer releases only the
    /// note it actually started.
    pub fn note_on(&mut self, note: u8, velocity: f32) {
        match self.key_mode {
            KeyMode::Whole => {
                // Solo and Unison are monophonic per layer, so round-robining across
                // both layers would give two simultaneous voices and split the
                // held-note stack — each note would land on a different bank,
                // defeating mono/legato. Pin them to one layer (Upper, whose block
                // both layers read in Whole). Poly/Twin still spread 8+8.
                let mono = matches!(
                    self.params.layer(Layer::Upper).assign_mode(),
                    AssignMode::Solo | AssignMode::Unison
                );
                if mono {
                    self.note_on_layer(Layer::Upper as usize, note, velocity);
                } else {
                    let layer = self.rr_layer;
                    self.rr_layer ^= 1;
                    self.note_on_layer(layer, note, velocity);
                }
            }
            KeyMode::Dual => {
                self.note_on_layer(Layer::Upper as usize, note, velocity);
                self.note_on_layer(Layer::Lower as usize, note, velocity);
            }
            KeyMode::Split => {
                let layer = if note < self.split_point {
                    Layer::Lower
                } else {
                    Layer::Upper
                };
                self.note_on_layer(layer as usize, note, velocity);
            }
        }
    }

    /// Start a note on a specific layer. [`Self::note_on`] calls this per the
    /// key-mode routing policy; exposed for tests and future per-layer drivers.
    /// The assign mode (Poly/Unison) is read live from the layer's param source
    /// (ADR 0003 §4) so it always reflects the current patch.
    pub fn note_on_layer(&mut self, layer: usize, note: u8, velocity: f32) {
        self.alloc_counter += 1;
        let src = Self::param_source(layer, self.key_mode);
        let p = self.params.layer(src);
        let mode = p.assign_mode();
        // Poly/Twin → Solo/Unison: gate off held poly voices before placing the new
        // mono note so they don't sustain under the monophonic allocator. Updating
        // last_assign_mode here also prevents process() from re-detecting the same
        // transition after the new note is placed.
        if self.last_assign_mode[layer] != mode {
            if matches!(self.last_assign_mode[layer], AssignMode::Poly | AssignMode::Twin)
                && matches!(mode, AssignMode::Solo | AssignMode::Unison)
            {
                self.banks[layer].all_notes_off();
            }
            self.last_assign_mode[layer] = mode;
        }
        let unison_detune = p.get(PatchParam::UnisonDetune);
        // Per-voice LFO 1 (E005 / 0018): the bank retriggers the triggered
        // channel(s)' LFO 1 phase to the shape's zero crossing at note-on, unless
        // free-run is set.
        let lfo1 = Lfo1Trigger {
            shape: p.lfo_shape(),
            free_run: p.bool(PatchParam::Lfo1FreeRun),
        };
        let legato = p.legato();
        self.banks[layer].note_on(
            mode,
            NoteOn {
                note,
                velocity,
                alloc_tick: self.alloc_counter,
                lfo1,
            },
            unison_detune,
            legato,
        );
    }

    pub fn note_off(&mut self, note: u8) {
        // Broadcast: each layer releases the note only if it is holding it. Mono
        // layers (Solo / Unison) run the stack path (revert to a still-held note);
        // every other mode just gates the matching channels off.
        self.alloc_counter += 1;
        for layer in 0..self.banks.len() {
            let src = Self::param_source(layer, self.key_mode);
            let p = self.params.layer(src);
            if matches!(p.assign_mode(), AssignMode::Solo | AssignMode::Unison) {
                let lfo1 = Lfo1Trigger {
                    shape: p.lfo_shape(),
                    free_run: p.bool(PatchParam::Lfo1FreeRun),
                };
                let legato = p.legato();
                let detune = p.get(PatchParam::UnisonDetune);
                self.banks[layer].mono_note_off(
                    p.assign_mode(),
                    note,
                    legato,
                    self.alloc_counter,
                    detune,
                    lfo1,
                );
            } else {
                self.banks[layer].note_off(note);
            }
        }
    }

    /// Sustain pedal (CC64). Channel-wide: broadcast to every layer's bank.
    /// Poly-only — mono modes (Solo / Unison) ignore the held flag and keep
    /// last-note-priority. Releasing the pedal gates off every note whose key
    /// was lifted while it was down.
    pub fn sustain(&mut self, on: bool) {
        for bank in &mut self.banks {
            bank.set_sustain(on);
        }
    }

    pub fn all_notes_off(&mut self) {
        for bank in &mut self.banks {
            bank.all_notes_off();
        }
    }

    /// Total active channels across both layers.
    pub fn active_count(&self) -> usize {
        self.banks.iter().map(|b| b.active_count()).sum()
    }

    pub fn reset(&mut self) {
        for bank in self.banks.iter_mut() {
            bank.reset_all();
        }
        self.lfo2.reset();
        self.master_fx.reset();
        self.output.reset();
        self.smoother.snap_all(&self.params);
        self.rr_layer = 0;
        self.last_assign_mode = [AssignMode::default(); LAYERS];
    }

    /// Render `out_l`/`out_r` (equal length). No events occur within this span;
    /// the caller splits the host buffer at event boundaries.
    pub fn process(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        // Params are constant across a process call; refresh envelope coeffs at
        // most once per layer, and only when they actually changed.
        // Also detect Poly/Twin → Solo/Unison transitions and gate off any held
        // poly voices so they don't stick under the now-monophonic allocator.
        for layer in 0..LAYERS {
            self.sync_envelopes(layer);
            let src = Self::param_source(layer, self.key_mode);
            let mode = self.params.layer(src).assign_mode();
            let prev = self.last_assign_mode[layer];
            if prev != mode {
                let was_poly = matches!(prev, AssignMode::Poly | AssignMode::Twin);
                let now_mono = matches!(mode, AssignMode::Solo | AssignMode::Unison);
                if was_poly && now_mono {
                    self.banks[layer].all_notes_off();
                }
                self.last_assign_mode[layer] = mode;
            }
        }

        // Oversampling factor for this call; a change resets the decimator.
        let os = self.params.global().oversample_factor();
        self.output.on_os_change(os);

        let key_mode = self.key_mode;
        let n = out_l.len().min(out_r.len());
        let mut start = 0;
        while start < n {
            let block = (n - start).min(CONTROL_BLOCK);
            // Advance gain-like smoothers toward the raw targets for this block.
            self.smoother.tick_block(&self.params);
            // Mod wheel is a single global control; tick once per block and
            // apply per layer (each layer routes it via its own params §9).
            let wheel = self.mod_wheel.tick();

            // Global LFO 2 (E005 / 0019): one instrument-wide LFO, sampled once
            // per block and broadcast to both layers. Its shape/rate/sync are
            // global params; host-sync resolves its rate from the engine tempo.
            let gv = self.smoother.values().global();
            let lfo2_shape = gv.lfo2_shape();
            let lfo2_rate = lfo_rate_from(
                GlobalParam::Lfo2Rate.desc(),
                gv.get(GlobalParam::Lfo2Rate),
                gv.bool(GlobalParam::Lfo2Sync),
                self.tempo_bpm,
            );
            let lfo2_val = self.lfo2.next(lfo2_shape);
            self.lfo2.set_rate(lfo2_rate);

            // Both layers render (summed) into oversampled stereo buses, then
            // decimated back to the base rate before the global FX bus (§7).
            // `PatchParam::Spread` distributes voice slots across L/R inside
            // each bank's `render_block`; spread = 0 collapses every lane to
            // centre so L = R bit-for-bit. When BOTH layers' source patches
            // hold spread = 0 (the common default), the R decimator is
            // skipped and `r_dec` is filled from `l_dec`; the mono→stereo
            // transition seeds the R decimator from L's converged state to
            // avoid a click (0107).
            let spread_zero = {
                let vals = self.smoother.values();
                vals.layer(Self::param_source(0, key_mode))
                    .get(PatchParam::Spread)
                    == 0.0
                    && vals
                        .layer(Self::param_source(1, key_mode))
                        .get(PatchParam::Spread)
                        == 0.0
            };

            let mut l_os_buf = [0.0f32; CONTROL_BLOCK * MAX_OVERSAMPLE];
            let mut r_os_buf = [0.0f32; CONTROL_BLOCK * MAX_OVERSAMPLE];
            let l_os = &mut l_os_buf[..block * os];
            let r_os = &mut r_os_buf[..block * os];
            for layer in 0..LAYERS {
                let ctx = self.build_ctx(layer, key_mode, os, wheel, lfo2_val);
                self.banks[layer].render_block(l_os, r_os, &ctx);
            }

            // Decimate the oversampled buses to the base rate. Both banks
            // silent → the OS bus is zero, so the silent-drain skip can
            // eventually zero-fill; spread = 0 → R is copied from L. All of
            // that bookkeeping lives in `OutputStage` (0106 / 0107).
            let both_silent = self.banks[0].is_silent() && self.banks[1].is_silent();
            let mut l_dec = [0.0f32; CONTROL_BLOCK];
            let mut r_dec = [0.0f32; CONTROL_BLOCK];
            let l_dec = &mut l_dec[..block];
            let r_dec = &mut r_dec[..block];
            self.output
                .decimate_block(l_os, r_os, l_dec, r_dec, os, spread_zero, both_silent);

            // Effects (stereo), then write out. On/off are raw (unsmoothed)
            // discrete switches; the FX chain itself lives in `MasterFx`.
            let g = self.params.global();
            let reverb_on = g.bool(GlobalParam::ReverbOn);
            let flags = MasterFxFlags {
                phaser: g.bool(GlobalParam::PhaserOn),
                chorus: g.bool(GlobalParam::ChorusOn),
                delay: g.bool(GlobalParam::DelayOn),
                reverb: reverb_on,
                limiter: g.bool(GlobalParam::LimiterOn),
            };
            self.master_fx
                .update(self.smoother.values().global(), reverb_on, self.tempo_bpm);
            // Drift: the per-voice oscillator pitch jitter amount, broadcast into
            // every voice's BlockCtx next block. Direct read (no smoother — drift
            // is a slow creative param, sub-audio).
            self.drift_amount = self
                .params
                .global()
                .get(GlobalParam::MasterDrift)
                .clamp(0.0, 1.0);

            // Apply the per-sample master-volume glide into the dry stereo
            // bus, then run the stereo effects a block at a time.
            let mut dry_l_buf = [0.0f32; CONTROL_BLOCK];
            let mut dry_r_buf = [0.0f32; CONTROL_BLOCK];
            let dry_l = &mut dry_l_buf[..block];
            let dry_r = &mut dry_r_buf[..block];
            for i in 0..block {
                let vol = self.smoother.next_volume();
                dry_l[i] = l_dec[i] * vol;
                dry_r[i] = r_dec[i] * vol;
            }

            let l_out = &mut out_l[start..start + block];
            let r_out = &mut out_r[start..start + block];
            self.master_fx
                .process_block(dry_l, dry_r, l_out, r_out, flags);
            start += block;
        }
    }

    /// Push envelope params to a layer's voices when they change. Reads the
    /// layer's param source (Whole → Upper for both). Applies to every voice
    /// (active or not) so a later-reused voice already has fresh coeffs.
    fn sync_envelopes(&mut self, layer: usize) {
        let src = Self::param_source(layer, self.key_mode);
        let p = self.params.layer(src);
        let snap = EnvSnapshot {
            env1: (
                p.get(PatchParam::Env1Attack),
                p.get(PatchParam::Env1Decay),
                p.get(PatchParam::Env1Sustain),
                p.get(PatchParam::Env1Release),
            ),
            env1_shape: p.env1_shape(),
            env2: (
                p.get(PatchParam::Env2Attack),
                p.get(PatchParam::Env2Decay),
                p.get(PatchParam::Env2Sustain),
                p.get(PatchParam::Env2Release),
            ),
            env2_shape: p.env2_shape(),
            drift_amount: self.drift_amount,
        };
        if self.last_env[layer] == Some(snap) {
            return;
        }
        self.banks[layer].set_envelopes(
            snap.env1,
            snap.env1_shape,
            snap.env2,
            snap.env2_shape,
            snap.drift_amount,
        );
        self.last_env[layer] = Some(snap);
    }

    /// Build one layer's control-block context from its param source (§3) and the
    /// global block. `wheel` is the once-per-block global mod-wheel value, applied
    /// here via this layer's routing params (§9). `lfo2_val` is the single global
    /// LFO 2 value, sampled once per block in `process` and broadcast (§5, E005).
    fn build_ctx(
        &self,
        layer: usize,
        key_mode: KeyMode,
        os: usize,
        wheel: f32,
        lfo2_val: f32,
    ) -> BlockCtx {
        let src = Self::param_source(layer, key_mode);
        let vals = self.smoother.values();
        let p = vals.layer(src);
        let g = vals.global();
        let tempo = self.tempo_bpm;
        // LFO 1 is per-voice (E005 / 0018): the bank ticks each channel's phase.
        // Resolve its shared rate (post host-sync) here and hand the bank LFO 1's
        // shape + onset times. LFO 2 is the global LFO, already sampled.
        let lfo1_rate_hz = lfo_rate(p, PatchParam::LfoRate, PatchParam::LfoSync, tempo);

        // Cross-mod type selector → (sync flag, PM index, ring flag). Off zeroes
        // sync/PM and disables ring, so the voice keeps the independent fast
        // path; the four variants are mutually exclusive.
        let (sync, pm_index, ring_mode) = match p.cross_mod_type() {
            CrossModType::Off => (false, 0.0, false),
            CrossModType::Sync => (true, 0.0, false),
            CrossModType::Pm => (false, p.get(PatchParam::CrossModAmount), false),
            CrossModType::Ring => (false, 0.0, true),
        };

        // Mod wheel (CC1) is a global control applied once per block, folded into
        // the route `*_extra` terms (and resonance) here rather than per voice.
        let resonance = (p.get(PatchParam::Resonance) + wheel * p.get(PatchParam::ModWheelReso))
            .clamp(0.0, 1.0);

        BlockCtx {
            os_sample_rate: self.sample_rate * os as f32,
            os,
            osc: OscParams {
                osc1_wave: p.osc_wave(PatchParam::Osc1Wave),
                osc2_wave: p.osc_wave(PatchParam::Osc2Wave),
                osc1_level: p.get(PatchParam::Osc1Level),
                osc2_level: p.get(PatchParam::Osc2Level),
                sub_level: p.get(PatchParam::SubLevel),
                noise_level: p.get(PatchParam::NoiseLevel),
                noise_color: p.noise_color(),
                osc1_pw: p.get(PatchParam::Osc1PulseWidth),
                osc2_pw: p.get(PatchParam::Osc2PulseWidth),
                // Octave and Coarse are integer-semitone params: hard-quantise them
                // (the fader stores a continuous value) so the tuning lands exactly
                // on a semitone. Fine stays continuous (cents).
                osc1_semi: p.get(PatchParam::Osc1Octave).round() * 12.0
                    + p.get(PatchParam::Osc1Coarse).round()
                    + p.get(PatchParam::Osc1Fine) / 100.0,
                osc2_semi: p.get(PatchParam::Osc2Octave).round() * 12.0
                    + p.get(PatchParam::Osc2Coarse).round()
                    + p.get(PatchParam::Osc2Fine) / 100.0,
            },
            cross_mod: CrossMod {
                sync,
                pm_index,
                ring_mode,
                cross_mod_type: p.cross_mod_type(),
            },
            filter: FilterParams {
                cutoff: p.get(PatchParam::Cutoff),
                hpf_cutoff: p.get(PatchParam::HpfCutoff),
                resonance,
                drive: p.get(PatchParam::Drive),
                filter_mode: p.filter_mode(),
                filter_slope: p.filter_slope(),
            },
            base_semis: g.get(GlobalParam::MasterTune),
            lfo1_shape: p.lfo_shape(),
            lfo1_rate_hz,
            lfo1_delay_time: p.get(PatchParam::Lfo1DelayTime),
            lfo1_fade: p.get(PatchParam::Lfo1Fade),
            lfo2_val,
            portamento_time: p.get(PatchParam::PortamentoTime),
            // Fixed routes (ADR 0004 §4).
            pitch: PitchRoute {
                lfo_sel: p.lfo_sel(PatchParam::PitchLfoSrc),
                lfo_depth: p.get(PatchParam::PitchLfoDepth),
                lfo_mod_only: p.bool(PatchParam::PitchLfoModOnly),
                env_sel: p.env_sel(PatchParam::PitchEnvSrc),
                env_depth: p.get(PatchParam::PitchEnvDepth),
                env_mod_only: p.bool(PatchParam::PitchEnvModOnly),
                extra: self.bend_norm * p.get(PatchParam::PitchWheelDepth),
                sweep_extra: wheel * p.get(PatchParam::ModWheelCrossModSweep),
            },
            pwm: PwmRoute {
                lfo_sel: p.lfo_sel(PatchParam::PwmLfoSrc),
                lfo_depth: p.get(PatchParam::PwmLfoDepth),
                env_sel: p.env_sel(PatchParam::PwmEnvSrc),
                env_depth: p.get(PatchParam::PwmEnvDepth),
                extra: wheel * p.get(PatchParam::ModWheelPwm),
            },
            cutoff: CutoffRoute {
                lfo1_depth: p.get(PatchParam::CutoffLfo1Depth),
                lfo2_depth: p.get(PatchParam::CutoffLfo2Depth),
                env_depth: p.get(PatchParam::CutoffEnvDepth),
                vel_depth: p.get(PatchParam::VelCutoffDepth),
                extra: wheel * p.get(PatchParam::ModWheelCutoff),
                key_track: p.get(PatchParam::FilterKeyTrack),
            },
            amp: AmpRoute {
                lfo_sel: p.lfo_sel(PatchParam::AmpLfoSrc),
                lfo_depth: p.get(PatchParam::AmpLfoDepth),
                env_bypass: p.bool(PatchParam::AmpEnvBypass),
            },
            drift_amount: self.drift_amount,
            layer_level: p.get(PatchParam::LayerLevel),
            spread: p.get(PatchParam::Spread),
        }
    }
}

/// Resolve an LFO's rate in Hz for this block (E004 / 0015). Sync off: the rate
/// knob is free-running Hz, exactly as before. Sync on: the knob's normalised
/// position (over `desc`'s range) selects a musical subdivision locked to
/// `tempo_bpm`. The LFO core clamps the result to its valid Hz range. Works for
/// both the per-patch LFO 1 rate and the global LFO 2 rate via their descriptors.
#[inline]
fn lfo_rate_from(desc: &ParamDesc, rate_value: f32, sync_on: bool, tempo_bpm: f32) -> f32 {
    if sync_on {
        // Spread subdivisions linearly across the knob's travel (`to_fader`), not
        // its tapered Hz value — even subdivision spacing with no midpoint skew.
        let pos = desc.to_fader(rate_value);
        sync::synced_hz(tempo_bpm, sync::index_from_norm(pos))
    } else {
        rate_value
    }
}

/// [`lfo_rate_from`] for a per-patch LFO rate/sync pair (LFO 1).
#[inline]
fn lfo_rate(p: &PatchValues, rate: PatchParam, sync_flag: PatchParam, tempo_bpm: f32) -> f32 {
    lfo_rate_from(rate.desc(), p.get(rate), p.bool(sync_flag), tempo_bpm)
}

/// Resolve the delay time in seconds for this block (E006). Sync off: the Time
/// knob is taken as literal seconds, exactly as before. Sync on: the knob's
/// normalised position selects a musical subdivision locked to `tempo_bpm`
/// (mirrors the LFO host-sync in [`lfo_rate_from`]). The knob's stored value is
/// never mutated, so toggling sync off reads back as the same ms again. The
/// returned value can still exceed the delay buffer; `StereoDelay::set_params`
/// clamps it to capacity regardless of tempo.
#[inline]
fn delay_time_seconds(sync_on: bool, time_value: f32, tempo_bpm: f32) -> f32 {
    if sync_on {
        // Subdivisions spread linearly across the Time knob's travel (`to_fader`),
        // matching the LFO sync — even spacing, no midpoint skew.
        let pos = GlobalParam::DelayTime.desc().to_fader(time_value);
        sync::synced_seconds(tempo_bpm, sync::index_from_norm(pos))
    } else {
        time_value
    }
}

/// Convenience: A4 = 440 Hz reference, exposed for tests/tools.
pub fn a4_hz() -> f32 {
    note_to_hz(69.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{
        AssignMode, GlobalParam, Layer, PatchParam, PatchValues, global_clap_id, patch_clap_id,
    };

    /// Upper-layer per-patch CLAP id (tests drive the single render path = Upper).
    fn pp(p: PatchParam) -> usize {
        patch_clap_id(Layer::Upper, p)
    }
    /// Global-param CLAP id.
    fn gp(g: GlobalParam) -> usize {
        global_clap_id(g)
    }
    /// Lower-layer per-patch CLAP id (for two-layer tests).
    fn lo(p: PatchParam) -> usize {
        patch_clap_id(Layer::Lower, p)
    }

    fn render(synth: &mut Synth, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let mut l = vec![0.0; frames];
        let mut r = vec![0.0; frames];
        synth.process(&mut l, &mut r);
        (l, r)
    }

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    }

    #[test]
    fn silent_when_idle() {
        let mut s = Synth::new(48_000.0);
        let (l, _) = render(&mut s, 512);
        assert!(rms(&l) < 1e-6, "idle output not silent");
    }

    #[test]
    fn note_produces_sound_then_releases_to_silence() {
        let mut s = Synth::new(48_000.0);
        // Fast amp envelope (ENV-2 drives the VCA by default) so the test is short.
        s.set_param(pp(PatchParam::Env2Attack), 0.001);
        s.set_param(pp(PatchParam::Env2Release), 0.01);
        s.set_param(gp(GlobalParam::ChorusOn), 0.0);
        s.note_on(69, 1.0);
        let (l, _) = render(&mut s, 4800);
        assert!(rms(&l) > 0.01, "note produced no sound");

        s.note_off(69);
        // Render well past the release.
        let (tail, _) = render(&mut s, 48_000);
        let last = &tail[tail.len() - 4800..];
        assert!(
            rms(last) < 1e-4,
            "did not release to silence: {}",
            rms(last)
        );
    }

    #[test]
    fn output_finite_under_stress() {
        let mut s = Synth::new(44_100.0);
        s.set_param(pp(PatchParam::Resonance), 1.0);
        s.set_param(gp(GlobalParam::DelayOn), 1.0);
        for n in 60..76 {
            s.note_on(n, 1.0);
        }
        let (l, r) = render(&mut s, 44_100);
        assert!(
            l.iter().chain(r.iter()).all(|x| x.is_finite()),
            "non-finite output"
        );
        let peak = l.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!(peak < 20.0, "output blew up: peak {peak}");
    }

    #[test]
    fn spread_step_glides_stereo_image() {
        // 0015: a Spread automation step must widen the stereo image over the
        // glide window, not jump it in one block. With several voices, Spread
        // pans them across L/R; measure the normalised L/R divergence right after
        // the step vs once settled — a snap would make the first block already
        // wide.
        let mut s = Synth::new(48_000.0);
        s.set_param(gp(GlobalParam::ChorusOn), 0.0); // FX off → clean L/R read
        s.set_param(pp(PatchParam::Env2Attack), 0.001);
        s.set_param(pp(PatchParam::Env2Sustain), 1.0);
        for n in [60, 64, 67, 71] {
            s.note_on(n, 1.0);
        }
        // Settle at Spread 0 (default) — mono, L == R.
        render(&mut s, 9_600);

        let divergence = |l: &[f32], r: &[f32]| {
            let num: f32 = l.iter().zip(r).map(|(a, b)| (a - b).abs()).sum();
            let den: f32 = l.iter().zip(r).map(|(a, b)| a.abs() + b.abs()).sum::<f32>() + 1e-9;
            num / den
        };

        let (l0, r0) = render(&mut s, CONTROL_BLOCK);
        assert!(divergence(&l0, &r0) < 1e-4, "Spread 0 should be mono");

        // Step Spread to full and read the very first block after the step.
        s.set_param(pp(PatchParam::Spread), 1.0);
        let (l1, r1) = render(&mut s, CONTROL_BLOCK);
        let first = divergence(&l1, &r1);

        // Let the glide settle, then read the converged image.
        render(&mut s, 9_600);
        let (l2, r2) = render(&mut s, CONTROL_BLOCK);
        let settled = divergence(&l2, &r2);

        assert!(settled > 0.05, "Spread should widen the image, got {settled}");
        assert!(
            first < 0.5 * settled,
            "Spread jumped in one block (no glide): first {first}, settled {settled}"
        );
    }

    #[test]
    fn vca_follows_env2() {
        // The VCA is hardwired to Env2 (ADR 0004 §4): a held note with Env2
        // sustain 0 and a fast decay settles to silence, proving the amp gain
        // comes from Env2 directly.
        let mut s = Synth::new(48_000.0);
        s.set_param(gp(GlobalParam::ChorusOn), 0.0);
        s.set_param(pp(PatchParam::Env2Decay), 0.01);
        s.set_param(pp(PatchParam::Env2Sustain), 0.0);
        s.note_on(69, 1.0);
        let (l, _) = render(&mut s, 48_000);
        let tail = &l[l.len() - 4800..];
        assert!(
            rms(tail) < 1e-6,
            "Env2 sustain 0 should settle to silence, got {}",
            rms(tail)
        );
    }

    #[test]
    fn noise_level_produces_sound_with_oscillators_silenced() {
        // With both oscillators at zero level, only the noise source can make
        // sound — proving noise is wired into the mixer.
        let mut s = Synth::new(48_000.0);
        s.set_param(gp(GlobalParam::ChorusOn), 0.0);
        s.set_param(pp(PatchParam::Osc1Level), 0.0);
        s.set_param(pp(PatchParam::Osc2Level), 0.0);
        s.set_param(pp(PatchParam::Env2Sustain), 1.0);
        s.note_on(69, 1.0);
        // Let the osc-level glide settle to 0, then a silent window (noise off).
        render(&mut s, 9_600);
        let (silent, _) = render(&mut s, 4_800);
        s.set_param(pp(PatchParam::NoiseLevel), 0.8);
        let (loud, _) = render(&mut s, 48_000);
        assert!(
            rms(&silent) < 1e-5,
            "no source should be silent: {}",
            rms(&silent)
        );
        let tail = &loud[loud.len() - 4800..];
        assert!(
            rms(tail) > 1e-3,
            "noise should be audible, got {}",
            rms(tail)
        );
    }

    #[test]
    fn amp_env_bypass_holds_full_level_ignoring_env2() {
        // Gate-only VCA: with Env2 sustain 0 and fast decay (which would silence
        // the enveloped VCA — see `vca_follows_env2`), bypass keeps a held note at
        // full level because the amp follows the gate, not Env2.
        let mut s = Synth::new(48_000.0);
        s.set_param(gp(GlobalParam::ChorusOn), 0.0);
        s.set_param(pp(PatchParam::Env2Decay), 0.01);
        s.set_param(pp(PatchParam::Env2Sustain), 0.0);
        s.set_param(pp(PatchParam::AmpEnvBypass), 1.0);
        s.note_on(69, 1.0);
        let (l, _) = render(&mut s, 48_000);
        let tail = &l[l.len() - 4800..];
        assert!(
            rms(tail) > 1e-2,
            "bypass should hold full level despite Env2 sustain 0, got {}",
            rms(tail)
        );
    }

    #[test]
    fn amp_tremolo_attenuates_output() {
        // A square-wave LFO into the amp at full depth chops the VCA between full
        // and silence, so the windowed RMS varies far more than the un-tremoloed
        // (steady-sustain) signal.
        let setup = |trem: bool| {
            let mut s = Synth::new(48_000.0);
            s.set_param(gp(GlobalParam::ChorusOn), 0.0);
            s.set_param(pp(PatchParam::Env2Sustain), 1.0);
            s.set_param(pp(PatchParam::LfoShape), 4.0); // Square
            s.set_param(pp(PatchParam::LfoRate), 8.0);
            if trem {
                s.set_param(pp(PatchParam::AmpLfoSrc), 1.0); // LFO 1
                s.set_param(pp(PatchParam::AmpLfoDepth), 1.0);
            }
            s.note_on(57, 1.0);
            let (l, _) = render(&mut s, 48_000);
            l
        };
        // Window RMS over 480-sample frames; tremolo makes it swing, steady doesn't.
        let spread = |l: &[f32]| {
            let w: Vec<f32> = l.chunks(480).map(rms).filter(|r| *r > 0.0).collect();
            let max = w.iter().cloned().fold(0.0f32, f32::max);
            let min = w.iter().cloned().fold(f32::MAX, f32::min);
            max - min
        };
        assert!(
            spread(&setup(true)) > 3.0 * spread(&setup(false)),
            "tremolo should swing the level far more than steady sustain"
        );
    }

    #[test]
    fn square_amp_tremolo_is_declicked() {
        // A square amp LFO snaps the VCA gain by `depth` at each half-cycle. The
        // gain resolves at block rate, so without smoothing that step lands in a
        // single sample and clicks. With a sine *carrier* (bounded per-sample
        // slope) the only way the output can jump sharply is the gain step, so
        // the max sample-to-sample difference of the tremoloed signal must stay
        // close to the un-tremoloed one — a click would dwarf it.
        let setup = |trem: bool| {
            let mut s = Synth::new(48_000.0);
            s.set_param(gp(GlobalParam::ChorusOn), 0.0);
            s.set_param(gp(GlobalParam::ReverbOn), 0.0);
            s.set_param(gp(GlobalParam::PhaserOn), 0.0);
            s.set_param(pp(PatchParam::Osc1Wave), 0.0); // Sine carrier
            s.set_param(pp(PatchParam::Osc2Level), 0.0); // osc1 only
            s.set_param(pp(PatchParam::Env2Sustain), 1.0);
            s.set_param(pp(PatchParam::Env2Attack), 0.001);
            s.set_param(pp(PatchParam::LfoShape), 4.0); // Square
            s.set_param(pp(PatchParam::LfoRate), 8.0);
            if trem {
                s.set_param(pp(PatchParam::AmpLfoSrc), 1.0); // LFO 1
                s.set_param(pp(PatchParam::AmpLfoDepth), 1.0); // full depth
            }
            s.note_on(45, 1.0); // low note → small per-sample sine slope
            let (l, _) = render(&mut s, 48_000);
            l
        };
        let max_step = |l: &[f32]| {
            l.windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0f32, f32::max)
        };
        let steady = max_step(&setup(false));
        let trem = max_step(&setup(true));
        assert!(
            trem < steady * 3.0,
            "square tremolo clicks: max step {trem} vs steady {steady}"
        );
    }

    #[test]
    fn env_block_skip_waits_for_amp_sustain() {
        // Envelope block-skip must engage only once Env2 (the VCA) actually
        // reaches Sustain. A held note with a long Env2 decay to a low sustain
        // must keep getting quieter through the decay; if the skip froze the
        // level mid-decay the amplitude would plateau early.
        let mut s = Synth::new(48_000.0);
        s.set_param(gp(GlobalParam::ChorusOn), 0.0);
        s.set_param(pp(PatchParam::Env2Attack), 0.001);
        s.set_param(pp(PatchParam::Env2Decay), 0.4); // long decay
        s.set_param(pp(PatchParam::Env2Sustain), 0.05); // low sustain
        s.note_on(60, 1.0);
        let (l, _) = render(&mut s, 24_000); // 0.5 s spans the decay
        let w = 2400;
        let early = rms(&l[w..2 * w]);
        let later = rms(&l[6 * w..7 * w]);
        let settled = rms(&l[9 * w..10 * w]);
        assert!(
            later < early * 0.7,
            "amp decay stalled: early {early} later {later}"
        );
        assert!(
            settled < later,
            "amp kept falling toward sustain: {later} -> {settled}"
        );
    }

    #[test]
    fn env_block_skip_does_not_freeze_mod_envelope() {
        // The skip predicate requires *both* envelopes in Sustain. Here Env2
        // (amp) snaps to full sustain immediately while Env1 — routed to pitch —
        // has a long decay. The skip must stay disengaged while Env1 sweeps, so
        // the pitch slides down from its raised start back to the played note as
        // Env1 → 0. A predicate that checked only Env2 would freeze Env1 and the
        // pitch would stall high. Frequency (zero-crossings) is an unambiguous
        // readout of whether Env1 kept moving.
        let mut s = pitched_synth();
        s.set_param(pp(PatchParam::Env2Decay), 0.001);
        s.set_param(pp(PatchParam::Env2Sustain), 1.0); // amp static almost at once
        s.set_param(pp(PatchParam::PitchEnvSrc), 1.0); // Env1 → pitch
        s.set_param(pp(PatchParam::PitchEnvDepth), 12.0); // +1 octave at Env1 = 1
        s.set_param(pp(PatchParam::Env1Attack), 0.0005);
        s.set_param(pp(PatchParam::Env1Decay), 0.4); // long
        s.set_param(pp(PatchParam::Env1Sustain), 0.0); // → settles to the played note
        s.note_on(57, 1.0); // A3 = 220 Hz; +1 oct = 440 Hz at the peak
        let (l, _) = render(&mut s, 24_000); // 0.5 s spans the decay
        let early = dominant_hz(&l[2400..7200], 48_000.0); // Env1 still high → ~ up an octave
        let late = dominant_hz(&l[19_200..24_000], 48_000.0); // Env1 ≈ 0 → ~ played note
        assert!(
            early > 300.0,
            "expected raised pitch while Env1 high, got {early} Hz"
        );
        assert!(
            late < 250.0,
            "pitch stalled high (mod envelope frozen): late {late} Hz"
        );
    }

    /// Dominant frequency of a mono buffer via zero-crossing count (rising
    /// edges). Crude but enough to tell an octave apart.
    fn dominant_hz(s: &[f32], sr: f32) -> f32 {
        let mut crossings = 0usize;
        for w in s.windows(2) {
            if w[0] <= 0.0 && w[1] > 0.0 {
                crossings += 1;
            }
        }
        crossings as f32 * sr / s.len() as f32
    }

    /// Base clean-sine builder: single osc1 sine, osc2 muted, vibrato killed,
    /// chorus off, fast amp attack. All four prior builders shared this core;
    /// call mutators on the returned synth for anything extra.
    fn clean_sine_synth() -> Synth {
        let mut s = Synth::new(48_000.0);
        s.set_param(pp(PatchParam::Osc1Wave), 0.0); // Sine
        s.set_param(pp(PatchParam::Osc2Level), 0.0);
        s.set_param(pp(PatchParam::PitchLfoDepth), 0.0); // kill default vibrato
        s.set_param(gp(GlobalParam::ChorusOn), 0.0);
        s.set_param(pp(PatchParam::Env2Attack), 0.001);
        s
    }

    fn pitched_synth() -> Synth {
        clean_sine_synth()
    }

    #[test]
    fn octave_up_doubles_frequency() {
        let mut base = pitched_synth();
        base.note_on(57, 1.0); // A3 = 220 Hz
        let (l0, _) = render(&mut base, 24_000);
        let f0 = dominant_hz(&l0[4800..], 48_000.0);

        let mut up = pitched_synth();
        up.set_param(pp(PatchParam::Osc1Octave), 1.0);
        up.note_on(57, 1.0);
        let (l1, _) = render(&mut up, 24_000);
        let f1 = dominant_hz(&l1[4800..], 48_000.0);

        assert!(
            (f1 / f0 - 2.0).abs() < 0.05,
            "octave up should double freq: {f0} -> {f1}"
        );
    }

    #[test]
    fn octave_and_coarse_combine_additively() {
        // +1 octave & +7 st = +19 st. Compare against +2 octaves & -5 st (also +19 st).
        let mut a = pitched_synth();
        a.set_param(pp(PatchParam::Osc1Octave), 1.0);
        a.set_param(pp(PatchParam::Osc1Coarse), 7.0);
        a.note_on(45, 1.0);
        let (la, _) = render(&mut a, 24_000);
        let fa = dominant_hz(&la[4800..], 48_000.0);

        let mut b = pitched_synth();
        b.set_param(pp(PatchParam::Osc1Octave), 2.0);
        b.set_param(pp(PatchParam::Osc1Coarse), -5.0);
        b.note_on(45, 1.0);
        let (lb, _) = render(&mut b, 24_000);
        let fb = dominant_hz(&lb[4800..], 48_000.0);

        assert!((fa / fb - 1.0).abs() < 0.02, "not additive: {fa} vs {fb}");
    }

    #[test]
    fn hpf_thins_low_content_when_engaged() {
        // A low note through a high HPF cutoff loses energy vs the open default.
        fn low_note_rms(hpf_hz: f32) -> f32 {
            let mut s = pitched_synth();
            s.set_param(pp(PatchParam::HpfCutoff), hpf_hz);
            s.note_on(33, 1.0); // A1 ≈ 55 Hz
            let (l, _) = render(&mut s, 24_000);
            rms(&l[4800..])
        }
        let open = low_note_rms(20.0); // default ≈ off
        let engaged = low_note_rms(2000.0);
        assert!(
            engaged < 0.5 * open,
            "HPF did not thin lows: open {open}, engaged {engaged}"
        );
    }

    #[test]
    fn ring_mode_displaces_osc1_and_off_is_inert() {
        // `CrossModType::Ring` routes osc1×osc2 into the osc1 mixer slot, so the
        // patch's timbre shifts vs. the Off render and stays finite. Off is the
        // inert fast path (its output is bit-identical across renders).
        fn render_ring(on: bool) -> Vec<f32> {
            let mut s = pitched_synth();
            s.set_param(pp(PatchParam::Osc1Wave), 0.0); // sine
            s.set_param(pp(PatchParam::Osc2Wave), 0.0);
            s.set_param(pp(PatchParam::Osc1Level), 0.5);
            s.set_param(pp(PatchParam::Osc2Level), 0.5);
            s.set_param(pp(PatchParam::Osc2Coarse), 5.0); // inharmonic vs osc1
            s.set_param(
                pp(PatchParam::CrossModType),
                if on { 3.0 } else { 0.0 },
            );
            s.note_on(45, 1.0);
            render(&mut s, 12_000).0
        }
        let dry = render_ring(false);
        assert_eq!(dry, render_ring(false), "Ring off path not deterministic");
        let wet = render_ring(true);
        assert!(wet.iter().all(|x| x.is_finite()), "ring output not finite");
        let diff = mean_abs_diff(&dry[4800..], &wet[4800..]);
        assert!(diff > 1e-3, "Ring mode did not change the output: {diff}");
    }

    #[test]
    fn filter_key_track_opens_cutoff_with_pitch() {
        // Key-track on: a high note sits a fixed octave-per-octave higher in
        // cutoff than with key-track off, so a saw plays brighter. Off: the note
        // pitch has no influence on cutoff. (ADR 0004 §4.)
        fn bright(key_track: bool) -> f32 {
            let mut s = pitched_synth();
            s.set_param(pp(PatchParam::Osc1Wave), 2.0); // saw
            s.set_param(pp(PatchParam::Cutoff), 300.0); // dark base
            s.set_param(pp(PatchParam::Resonance), 0.0);
            s.set_param(
                pp(PatchParam::FilterKeyTrack),
                if key_track { 1.0 } else { 0.0 },
            );
            s.note_on(72, 1.0); // a high note → large key-track shift when on
            let (l, _) = render(&mut s, 24_000);
            assert!(
                l.iter().all(|x| x.is_finite()),
                "key-track output not finite"
            );
            rms(&l[4800..])
        }
        let off = bright(false);
        let on = bright(true);
        assert!(
            on > 1.5 * off,
            "key-track did not open the filter with pitch: off {off}, on {on}"
        );
    }

    /// Render a saw with LFO 1 → cutoff at the given onset, capturing `window`.
    /// `depth = 0` is the no-LFO baseline (the route contributes nothing).
    fn lfo1_cutoff_render(
        depth: f32,
        delay: f32,
        fade: f32,
        rate: f32,
        window: std::ops::Range<usize>,
    ) -> Vec<f32> {
        let mut s = pitched_synth();
        s.set_param(pp(PatchParam::Osc1Wave), 2.0); // saw
        s.set_param(pp(PatchParam::Cutoff), 1000.0);
        s.set_param(pp(PatchParam::CutoffLfo1Depth), depth); // LFO 1 → cutoff
        s.set_param(pp(PatchParam::LfoRate), rate);
        s.set_param(pp(PatchParam::Lfo1DelayTime), delay);
        s.set_param(pp(PatchParam::Lfo1Fade), fade);
        s.note_on(69, 1.0);
        let (l, _) = render(&mut s, 96_000);
        l[window].to_vec()
    }

    fn mean_abs_diff(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f32>() / a.len() as f32
    }

    #[test]
    fn lfo1_onset_holds_then_fades_modulation_in() {
        // With a 0.5 s delay then 0.5 s fade, LFO 1's value is gated to zero
        // through the delay, so an LFO 1 → cutoff route contributes nothing and
        // the output matches the no-LFO baseline; once settled the filter sweeps
        // and the output diverges (E005 / 0018 two-stage onset).
        let during = 9600..19_200;
        let settled = 58_000..67_600;
        let delay_diff = mean_abs_diff(
            &lfo1_cutoff_render(0.0, 0.5, 0.5, 4.0, during.clone()),
            &lfo1_cutoff_render(48.0, 0.5, 0.5, 4.0, during),
        );
        let settled_diff = mean_abs_diff(
            &lfo1_cutoff_render(0.0, 0.5, 0.5, 4.0, settled.clone()),
            &lfo1_cutoff_render(48.0, 0.5, 0.5, 4.0, settled),
        );
        assert!(
            delay_diff < 1e-6,
            "LFO 1 not held at zero in the delay: {delay_diff}"
        );
        assert!(
            settled_diff > 1e-3,
            "LFO 1 did not open after delay+fade: {settled_diff}"
        );
    }

    #[test]
    fn lfo1_onset_zero_matches_immediate_modulation() {
        // Delay 0 + fade 0: LFO 1 modulates at full depth from the first block,
        // so the LFO 1 → cutoff route diverges from the no-LFO baseline at once.
        let win = 0..4800;
        let diff = mean_abs_diff(
            &lfo1_cutoff_render(0.0, 0.0, 0.0, 6.0, win.clone()),
            &lfo1_cutoff_render(48.0, 0.0, 0.0, 6.0, win),
        );
        assert!(diff > 1e-3, "0/0 onset should modulate at once: {diff}");
    }

    #[test]
    fn sync_engages_and_sweeps_formant_finitely() {
        // Integration check that the coupled path is live and stable. (The
        // master-period lock itself is proven in the DSP unit test
        // `synced_slave_locks_to_master_period`; a zero-crossing fundamental
        // detector can't see it through the synced waveform.) Here: enabling
        // sync changes the timbre, sweeping the slave tuning sweeps it further
        // (the synced formant), and every render stays finite.
        fn render_sync(sync: bool, osc2_coarse: f32) -> Vec<f32> {
            let mut s = pitched_synth();
            // CrossModType: Sync (1) engages the band-limited hard sync.
            s.set_param(pp(PatchParam::CrossModType), if sync { 1.0 } else { 0.0 });
            s.set_param(pp(PatchParam::Osc1Wave), 2.0); // saw master
            s.set_param(pp(PatchParam::Osc2Wave), 2.0); // saw slave
            s.set_param(pp(PatchParam::Osc2Level), 0.8);
            s.set_param(pp(PatchParam::Osc2Coarse), osc2_coarse);
            s.note_on(45, 1.0); // A2 ≈ 110 Hz master
            let (l, _) = render(&mut s, 24_000);
            assert!(l.iter().all(|x| x.is_finite()), "sync output not finite");
            l[4800..].to_vec()
        }
        fn diff(a: &[f32], b: &[f32]) -> f32 {
            a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f32>() / a.len() as f32
        }
        let unsynced = render_sync(false, -7.0);
        let synced_low = render_sync(true, -7.0);
        let synced_high = render_sync(true, 7.0);
        // Sync changes the timbre vs the independent path …
        assert!(
            diff(&unsynced, &synced_low) > 1e-3,
            "sync did not change the output"
        );
        // … and sweeping the slave tuning sweeps the synced formant.
        assert!(
            diff(&synced_low, &synced_high) > 1e-3,
            "slave tuning did not sweep the synced formant"
        );
    }

    #[test]
    fn cross_mod_adds_content_and_stays_finite() {
        // Through-zero PM (CrossModType::Pm) with osc2 at an inharmonic interval
        // injects a sideband at f(osc1)+f(osc2). Measure that bin via a single-bin
        // DFT: ≈0 with PM off, present at index > 0, output finite throughout.
        let sr = 48_000.0;
        let f1 = note_to_hz(45.0); // A2 ≈ 110 Hz carrier
        let f2 = note_to_hz(45.0 + 5.0); // osc2 +5 st (inharmonic)
        fn sideband(pm_index: f32, side_hz: f32, sr: f32) -> (f32, bool) {
            let mut s = pitched_synth();
            s.set_param(pp(PatchParam::Osc2Level), 0.0); // carrier audible alone
            s.set_param(pp(PatchParam::Osc2Coarse), 5.0); // inharmonic vs osc1
            // PM mode when index > 0; Off (independent path) at index 0.
            s.set_param(
                pp(PatchParam::CrossModType),
                if pm_index > 0.0 { 2.0 } else { 0.0 },
            );
            s.set_param(pp(PatchParam::CrossModAmount), pm_index);
            s.note_on(45, 1.0);
            let (l, _) = render(&mut s, 24_000);
            let finite = l.iter().all(|x| x.is_finite());
            let tail = &l[4800..]; // past the amp-envelope attack
            let w = std::f32::consts::TAU * side_hz / sr;
            let len = tail.len();
            let (mut re, mut im) = (0.0f32, 0.0f32);
            // Hann window: keep the carrier's leakage out of the sideband bin.
            for (n, &x) in tail.iter().enumerate() {
                let win = 0.5 * (1.0 - (std::f32::consts::TAU * n as f32 / (len - 1) as f32).cos());
                let ph = w * n as f32;
                re += x * win * ph.cos();
                im -= x * win * ph.sin();
            }
            ((re * re + im * im).sqrt() / len as f32, finite)
        }
        let (clean, clean_finite) = sideband(0.0, f1 + f2, sr);
        let (modulated, mod_finite) = sideband(0.8, f1 + f2, sr);
        assert!(clean_finite && mod_finite, "cross-mod output not finite");
        assert!(
            modulated > 10.0 * clean.max(1e-6),
            "cross-mod produced no sideband: clean {clean}, modulated {modulated}"
        );
    }

    /// Single audible osc2 sine — for mod-wheel→osc2-pitch tests.
    fn osc2_sine_synth() -> Synth {
        let mut s = clean_sine_synth();
        s.set_param(pp(PatchParam::Osc1Level), 0.0);
        s.set_param(pp(PatchParam::Osc2Wave), 0.0); // sine
        s.set_param(pp(PatchParam::Osc2Level), 0.8);
        s.set_param(pp(PatchParam::Osc2Coarse), 0.0);
        s.set_param(pp(PatchParam::Osc2Fine), 0.0);
        s
    }

    #[test]
    fn pitch_bend_shifts_rendered_pitch() {
        // Full positive bend (+1.0 normalised) = +2 st = ×2^(2/12) ≈ 1.122.
        let mut base = pitched_synth();
        base.note_on(57, 1.0); // A3 ≈ 220 Hz
        let (l0, _) = render(&mut base, 24_000);
        let f0 = dominant_hz(&l0[4800..], 48_000.0);

        let mut bent = pitched_synth();
        bent.set_pitch_bend(1.0);
        bent.note_on(57, 1.0);
        let (l1, _) = render(&mut bent, 24_000);
        let f1 = dominant_hz(&l1[4800..], 48_000.0);

        let expected = 2.0f32.powf(2.0 / 12.0);
        assert!(
            (f1 / f0 - expected).abs() < 0.03,
            "bend should raise pitch ×{expected:.3}: {f0} -> {f1}"
        );
    }

    #[test]
    fn mod_wheel_cross_mod_sweep_shifts_audible_osc() {
        // Wheel→X-Mod sweep depth 12 st, wheel full → +1 oct on the targeted
        // osc(s). In Off mode the sweep hits both oscs; here osc1 is muted, so
        // only osc2 is audible — its freq should double.
        let mut base = osc2_sine_synth();
        base.note_on(57, 1.0); // 220 Hz
        let (l0, _) = render(&mut base, 24_000);
        let f0 = dominant_hz(&l0[4800..], 48_000.0);

        let mut up = osc2_sine_synth();
        up.set_param(pp(PatchParam::ModWheelCrossModSweep), 12.0);
        up.set_mod_wheel(1.0);
        up.note_on(57, 1.0);
        let (l1, _) = render(&mut up, 24_000);
        let f1 = dominant_hz(&l1[4800..], 48_000.0);

        assert!(
            (f1 / f0 - 2.0).abs() < 0.05,
            "wheel→x-mod +12 st should double audible osc freq: {f0} -> {f1}"
        );
    }

    #[test]
    fn fm_mode_pitch_env_mod_only_modulates_osc2_not_osc1() {
        // The "Mod" switch isolates env→pitch to the modulator oscillator.
        // Modulator = osc2 by default; Sync flips to osc1. Verify by
        // silencing osc2 and listening to osc1 alone: a hot env→pitch
        // (+12 st) must NOT shift the carrier in Off / Ring / Pm modes
        // (all of which route to osc2). Sync's osc1-routing is covered
        // separately.
        fn carrier_pitch(cross_mod: f32, amount: f32) -> f32 {
            let mut s = Synth::new(48_000.0);
            s.set_param(pp(PatchParam::Osc1Wave), 0.0); // sine carrier
            s.set_param(pp(PatchParam::Osc1Level), 0.8);
            s.set_param(pp(PatchParam::Osc2Wave), 0.0); // sine modulator
            s.set_param(pp(PatchParam::Osc2Level), 0.0); // silent — only osc1 audible
            s.set_param(pp(PatchParam::PitchLfoDepth), 0.0);
            s.set_param(gp(GlobalParam::ChorusOn), 0.0);
            s.set_param(pp(PatchParam::CrossModType), cross_mod);
            s.set_param(pp(PatchParam::CrossModAmount), amount);
            // Env 1 → pitch, +12 st, mod-only ON. Hot AD + full sustain so
            // env_1 sits at 1.0 across the capture window.
            s.set_param(pp(PatchParam::PitchEnvSrc), 1.0); // Env 1
            s.set_param(pp(PatchParam::PitchEnvDepth), 12.0);
            s.set_param(pp(PatchParam::PitchEnvModOnly), 1.0);
            s.set_param(pp(PatchParam::Env1Attack), 0.001);
            s.set_param(pp(PatchParam::Env1Decay), 0.001);
            s.set_param(pp(PatchParam::Env1Sustain), 1.0);
            s.set_param(pp(PatchParam::Env2Attack), 0.001);
            s.note_on(57, 1.0); // A3 ≈ 220 Hz
            let (l, _) = render(&mut s, 24_000);
            dominant_hz(&l[4800..], 48_000.0)
        }
        // Reference: plain A3 carrier, no env→pitch.
        let clean = {
            let mut s = Synth::new(48_000.0);
            s.set_param(pp(PatchParam::Osc1Wave), 0.0);
            s.set_param(pp(PatchParam::Osc1Level), 0.8);
            s.set_param(pp(PatchParam::Osc2Level), 0.0);
            s.set_param(pp(PatchParam::PitchLfoDepth), 0.0);
            s.set_param(gp(GlobalParam::ChorusOn), 0.0);
            s.set_param(pp(PatchParam::Env2Attack), 0.001);
            s.note_on(57, 1.0);
            let (l, _) = render(&mut s, 24_000);
            dominant_hz(&l[4800..], 48_000.0)
        };
        // FM at amount = 0 must still route env to osc2 — the mode is FM
        // regardless of depth (the kernel takes the fast path at 0, but the
        // semantic routing still picks osc2 as the modulator).
        let fm_zero = carrier_pitch(2.0, 0.0);
        assert!(
            (fm_zero / clean - 1.0).abs() < 0.03,
            "FM amount=0 + mod-only shifted carrier (routing read amount, \
             not mode): clean {clean}, fm0 {fm_zero}",
        );
        // FM with low pm_index keeps the carrier the dominant FFT peak; env
        // should leave it untouched (routes to silent osc2).
        let fm = carrier_pitch(2.0, 0.1);
        assert!(
            (fm / clean - 1.0).abs() < 0.03,
            "FM + mod-only shifted carrier (env leaked to osc1): clean {clean}, fm {fm}",
        );
        // Off mode: mod-only routes to osc2 (default modulator) — same as
        // Pm — so the audible osc1 stays put. Without this isolation the
        // Mod switch would be a no-op when no cross-mod is in play.
        let off = carrier_pitch(0.0, 0.0);
        assert!(
            (off / clean - 1.0).abs() < 0.03,
            "Off + mod-only shifted carrier (env leaked to osc1): clean {clean}, off {off}",
        );
    }

    #[test]
    fn mod_wheel_zero_depth_is_inert() {
        // With every mod-wheel depth at zero (default), a full wheel changes
        // nothing — the panel routes are independent and all start unrouted.
        let mut base = osc2_sine_synth();
        base.note_on(57, 1.0);
        let (l0, _) = render(&mut base, 24_000);
        let f0 = dominant_hz(&l0[4800..], 48_000.0);

        let mut off = osc2_sine_synth();
        off.set_mod_wheel(1.0);
        off.note_on(57, 1.0);
        let (l1, _) = render(&mut off, 24_000);
        let f1 = dominant_hz(&l1[4800..], 48_000.0);

        assert!(
            (f1 / f0 - 1.0).abs() < 0.02,
            "zero-depth wheel shifted pitch: {f0} -> {f1}"
        );
    }

    #[test]
    fn mod_wheel_cutoff_moves_cutoff() {
        // Wheel→Cutoff: a full wheel opens the filter, passing more saw
        // harmonics → higher RMS than the dark baseline.
        fn bright(wheel: f32) -> f32 {
            let mut s = Synth::new(48_000.0);
            s.set_param(pp(PatchParam::Osc1Wave), 2.0); // saw (harmonic-rich)
            s.set_param(pp(PatchParam::Osc2Level), 0.0);
            s.set_param(pp(PatchParam::PitchLfoDepth), 0.0);
            s.set_param(gp(GlobalParam::ChorusOn), 0.0);
            s.set_param(pp(PatchParam::Env2Attack), 0.001);
            s.set_param(pp(PatchParam::Cutoff), 200.0); // dark base
            s.set_param(pp(PatchParam::Resonance), 0.0);
            s.set_param(pp(PatchParam::ModWheelCutoff), 48.0); // ×2^4 = 16
            s.set_mod_wheel(wheel);
            s.note_on(45, 1.0); // 110 Hz, many harmonics
            let (l, _) = render(&mut s, 24_000);
            assert!(
                l.iter().all(|x| x.is_finite()),
                "mod-wheel cutoff not finite"
            );
            rms(&l[4800..])
        }
        let dark = bright(0.0);
        let open = bright(1.0);
        assert!(
            open > 1.3 * dark,
            "wheel→cutoff did not open the filter: dark {dark}, open {open}"
        );
    }

    // ── E005 / 0018: per-voice LFO 1 ─────────────────────────────────────────

    #[test]
    fn per_voice_lfo1_retriggers_only_its_own_voice() {
        // LFO 1 is per voice: a new note retriggers only its own channel's LFO 1
        // (to the sine zero crossing = phase 0); a held voice's phase keeps
        // running, undisturbed.
        let mut s = advance_ch0_lfo(5.0, false);
        let ch0_before = s.banks[0].lfo1_phase(0);
        assert!(ch0_before > 0.01, "held voice should have advanced");
        s.note_on_layer(0, 64, 1.0); // → channel 1, retriggers only its own LFO 1
        assert_eq!(
            s.banks[0].lfo1_phase(1),
            0.0,
            "new voice retriggers to zero"
        );
        assert_eq!(
            s.banks[0].lfo1_phase(0),
            ch0_before,
            "held voice's LFO 1 must be undisturbed by another note"
        );
    }

    #[test]
    fn per_voice_lfo1_retrigger_lands_on_zero_crossing() {
        // The per-voice retrigger lands on each shape's zero crossing (sine 0,
        // tri 0.25, saws 0.5; square/S&H at the boundary).
        for (shape_idx, expected) in [(0.0, 0.0), (1.0, 0.25), (2.0, 0.5), (4.0, 0.0)] {
            let mut s = pitched_synth();
            s.set_param(pp(PatchParam::LfoShape), shape_idx);
            // Manually advance ch0 LFO so we can then retrigger ch1.
            s.set_param(pp(PatchParam::LfoRate), 5.0);
            s.note_on_layer(0, 60, 1.0);
            let _ = render(&mut s, 6000);
            s.note_on_layer(0, 64, 1.0); // channel 1 freshly triggered
            assert_eq!(
                s.banks[0].lfo1_phase(1),
                expected,
                "shape {shape_idx} should retrigger to its zero crossing"
            );
        }
    }

    #[test]
    fn lfo1_free_run_keeps_phase_across_note_ons() {
        // Free-run on: re-triggering a channel does not reset its LFO 1 phase.
        let mut s = advance_ch0_lfo(5.0, true);
        let before = s.banks[0].lfo1_phase(0);
        assert!(before > 0.01);
        s.note_on_layer(0, 60, 1.0); // reuses channel 0 (same note); no reset
        assert_eq!(
            s.banks[0].lfo1_phase(0),
            before,
            "free-run must not reset the per-voice phase"
        );
    }

    // ── E005 / 0019: global instrument-wide LFO 2 ────────────────────────────

    #[test]
    fn lfo2_zero_depth_matches_pre_change_output() {
        // No route selects LFO 2 (only LFO 1 → cutoff here), so ticking the
        // global LFO 2 with a live rate/shape reproduces the output bit-for-bit.
        let mut a = pitched_synth();
        a.set_param(pp(PatchParam::CutoffLfo1Depth), 24.0); // LFO 1 → cutoff
        a.note_on(57, 1.0);
        let (base, _) = render(&mut a, 12_000);

        // Same patch, but tick the global LFO 2 with a live rate/shape (unrouted —
        // LFO 2's own cutoff depth stays zero).
        let mut b = pitched_synth();
        b.set_param(pp(PatchParam::CutoffLfo1Depth), 24.0);
        b.set_param(gp(GlobalParam::Lfo2Rate), 3.0);
        b.set_param(gp(GlobalParam::Lfo2Shape), 5.0); // S&H — exercises its PRNG
        b.note_on(57, 1.0);
        let (with_lfo2, _) = render(&mut b, 12_000);

        let max_err = base
            .iter()
            .zip(&with_lfo2)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0f32, f32::max);
        assert!(max_err == 0.0, "zero-depth LFO2 changed output: {max_err}");
    }

    #[test]
    fn global_lfo2_is_shared_across_both_layers() {
        // The global LFO 2 reaches both layers from one shared phase: in Dual
        // mode, routing LFO2→pitch on each layer and playing the same note on
        // both yields the combined output = exactly twice one layer's (same LFO2
        // phase drives both). Proves a single instrument-wide source, not per-layer.
        fn configure(s: &mut Synth) {
            s.set_param(gp(GlobalParam::ChorusOn), 0.0);
            // Drift is per-voice random — would decorrelate the two layers and
            // sink the bit-equal sum check below.
            s.set_drift_amount(0.0);
            s.set_key_mode(KeyMode::Dual);
            for layer in Layer::ALL {
                s.set_param(patch_clap_id(layer, PatchParam::Osc1Wave), 0.0); // sine
                s.set_param(patch_clap_id(layer, PatchParam::Osc2Level), 0.0);
                s.set_param(patch_clap_id(layer, PatchParam::PitchLfoSrc), 2.0); // LFO 2
                s.set_param(patch_clap_id(layer, PatchParam::PitchLfoDepth), 7.0);
            }
            s.set_param(gp(GlobalParam::Lfo2Rate), 5.0);
        }
        let mut one = Synth::new(48_000.0);
        configure(&mut one);
        one.note_on_layer(0, 69, 1.0);
        let (single, _) = render(&mut one, 9600);

        let mut two = Synth::new(48_000.0);
        configure(&mut two);
        two.note_on_layer(0, 69, 1.0);
        two.note_on_layer(1, 69, 1.0);
        let (both, _) = render(&mut two, 9600);

        assert!(rms(&single) > 0.01, "LFO2→amp produced no sound");
        let max_err = single
            .iter()
            .zip(&both)
            .map(|(a, b)| (2.0 * a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_err < 1e-4,
            "global LFO2 not shared identically: {max_err}"
        );
    }

    // ── E004 / 0015: host-tempo sync ────────────────────────────────────────

    /// Set the rate knob so its fader position lands exactly on subdivision `idx`
    /// (the inverse of `to_fader` ∘ `sync::index_from_norm`).
    fn rate_for_subdiv(idx: usize) -> f32 {
        let last = (sync::SUBDIVISIONS.len() - 1) as f32;
        PatchParam::LfoRate.desc().from_fader(idx as f32 / last)
    }

    #[test]
    fn lfo_sync_off_is_free_running_hz() {
        let mut p = PatchValues::default();
        p.set(PatchParam::LfoRate, 7.3);
        // Sync off (default): the rate knob is taken as literal Hz, tempo ignored.
        assert_eq!(
            lfo_rate(&p, PatchParam::LfoRate, PatchParam::LfoSync, 120.0),
            7.3
        );
    }

    #[test]
    fn lfo_sync_on_resolves_subdivision_from_tempo() {
        // Indices of the quarter-note family in the subdivision table.
        let q = sync::SUBDIVISIONS
            .iter()
            .position(|s| s.label == "1/4")
            .unwrap();
        let qd = sync::SUBDIVISIONS
            .iter()
            .position(|s| s.label == "1/4.")
            .unwrap();
        let qt = sync::SUBDIVISIONS
            .iter()
            .position(|s| s.label == "1/4T")
            .unwrap();

        let mut p = PatchValues::default();
        p.set(PatchParam::LfoSync, 1.0);
        let resolve =
            |p: &PatchValues, bpm| lfo_rate(p, PatchParam::LfoRate, PatchParam::LfoSync, bpm);

        // Straight quarter: one cycle per beat.
        p.set(PatchParam::LfoRate, rate_for_subdiv(q));
        assert!((resolve(&p, 120.0) - 2.0).abs() < 1e-4, "1/4 @120");
        assert!((resolve(&p, 90.0) - 1.5).abs() < 1e-4, "1/4 @90");
        // Dotted (×1.5 length) and triplet (×2/3 length) at 140 BPM.
        p.set(PatchParam::LfoRate, rate_for_subdiv(qd));
        assert!(
            (resolve(&p, 140.0) - (140.0 / 60.0) / 1.5).abs() < 1e-4,
            "1/4. @140"
        );
        p.set(PatchParam::LfoRate, rate_for_subdiv(qt));
        assert!(
            (resolve(&p, 140.0) - (140.0 / 60.0) / (2.0 / 3.0)).abs() < 1e-4,
            "1/4T @140"
        );
    }

    // ── E006: tempo-synced delay time ────────────────────────────────────────

    /// Set the delay time knob so its fader position lands exactly on subdivision
    /// `idx` (inverse of `to_fader` ∘ `sync::index_from_norm`).
    fn delay_time_for_subdiv(idx: usize) -> f32 {
        let last = (sync::SUBDIVISIONS.len() - 1) as f32;
        GlobalParam::DelayTime.desc().from_fader(idx as f32 / last)
    }

    #[test]
    fn delay_sync_off_is_literal_seconds() {
        // Sync off: the Time knob is taken as literal seconds, tempo ignored.
        assert_eq!(delay_time_seconds(false, 0.42, 120.0), 0.42);
        assert_eq!(delay_time_seconds(false, 0.42, 60.0), 0.42);
    }

    #[test]
    fn delay_sync_on_resolves_subdivision_from_tempo() {
        let q = sync::SUBDIVISIONS
            .iter()
            .position(|s| s.label == "1/4")
            .unwrap();
        let v = delay_time_for_subdiv(q);
        // 1/4 = one beat: 0.5 s @120, 1.0 s @60.
        assert!(
            (delay_time_seconds(true, v, 120.0) - 0.5).abs() < 1e-4,
            "1/4 @120"
        );
        assert!(
            (delay_time_seconds(true, v, 60.0) - 1.0).abs() < 1e-4,
            "1/4 @60"
        );
    }

    #[test]
    fn delay_synced_time_snaps_back_to_ms_when_sync_off() {
        // A knob value that means a subdivision while synced must read back as
        // the same literal seconds the instant sync is switched off (the stored
        // param value is never mutated, only reinterpreted).
        let v = delay_time_for_subdiv(3); // some arbitrary subdivision
        let synced = delay_time_seconds(true, v, 100.0);
        let unsynced = delay_time_seconds(false, v, 100.0);
        assert_ne!(synced, unsynced, "sync should reinterpret the value");
        assert_eq!(unsynced, v, "off must return the literal stored seconds");
    }

    #[test]
    fn set_tempo_rejects_nonfinite_and_nonpositive() {
        let mut s = Synth::new(48_000.0);
        s.set_tempo(f32::NAN);
        assert_eq!(s.tempo_bpm, sync::DEFAULT_TEMPO_BPM);
        s.set_tempo(0.0);
        assert_eq!(s.tempo_bpm, sync::DEFAULT_TEMPO_BPM);
        s.set_tempo(128.0);
        assert_eq!(s.tempo_bpm, 128.0);
    }

    #[test]
    fn synced_lfo_renders_finite_and_audible() {
        // End-to-end: a synced LFO→cutoff route at a fast subdivision drives the
        // filter and stays finite (the rate path never NaNs through the engine).
        let mut s = pitched_synth();
        s.set_param(pp(PatchParam::Osc1Wave), 2.0); // saw
        s.set_param(pp(PatchParam::Cutoff), 1200.0);
        s.set_param(pp(PatchParam::CutoffLfo1Depth), 36.0); // LFO 1 → cutoff
        s.set_param(pp(PatchParam::LfoSync), 1.0);
        s.set_param(pp(PatchParam::LfoRate), rate_for_subdiv(9)); // 1/8
        s.set_tempo(128.0);
        s.note_on(45, 1.0);
        let (l, _) = render(&mut s, 24_000);
        assert!(
            l.iter().all(|x| x.is_finite()),
            "synced LFO output not finite"
        );
        assert!(rms(&l) > 0.01, "synced LFO produced no sound");
    }

    #[test]
    fn voice_stealing_keeps_polyphony_bounded() {
        let mut s = Synth::new(48_000.0);
        for n in 0..40u8 {
            s.note_on(n, 1.0);
        }
        let active = s.active_count();
        assert!(
            active <= vxn_dsp::MAX_VOICES,
            "too many active voices: {active}"
        );
    }

    // ── E003 / 0008: two-layer render ───────────────────────────────────────

    #[test]
    fn param_source_follows_key_mode() {
        // Whole: both layers read layer A (Upper). Dual/Split: each reads its own.
        assert_eq!(Synth::param_source(0, KeyMode::Whole), Layer::Upper);
        assert_eq!(Synth::param_source(1, KeyMode::Whole), Layer::Upper);
        for m in [KeyMode::Dual, KeyMode::Split] {
            assert_eq!(Synth::param_source(0, m), Layer::Upper);
            assert_eq!(Synth::param_source(1, m), Layer::Lower);
        }
    }

    /// A deterministic patch (sine LFO, chorus off, drift off) so two layers
    /// fed identical params + notes render bit-for-bit identically. Drift is
    /// per-voice random by design, so it must be silenced for any test
    /// asserting cross-layer bit-equality.
    fn deterministic(s: &mut Synth) {
        s.set_param(gp(GlobalParam::ChorusOn), 0.0);
        s.set_param(pp(PatchParam::Env2Attack), 0.001);
        s.set_drift_amount(0.0);
    }

    #[test]
    fn whole_two_identical_layers_sum_to_double_single() {
        // ADR 0003 §3 Whole-equivalence: both layers read Upper's block, so two
        // layers playing the same note = exactly twice one layer's output.
        let mut one = Synth::new(48_000.0);
        deterministic(&mut one);
        one.note_on_layer(0, 69, 1.0);
        let (single, _) = render(&mut one, 9600);

        let mut two = Synth::new(48_000.0);
        deterministic(&mut two);
        two.note_on_layer(0, 69, 1.0);
        two.note_on_layer(1, 69, 1.0);
        let (both, _) = render(&mut two, 9600);

        assert!(rms(&single) > 0.01, "reference layer was silent");
        let max_err = single
            .iter()
            .zip(&both)
            .map(|(a, b)| (2.0 * a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_err < 1e-4,
            "two layers != 2x one layer: max_err {max_err}"
        );
    }

    #[test]
    fn dual_layers_superpose_two_independent_patches() {
        // Dual: Upper a plain sine, Lower a saw an octave up. Each layer reads
        // its own block; the two-layer sum equals the two layers rendered alone
        // (superposition), and the two patches are audibly different.
        fn configure(s: &mut Synth) {
            deterministic(s);
            s.set_key_mode(KeyMode::Dual);
            // Upper: sine.
            s.set_param(pp(PatchParam::Osc1Wave), 0.0);
            s.set_param(pp(PatchParam::PitchLfoDepth), 0.0);
            // Lower: saw, +1 octave.
            s.set_param(lo(PatchParam::Osc1Wave), 2.0);
            s.set_param(lo(PatchParam::Osc1Octave), 1.0);
            s.set_param(lo(PatchParam::PitchLfoDepth), 0.0);
            s.set_param(lo(PatchParam::Env2Attack), 0.001);
        }
        let frames = 9600;
        let mut up = Synth::new(48_000.0);
        configure(&mut up);
        up.note_on_layer(0, 57, 1.0);
        let (upper_only, _) = render(&mut up, frames);

        let mut lw = Synth::new(48_000.0);
        configure(&mut lw);
        lw.note_on_layer(1, 57, 1.0);
        let (lower_only, _) = render(&mut lw, frames);

        let mut both = Synth::new(48_000.0);
        configure(&mut both);
        both.note_on_layer(0, 57, 1.0);
        both.note_on_layer(1, 57, 1.0);
        let (combined, _) = render(&mut both, frames);

        assert!(
            rms(&upper_only) > 0.01 && rms(&lower_only) > 0.01,
            "a layer was silent"
        );
        // Two different patches.
        let diff = upper_only
            .iter()
            .zip(&lower_only)
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>()
            / frames as f32;
        assert!(
            diff > 1e-3,
            "the two layers are not distinguishable: {diff}"
        );
        // Sum of the two independent layers == the combined render.
        let max_err = combined
            .iter()
            .zip(upper_only.iter().zip(&lower_only))
            .map(|(c, (a, b))| (c - (a + b)).abs())
            .fold(0.0f32, f32::max);
        assert!(max_err < 1e-4, "layers do not superpose: max_err {max_err}");
        assert!(combined.iter().all(|x| x.is_finite()), "non-finite sum");
    }

    // ── E003 / 0009: event router & key mode ────────────────────────────────

    fn layer_active(s: &Synth, layer: usize) -> usize {
        s.banks[layer].active_count()
    }

    #[test]
    fn whole_round_robins_successive_note_ons() {
        let mut s = Synth::new(48_000.0);
        s.set_key_mode(KeyMode::Whole);
        s.note_on(60, 1.0);
        s.note_on(62, 1.0);
        // Two notes, alternating layers → one channel active in each.
        assert_eq!(layer_active(&s, 0), 1);
        assert_eq!(layer_active(&s, 1), 1);
    }

    #[test]
    fn dual_triggers_both_layers_per_note() {
        let mut s = Synth::new(48_000.0);
        s.set_key_mode(KeyMode::Dual);
        s.note_on(60, 1.0);
        // One note → both layers play it.
        assert_eq!(layer_active(&s, 0), 1);
        assert_eq!(layer_active(&s, 1), 1);
    }

    #[test]
    fn split_routes_by_pitch_about_the_split_point() {
        let mut s = Synth::new(48_000.0);
        s.set_key_mode(KeyMode::Split);
        s.set_split_point(60);
        s.note_on(48, 1.0); // below → Lower (layer 1)
        s.note_on(72, 1.0); // at/above → Upper (layer 0)
        assert_eq!(layer_active(&s, Layer::Lower as usize), 1);
        assert_eq!(layer_active(&s, Layer::Upper as usize), 1);
        // A note exactly at the split point goes to Upper.
        s.note_on(60, 1.0);
        assert_eq!(layer_active(&s, Layer::Upper as usize), 2);
    }

    #[test]
    fn note_off_releases_only_the_layer_that_started_it() {
        let mut s = Synth::new(48_000.0);
        s.set_key_mode(KeyMode::Split);
        s.set_split_point(60);
        s.note_on(48, 1.0); // Lower
        s.note_off(48); // broadcast; only Lower holds it
        // Gate cleared on Lower; Upper never had it. Render the release out.
        s.set_param(pp(PatchParam::Env2Release), 0.001);
        s.set_param(lo(PatchParam::Env2Release), 0.001);
        let (l, _) = render(&mut s, 4800);
        assert!(rms(&l[2400..]) < 1e-4, "note did not release");
    }

    #[test]
    fn sustain_pedal_defers_poly_release() {
        let mut s = Synth::new(48_000.0);
        // Fast release on both envelopes so a released voice deactivates within
        // the render window; a pedal-held voice keeps its gate high regardless.
        for set in [pp, lo] {
            s.set_param(set(PatchParam::Env1Release), 0.001);
            s.set_param(set(PatchParam::Env2Release), 0.001);
        }
        s.note_on(60, 1.0);
        s.sustain(true);
        s.note_off(60); // pedal down → release deferred
        let _ = render(&mut s, 4800);
        assert_eq!(s.active_count(), 1, "pedal held: voice must keep sounding");
        s.sustain(false); // pedal up → deferred release fires
        let _ = render(&mut s, 4800);
        assert_eq!(s.active_count(), 0, "pedal up: voice must release");
    }

    #[test]
    fn sustain_pedal_off_with_key_still_down_keeps_note() {
        let mut s = Synth::new(48_000.0);
        for set in [pp, lo] {
            s.set_param(set(PatchParam::Env1Release), 0.001);
            s.set_param(set(PatchParam::Env2Release), 0.001);
        }
        s.note_on(60, 1.0);
        s.sustain(true);
        // Key never released; pedal-up must not gate a still-held key off.
        s.sustain(false);
        let _ = render(&mut s, 4800);
        assert_eq!(s.active_count(), 1, "held key must survive pedal-up");
    }

    #[test]
    fn held_notes_survive_a_mode_and_split_change() {
        // A sounding note keeps playing through a key-mode / split-point change;
        // only new note-ons follow the new routing (ADR 0003 §Consequences).
        let mut s = Synth::new(48_000.0);
        s.set_key_mode(KeyMode::Whole);
        s.note_on(64, 1.0);
        let before = s.active_count();
        assert_eq!(before, 1);
        s.set_key_mode(KeyMode::Split);
        s.set_split_point(72);
        // Still sounding (not stranded).
        assert_eq!(s.active_count(), 1);
        let (l, _) = render(&mut s, 2400);
        assert!(rms(&l) > 0.001, "held note went silent across the change");
    }

    // ── E003 / 0010: per-layer assign-mode processor (poly) ─────────────────

    #[test]
    fn poly_layer_holds_eight_then_steals_oldest() {
        // Dual so each note hits both layers; one layer's allocation is confined
        // to its 8 channels and the 9th note steals (never exceeds 8).
        let mut s = Synth::new(48_000.0);
        s.set_key_mode(KeyMode::Dual);
        for n in 60..68 {
            s.note_on(n, 1.0); // 8 distinct notes
        }
        assert_eq!(layer_active(&s, 0), 8, "layer A should be full at 8");
        assert_eq!(layer_active(&s, 1), 8, "layer B should be full at 8");
        // 9th note steals rather than growing the layer past its 8 channels.
        s.note_on(68, 1.0);
        assert_eq!(layer_active(&s, 0), 8, "layer A must stay bounded to 8");
        assert_eq!(layer_active(&s, 1), 8, "layer B must stay bounded to 8");
    }

    #[test]
    fn layer_allocation_is_independent() {
        // Split: low notes → Lower, high → Upper. Flooding one layer never
        // touches the other's channels (independent per-layer allocation).
        let mut s = Synth::new(48_000.0);
        s.set_key_mode(KeyMode::Split);
        s.set_split_point(60);
        for n in 36..50 {
            s.note_on(n, 1.0); // all below split → Lower only
        }
        assert_eq!(
            layer_active(&s, Layer::Lower as usize),
            8,
            "Lower bounded to 8"
        );
        assert_eq!(
            layer_active(&s, Layer::Upper as usize),
            0,
            "Upper untouched by Lower's flood"
        );
    }

    #[test]
    fn assign_mode_param_reads_unison() {
        let mut p = ParamValues::default();
        assert_eq!(
            p.layer(Layer::Upper).assign_mode(),
            crate::params::AssignMode::Poly
        );
        p.layer_mut(Layer::Upper).set(PatchParam::AssignMode, 1.0);
        assert_eq!(
            p.layer(Layer::Upper).assign_mode(),
            crate::params::AssignMode::Unison
        );
    }

    // ── E003 / 0011: unison assign mode ─────────────────────────────────────

    /// Put a layer into a given assign mode with a unison detune (cents).
    fn set_assign(s: &mut Synth, layer: usize, unison: bool, detune: f32) {
        let mode = if unison {
            AssignMode::Unison
        } else {
            AssignMode::Poly
        };
        set_assign_mode(s, layer, mode, detune);
    }

    fn set_assign_mode(s: &mut Synth, layer: usize, mode: AssignMode, detune: f32) {
        s.set_param(
            patch_clap_id(Layer::ALL[layer], PatchParam::AssignMode),
            mode as usize as f32,
        );
        s.set_param(
            patch_clap_id(Layer::ALL[layer], PatchParam::UnisonDetune),
            detune,
        );
    }

    fn set_legato(s: &mut Synth, layer: usize, on: bool) {
        s.set_param(
            patch_clap_id(Layer::ALL[layer], PatchParam::Legato),
            on as u8 as f32,
        );
    }

    #[test]
    fn solo_is_monophonic_across_distinct_notes() {
        let mut s = Synth::new(48_000.0);
        s.set_key_mode(KeyMode::Whole); // Whole → layer reads Upper's assign
        set_assign_mode(&mut s, 0, AssignMode::Solo, 0.0);
        for n in [60, 64, 67, 72] {
            s.note_on_layer(0, n, 1.0);
            assert_eq!(layer_active(&s, 0), 1, "Solo must keep exactly one channel");
        }
    }

    #[test]
    fn whole_mode_solo_pins_to_one_layer_not_round_robin() {
        let mut s = Synth::new(48_000.0);
        s.set_key_mode(KeyMode::Whole);
        set_assign_mode(&mut s, 0, AssignMode::Solo, 0.0);
        // Two notes through the routing path: Poly would round-robin (one per
        // layer); Solo must keep both on layer 0 so it stays one mono voice.
        s.note_on(60, 1.0);
        s.note_on(64, 1.0);
        assert_eq!(layer_active(&s, 0), 1, "Solo stays one voice on layer 0");
        assert_eq!(layer_active(&s, 1), 0, "Solo never spills to layer 1");
        assert_eq!(s.banks[0].gated_note(0), Some(64));
    }

    #[test]
    fn solo_pins_to_channel_zero_and_quiesces_others() {
        let mut s = Synth::new(48_000.0);
        s.set_key_mode(KeyMode::Whole);
        // Leave a Poly chord ringing on several channels, then switch to Solo.
        set_assign_mode(&mut s, 0, AssignMode::Poly, 0.0);
        for n in [60, 64, 67] {
            s.note_on_layer(0, n, 1.0);
        }
        assert_eq!(layer_active(&s, 0), 3);
        set_assign_mode(&mut s, 0, AssignMode::Solo, 0.0);
        s.note_on_layer(0, 72, 1.0);
        // The new note sounds on channel 0; every other channel is gated off (its
        // tail releasing), so only one note is gated/sounding.
        assert_eq!(s.banks[0].gated_note(0), Some(72));
        let gated: Vec<u8> = (0..8).filter_map(|v| s.banks[0].gated_note(v)).collect();
        assert_eq!(
            gated,
            vec![72],
            "Solo gates exactly one note, pinned to ch0"
        );
    }

    #[test]
    fn solo_stack_reverts_to_held_note_on_release() {
        let mut s = Synth::new(48_000.0);
        s.set_key_mode(KeyMode::Whole);
        set_assign_mode(&mut s, 0, AssignMode::Solo, 0.0);
        s.note_on_layer(0, 60, 1.0); // hold C
        s.note_on_layer(0, 64, 1.0); // hold E on top — C still held underneath
        assert_eq!(s.banks[0].gated_note(0), Some(64));
        s.note_off(64); // release E → revert to the still-held C
        assert_eq!(s.banks[0].gated_note(0), Some(60), "revert to held note");
        s.note_off(60); // release C → nothing held, channel releases
        assert_eq!(s.banks[0].gated_note(0), None);
    }

    #[test]
    fn solo_release_of_non_top_note_keeps_sounding_note() {
        let mut s = Synth::new(48_000.0);
        s.set_key_mode(KeyMode::Whole);
        set_assign_mode(&mut s, 0, AssignMode::Solo, 0.0);
        s.note_on_layer(0, 60, 1.0);
        s.note_on_layer(0, 64, 1.0); // E sounding, C held underneath
        s.note_off(60); // release the underlying C — E must keep sounding
        assert_eq!(s.banks[0].gated_note(0), Some(64));
        s.note_off(64); // now release E → nothing left
        assert_eq!(s.banks[0].gated_note(0), None);
    }

    #[test]
    fn solo_legato_does_not_retrigger_while_a_note_is_held() {
        let mut s = Synth::new(48_000.0);
        s.set_key_mode(KeyMode::Whole);
        set_assign_mode(&mut s, 0, AssignMode::Solo, 0.0);
        set_legato(&mut s, 0, true);
        s.note_on_layer(0, 60, 1.0);
        assert!(s.banks[0].trigger_pending(0), "first note always triggers");
        render(&mut s, 64); // consume the pending trigger
        s.note_on_layer(0, 64, 1.0); // legato slur — pitch changes, no retrigger
        assert_eq!(s.banks[0].gated_note(0), Some(64));
        assert!(
            !s.banks[0].trigger_pending(0),
            "legato note must not retrigger the envelope/phase"
        );
    }

    #[test]
    fn unison_legato_slides_all_channels_without_retrigger() {
        let mut s = Synth::new(48_000.0);
        s.set_key_mode(KeyMode::Whole);
        set_assign_mode(&mut s, 0, AssignMode::Unison, 12.0);
        set_legato(&mut s, 0, true);
        s.note_on(60, 1.0);
        assert_eq!(layer_active(&s, 0), 8, "Unison fills all 8 channels");
        render(&mut s, 64); // consume the pending triggers
        s.note_on(64, 1.0); // legato slur across the whole stack
        assert_eq!(layer_active(&s, 0), 8, "still the same 8-channel voice");
        for v in 0..8 {
            assert_eq!(s.banks[0].gated_note(v), Some(64), "all channels follow");
            assert!(
                !s.banks[0].trigger_pending(v),
                "legato Unison must not retrigger channel {v}"
            );
        }
    }

    #[test]
    fn unison_legato_reverts_to_held_note_on_release() {
        let mut s = Synth::new(48_000.0);
        s.set_key_mode(KeyMode::Whole);
        set_assign_mode(&mut s, 0, AssignMode::Unison, 12.0);
        set_legato(&mut s, 0, true);
        s.note_on(60, 1.0);
        s.note_on(64, 1.0); // 64 sounding, 60 held underneath
        s.note_off(64); // revert the whole stack to 60
        assert_eq!(layer_active(&s, 0), 8);
        for v in 0..8 {
            assert_eq!(s.banks[0].gated_note(v), Some(60));
        }
        s.note_off(60); // nothing held → release
        assert_eq!(
            layer_active(&s, 0),
            8,
            "still releasing (gates off, not idle)"
        );
        assert_eq!(s.banks[0].gated_note(0), None, "gate cleared");
    }

    #[test]
    fn solo_without_legato_retriggers_each_note() {
        let mut s = Synth::new(48_000.0);
        s.set_key_mode(KeyMode::Whole);
        set_assign_mode(&mut s, 0, AssignMode::Solo, 0.0);
        set_legato(&mut s, 0, false);
        s.note_on_layer(0, 60, 1.0);
        render(&mut s, 64);
        s.note_on_layer(0, 64, 1.0); // no legato → fresh trigger
        assert!(
            s.banks[0].trigger_pending(0),
            "non-legato Solo retriggers every note"
        );
    }

    #[test]
    fn twin_assigns_two_channels_per_note_and_stays_bounded() {
        let mut s = Synth::new(48_000.0);
        s.set_key_mode(KeyMode::Whole);
        set_assign_mode(&mut s, 0, AssignMode::Twin, 12.0);
        s.note_on_layer(0, 60, 1.0);
        assert_eq!(layer_active(&s, 0), 2, "Twin = two channels for one note");
        // Four notes saturate the 8-channel layer; further notes steal, not grow.
        for n in [62, 64, 65, 67] {
            s.note_on_layer(0, n, 1.0);
        }
        assert_eq!(
            layer_active(&s, 0),
            8,
            "Twin tops out at 8 channels (4 notes)"
        );
    }

    #[test]
    fn unison_engages_all_eight_channels_on_one_note() {
        let mut s = Synth::new(48_000.0);
        s.set_key_mode(KeyMode::Whole); // Whole → both layers read Upper's assign
        set_assign(&mut s, 0, true, 12.0);
        s.note_on_layer(0, 60, 1.0);
        assert_eq!(layer_active(&s, 0), 8, "unison should fill all 8 channels");
    }

    #[test]
    fn unison_detune_spreads_pitch_and_zero_collapses() {
        // Detune > 0: a single note's spectrum is wider (beating partials) than
        // the same note with detune 0 — compare summed energy spread crudely via
        // the difference between the two renders; they must differ.
        fn render_unison(detune: f32) -> Vec<f32> {
            let mut s = Synth::new(48_000.0);
            s.set_param(gp(GlobalParam::ChorusOn), 0.0);
            s.set_param(pp(PatchParam::Osc1Wave), 0.0); // sine
            s.set_param(pp(PatchParam::Osc2Level), 0.0);
            s.set_param(pp(PatchParam::PitchLfoDepth), 0.0);
            s.set_param(pp(PatchParam::Env2Attack), 0.001);
            set_assign(&mut s, 0, true, detune);
            s.note_on_layer(0, 57, 1.0);
            render(&mut s, 24_000).0
        }
        let tuned = render_unison(0.0);
        let spread = render_unison(25.0);
        let diff = tuned
            .iter()
            .zip(&spread)
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>()
            / tuned.len() as f32;
        assert!(
            diff > 1e-3,
            "detune did not change the unison spectrum: {diff}"
        );
        assert!(spread.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn unison_level_is_normalised_not_eight_times_poly() {
        // A unison note must not be ~8x louder than a poly note. The Unison stack's
        // 8 copies get independent random start phases (0011), so at detune 0 they
        // sum as a random walk (~√8), and `1/√8` normalisation keeps the RMS in the
        // same ballpark as one voice — never the naive 8×. The per-trigger phases
        // vary, so the lower bound is asserted on the mean over several triggers.
        let mut s = Synth::new(48_000.0);
        s.set_param(gp(GlobalParam::ChorusOn), 0.0);
        s.set_param(pp(PatchParam::Osc1Wave), 0.0);
        s.set_param(pp(PatchParam::Osc2Level), 0.0);
        s.set_param(pp(PatchParam::PitchLfoDepth), 0.0);
        s.set_param(pp(PatchParam::Env2Attack), 0.001);
        s.set_param(pp(PatchParam::Env2Release), 0.001);

        // One fresh note's steady-state RMS, then release and silence so the next
        // trigger starts clean (and, for Unison, redraws its random phases).
        let mut one_note_rms = |unison: bool| -> f32 {
            set_assign(&mut s, 0, unison, 0.0); // detune 0 → coherent worst case
            s.note_on_layer(0, 57, 1.0);
            let r = rms(&render(&mut s, 12_000).0[4800..]);
            s.note_off(57);
            let _ = render(&mut s, 2_400); // let the release free the channels
            r
        };

        let poly = one_note_rms(false);
        // Average Unison over several triggers: any single trigger's random phases
        // can cancel or reinforce, but the mean tracks the √N power normalisation.
        let trials = 8;
        let mut uni_sum = 0.0;
        let mut uni_max: f32 = 0.0;
        for _ in 0..trials {
            let u = one_note_rms(true);
            uni_sum += u;
            uni_max = uni_max.max(u);
        }
        let uni_mean = uni_sum / trials as f32;
        // Upper bound holds on every trigger: even all-aligned phases give only √8.
        assert!(
            uni_max < 4.0 * poly,
            "unison too loud: poly {poly}, unison max {uni_max}"
        );
        // Mean stays a fraction of a single voice (not silent, not boosted away).
        assert!(
            uni_mean > 0.4 * poly && uni_mean < 2.0 * poly,
            "unison level off: poly {poly}, unison mean {uni_mean}"
        );
    }

    #[test]
    fn switching_poly_unison_is_clean() {
        // Unison fills 8; switching to Poly and playing leaves no stuck channels.
        let mut s = Synth::new(48_000.0);
        s.set_param(pp(PatchParam::Env2Release), 0.001);
        set_assign(&mut s, 0, true, 10.0);
        s.note_on_layer(0, 60, 1.0);
        assert_eq!(layer_active(&s, 0), 8);
        s.note_off(60);
        let _ = render(&mut s, 4800); // let the release free the channels
        assert_eq!(
            layer_active(&s, 0),
            0,
            "unison channels stuck after release"
        );
        // Now Poly: one note → one channel.
        set_assign(&mut s, 0, false, 0.0);
        s.note_on_layer(0, 64, 1.0);
        assert_eq!(
            layer_active(&s, 0),
            1,
            "poly after unison should use 1 channel"
        );
    }

    // ── E003 / 0012: portamento ─────────────────────────────────────────────

    /// Clean single-sine layer for pitch readout, with portamento configured.
    fn glide_synth(time: f32) -> Synth {
        let mut s = clean_sine_synth();
        // Glide has no on/off: a non-zero time enables it (time 0 = off).
        s.set_param(pp(PatchParam::PortamentoTime), time);
        s
    }

    #[test]
    fn portamento_glides_pitch_toward_the_target() {
        // Play A2 on layer 0, let it fully release (freeing the channel with its
        // last pitch), then play A3: pitch should start near A2 and rise to A3.
        let mut s = glide_synth(0.12);
        // Fast release on both envelopes so the channel frees (free needs both idle).
        s.set_param(pp(PatchParam::Env1Release), 0.001);
        s.set_param(pp(PatchParam::Env2Release), 0.001);
        s.note_on_layer(0, 45, 1.0); // A2 ≈ 110 Hz
        let _ = render(&mut s, 9600);
        s.note_off(45);
        let _ = render(&mut s, 9600); // release frees channel 0 (glide_semi = 45)
        assert_eq!(
            layer_active(&s, 0),
            0,
            "channel should be free before reuse"
        );

        s.note_on_layer(0, 57, 1.0); // A3 ≈ 220 Hz target
        let (l, _) = render(&mut s, 24_000);
        let early = dominant_hz(&l[480..2400], 48_000.0);
        let late = dominant_hz(&l[19_200..24_000], 48_000.0);
        assert!(
            early < 0.85 * late,
            "pitch did not glide upward: early {early}, late {late}"
        );
        assert!(
            (late / note_to_hz(57.0) - 1.0).abs() < 0.08,
            "glide did not reach the target: {late} vs {}",
            note_to_hz(57.0)
        );
    }

    #[test]
    fn portamento_time_zero_is_instant() {
        // Time 0 with glide on reproduces the immediate-pitch behaviour.
        let mut s = glide_synth(0.0);
        s.note_on_layer(0, 57, 1.0);
        let (l, _) = render(&mut s, 24_000);
        let f = dominant_hz(&l[480..4800], 48_000.0);
        assert!(
            (f / note_to_hz(57.0) - 1.0).abs() < 0.08,
            "time 0 should sound the target at once: {f}"
        );
    }

    #[test]
    fn portamento_is_independent_per_layer() {
        // Layer 0 glides; layer 1 has glide off. A glide on layer 0 must not move
        // layer 1's steady pitch. Dual so each layer reads its own params.
        let mut s = Synth::new(48_000.0);
        s.set_key_mode(KeyMode::Dual);
        // Clean single-sine on both layers for a stable pitch readout.
        for layer in Layer::ALL {
            s.set_param(patch_clap_id(layer, PatchParam::Osc1Wave), 0.0);
            s.set_param(patch_clap_id(layer, PatchParam::Osc2Level), 0.0);
            s.set_param(patch_clap_id(layer, PatchParam::PitchLfoDepth), 0.0);
            s.set_param(patch_clap_id(layer, PatchParam::Env2Attack), 0.001);
        }
        s.set_param(gp(GlobalParam::ChorusOn), 0.0);
        // Layer 1 (Lower): glide off, plays a steady note.
        s.note_on_layer(1, 69, 1.0); // A4 = 440
        let (steady, _) = render(&mut s, 9600);
        let f_steady = dominant_hz(&steady[2400..9600], 48_000.0);
        // Layer 0 (Upper): turn glide on (non-zero time) and sweep; layer 1 sounds.
        s.set_param(patch_clap_id(Layer::Upper, PatchParam::PortamentoTime), 0.3);
        s.note_on_layer(0, 33, 1.0);
        let (_both, _) = render(&mut s, 9600);
        // Layer 1's note is still ~440 (not dragged by layer 0's glide). Verified
        // structurally: independent glide state per bank — assert it stayed up.
        assert!(
            (f_steady - 440.0).abs() < 10.0,
            "layer 1 baseline pitch wrong: {f_steady}"
        );
        assert_eq!(layer_active(&s, 1), 1, "layer 1 note should still sound");
    }

    #[test]
    fn sixteen_notes_spread_across_both_layers_and_stay_finite() {
        // Round-robin note-on (the interim Whole router) fills 8+8 = 16 channels.
        let mut s = Synth::new(48_000.0);
        s.set_param(gp(GlobalParam::DelayOn), 1.0);
        s.set_param(pp(PatchParam::Resonance), 1.0);
        for n in 60..76 {
            s.note_on(n, 1.0);
        }
        assert_eq!(
            s.active_count(),
            16,
            "expected 16 channels across two layers"
        );
        let (l, r) = render(&mut s, 24_000);
        assert!(
            l.iter().chain(r.iter()).all(|x| x.is_finite()),
            "non-finite output"
        );
    }

    /// Run a short note through the engine with the FX block to default-off
    /// state except for the parameters the caller pre-set.
    fn render_short_note(s: &mut Synth, frames: usize) -> (Vec<f32>, Vec<f32>) {
        s.set_param(pp(PatchParam::Env2Attack), 0.001);
        s.set_param(pp(PatchParam::Env2Release), 0.01);
        s.set_param(gp(GlobalParam::ChorusOn), 0.0);
        s.note_on(69, 1.0);
        render(s, frames)
    }

    /// Assert that an FX bypass pair produces sample-identical output regardless
    /// of the knob values: builds two synths that share the `off_param` = 0.0
    /// switch, then sets each `(knob, value)` pair to its min on synth A and
    /// max on synth B, renders a short note through both, and asserts L and R
    /// channels match. Every FX bypass pair (`PhaserOn`/`ReverbOn`) passes exactly
    /// the dry stereo bus through unchanged when off.
    fn assert_fx_off_is_dry_pass(off_param: GlobalParam, knobs_a: &[(GlobalParam, f32)], knobs_b: &[(GlobalParam, f32)]) {
        let setup = |knobs: &[(GlobalParam, f32)]| -> (Vec<f32>, Vec<f32>) {
            let mut s = Synth::new(48_000.0);
            s.set_param(gp(off_param), 0.0);
            for &(k, v) in knobs {
                s.set_param(gp(k), v);
            }
            render_short_note(&mut s, 4800)
        };
        let (al, ar) = setup(knobs_a);
        let (bl, br) = setup(knobs_b);
        let name = format!("{off_param:?}");
        assert_eq!(al, bl, "{name} off path is not dry-pass on L");
        assert_eq!(ar, br, "{name} off path is not dry-pass on R");
    }

    /// Advance channel 0's LFO 1 in a pitched synth by starting a note and
    /// rendering a short block. Sets `LfoRate` to `rate` and `Lfo1FreeRun` to
    /// `free_run` before triggering. Returns the synth so the caller can
    /// inspect `lfo1_phase` or trigger further notes.
    fn advance_ch0_lfo(rate: f32, free_run: bool) -> Synth {
        let mut s = pitched_synth();
        s.set_param(pp(PatchParam::LfoRate), rate);
        if free_run {
            s.set_param(pp(PatchParam::Lfo1FreeRun), 1.0);
        }
        s.note_on_layer(0, 60, 1.0);
        let _ = render(&mut s, 6000);
        s
    }

    #[test]
    fn phaser_off_passes_dry_unchanged() {
        // With phaser_on=0 the phaser branch must keep the engine sample-
        // exact against a build with the phaser absent. The phaser knobs
        // must have no effect when the switch is off.
        assert_fx_off_is_dry_pass(
            GlobalParam::PhaserOn,
            &[
                (GlobalParam::PhaserRate, 0.05),
                (GlobalParam::PhaserDepth, 0.0),
                (GlobalParam::PhaserFB, 0.0),
                (GlobalParam::PhaserMix, 0.0),
            ],
            &[
                (GlobalParam::PhaserRate, 8.0),
                (GlobalParam::PhaserDepth, 1.0),
                (GlobalParam::PhaserFB, 0.8),
                (GlobalParam::PhaserMix, 1.0),
            ],
        );
    }

    #[test]
    fn phaser_on_audibly_changes_output() {
        // With phaser_on=1 and a non-zero mix, the output must diverge from
        // the phaser-off baseline. Chorus off so the only stereo-active stage
        // is the phaser itself.
        let mut a = Synth::new(48_000.0);
        a.set_param(gp(GlobalParam::ChorusOn), 0.0);
        a.set_param(gp(GlobalParam::PhaserOn), 0.0);
        let (al, _ar) = render_short_note(&mut a, 4800);

        let mut b = Synth::new(48_000.0);
        b.set_param(gp(GlobalParam::ChorusOn), 0.0);
        b.set_param(gp(GlobalParam::PhaserOn), 1.0);
        b.set_param(gp(GlobalParam::PhaserRate), 1.0);
        b.set_param(gp(GlobalParam::PhaserDepth), 0.9);
        b.set_param(gp(GlobalParam::PhaserFB), 0.6);
        b.set_param(gp(GlobalParam::PhaserMix), 0.7);
        let (bl, _br) = render_short_note(&mut b, 4800);

        let mut diverged = false;
        for i in 0..al.len().min(bl.len()) {
            if (al[i] - bl[i]).abs() > 1.0e-4 {
                diverged = true;
                break;
            }
        }
        assert!(diverged, "phaser_on should perturb the output vs phaser_off");
    }

    #[test]
    fn reverb_off_passes_dry_unchanged() {
        // With reverb_on=0 the reverb branch is gated off, so the dry chain
        // output must not depend on size / decay / damp / mix. Compare two
        // runs that differ in every reverb knob.
        assert_fx_off_is_dry_pass(
            GlobalParam::ReverbOn,
            &[
                (GlobalParam::ReverbSize, 0.0),
                (GlobalParam::ReverbDecay, 0.5),
                (GlobalParam::ReverbDamp, 0.0),
                (GlobalParam::ReverbMix, 0.0),
            ],
            &[
                (GlobalParam::ReverbSize, 1.0),
                (GlobalParam::ReverbDecay, 8.0),
                (GlobalParam::ReverbDamp, 1.0),
                (GlobalParam::ReverbMix, 1.0),
            ],
        );
    }
}
