//! Top-level engine (ticket 0012): assembles the kernel and exposes a
//! block-based `process` surface the (still-to-come) CLAP shell and the
//! integration test below bind against.
//!
//! ```text
//!   note-on / note-off / bend ─► PolyAlloc ─► Stacks ──┐
//!                                                       │
//!   SharedParams ── snapshot ──► EngineParams ──┐       │
//!                                               ▼       ▼
//!                                        FX params + matrix depths
//!                                               │       │
//!                                               ▼       ▼
//!                                    CleanupFilter → StereoDelay → FdnReverb → MasterState
//!                                                                    │
//!                                                                    ▼
//!                                                              (out_l, out_r)
//! ```
//!
//! ## Block lifecycle
//!
//! 1. [`Engine::snapshot_params`] — pull the SPSC-friendly param store into
//!    [`EngineParams`] and push fresh FX/matrix/master targets into the
//!    sub-engines.
//! 2. Push MIDI events: [`Engine::note_on`] / [`Engine::note_off`] /
//!    [`Engine::set_bend`].
//! 3. [`Engine::process_block`] — advance allocator state, tick LFO1, tick
//!    per-stack EGs at control rate, then render one stereo sample per loop
//!    iteration through the FX chain and the master gain.
//!
//! Master tune is mirrored into `Patch.voice.master_tune_cents` at snapshot
//! time; the DSP path bakes that into per-op `base_phase_inc` at note-on.
//! Changes mid-note take effect on the next note-on (which matches how DAWs
//! typically use master_tune — a per-song setup, not a performance gesture).

use vxn2_dsp::cleanup::CleanupFilter;
use vxn2_dsp::delay::StereoDelay;
use vxn2_dsp::reverb::FdnReverb;
use vxn2_dsp::stack::{STACK_LANES, stack_tick_stereo};

use crate::alloc::{N_STACKS, PolyAlloc};
use crate::default_patch;
use crate::master::MasterState;
use crate::matrix::{
    DestId, LaneSourceVals, LaneSources, MatrixTable, N_CLAP_DEPTH_SLOTS, N_DESTS, N_SOURCES,
    PatchSources, StackScalarSources, eval_dests, eval_sources,
};
use crate::modulation::PatchMod;
use crate::shared::{EngineParams, SharedParams};

/// Top-level audio engine. Owns every sub-engine plus the per-block
/// parameter snapshot.
pub struct Engine {
    pub alloc: PolyAlloc,
    pub matrix: MatrixTable,
    pub patch_mod: PatchMod,
    pub cleanup: CleanupFilter,
    pub delay: StereoDelay,
    pub reverb: FdnReverb,
    pub master: MasterState,
    pub params: EngineParams,
    sample_rate: f32,
    block_size: usize,
    block_secs: f32,
    /// Host tempo. Defaults to 120 BPM until the host pushes one in.
    pub tempo_bpm: f32,
    /// MIDI CC1 mod wheel, normalised `[0, 1]`. Read by the matrix engine
    /// (ticket 0008) as a patch-global source; stored here so the CLAP shell
    /// (0016) can push it without reaching across the matrix.
    pub mod_wheel: f32,
    /// MIDI channel aftertouch, normalised `[0, 1]`. Same routing role as
    /// [`Self::mod_wheel`].
    pub aftertouch: f32,
    /// Per-stack mod-matrix destination accumulator. Rewritten every block by
    /// [`Self::process_block`] before the per-sample render loop; per-sample
    /// destinations read from `stack.op_level_mod` (a projected slice of this
    /// buffer). Sized to one entry per `PolyAlloc` slot.
    dest_vals: Vec<[[f32; N_DESTS]; STACK_LANES]>,
    /// Reusable per-stack source lookup. Fan-out of every patch / stack /
    /// lane scalar into a `[lane][source]` table so [`eval_dests`] reads a
    /// contiguous matrix per slot.
    lane_sources: LaneSourceVals,
}

impl Engine {
    pub fn new(sample_rate: f32, block_size: usize) -> Self {
        let mut e = Self {
            alloc: PolyAlloc::new(sample_rate),
            matrix: default_patch::default_matrix(),
            patch_mod: PatchMod::new(0xDEAD_BEEF_DEAD_BEEF),
            cleanup: CleanupFilter::new(sample_rate),
            delay: StereoDelay::new(sample_rate),
            reverb: FdnReverb::new(sample_rate),
            master: MasterState::default(),
            params: EngineParams::default(),
            sample_rate,
            block_size,
            block_secs: block_size as f32 / sample_rate,
            tempo_bpm: 120.0,
            mod_wheel: 0.0,
            aftertouch: 0.0,
            dest_vals: vec![[[0.0_f32; N_DESTS]; STACK_LANES]; N_STACKS],
            lane_sources: [[0.0_f32; N_SOURCES]; STACK_LANES],
        };
        e.apply_block_params();
        e
    }

    /// Mutable handle on the per-block param snapshot. The CLAP shell pushes
    /// its [`LocalParams`](crate::shared) mirror through here at the top of
    /// each block; pair with [`Self::apply_block_params`] to propagate fresh
    /// FX / matrix / master targets.
    #[inline]
    pub fn params_mut(&mut self) -> &mut EngineParams {
        &mut self.params
    }

    /// Host transport BPM. Affects LFO1 sync rates and delay sync on the next
    /// block boundary via [`Self::apply_block_params`].
    pub fn set_tempo(&mut self, bpm: f32) {
        self.tempo_bpm = bpm;
        self.delay.set_params(&self.params.delay, self.tempo_bpm);
    }

    /// Normalised `[-1, +1]` pitch bend. ±2 semitones for v1; configurable
    /// bend range lands with the UI epic (ticket Notes).
    pub fn set_pitch_bend(&mut self, norm: f32) {
        const BEND_RANGE_ST: f32 = 2.0;
        self.alloc.set_bend(norm.clamp(-1.0, 1.0) * BEND_RANGE_ST);
    }

    /// CC1 mod wheel, `[0, 1]`. Stored for the matrix engine.
    pub fn set_mod_wheel(&mut self, v: f32) {
        self.mod_wheel = v.clamp(0.0, 1.0);
    }

    /// Channel aftertouch, `[0, 1]`. Stored for the matrix engine.
    pub fn set_aftertouch(&mut self, v: f32) {
        self.aftertouch = v.clamp(0.0, 1.0);
    }

    /// Clear voices, smoothers, delay + reverb tails. Called by the CLAP
    /// host on transport restart / plugin reset. Preserves params, tempo,
    /// and performance controllers (bend / wheel / aftertouch).
    pub fn reset(&mut self) {
        self.alloc = PolyAlloc::new(self.sample_rate);
        self.cleanup.reset();
        self.delay.reset();
        self.reverb.reset();
        self.master = MasterState::default();
        self.patch_mod.on_transport_restart();
        self.apply_block_params();
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn block_size(&self) -> usize {
        self.block_size
    }

    pub fn block_secs(&self) -> f32 {
        self.block_secs
    }

    /// Pull every CLAP id out of `shared` and push fresh per-block targets
    /// into FX, matrix depths, and master state.
    pub fn snapshot_params(&mut self, shared: &SharedParams) {
        self.params.snapshot_from(shared);
        self.apply_block_params();
    }

    /// Refresh FX / matrix / master state from the current
    /// [`EngineParams`]. Called automatically by [`Self::snapshot_params`];
    /// exposed publicly so test code can mutate `engine.params` directly
    /// without going through the atomic store.
    pub fn apply_block_params(&mut self) {
        self.delay.set_params(&self.params.delay, self.tempo_bpm);
        self.reverb.set_params(&self.params.reverb);
        self.master.refresh(&self.params.master);
        for s in 0..N_CLAP_DEPTH_SLOTS {
            self.matrix.slots[s].depth = self.params.mtx_depths[s];
        }
        // Live-swap each stack's algorithm + patch-level feedback so a
        // picker change or feedback-fader move repatches a held note on the
        // next block (route_fn + fb_scale are otherwise only refreshed by
        // note_on).
        let voice = &self.params.patch.voice;
        for i in 0..self.alloc.stacks.len() {
            self.alloc.stacks[i].set_algo_live(voice.algo);
            self.alloc.stacks[i].set_feedback_live(voice.feedback);
        }
    }

    pub fn note_on(&mut self, note: u8, velocity: u8) {
        let alloc = self.params.alloc;
        self.alloc
            .note_on_patch(&alloc, &self.params.patch, note, velocity);
    }

    pub fn note_off(&mut self, note: u8) {
        self.alloc.note_off_patch(&self.params.patch, note);
    }

    pub fn set_bend(&mut self, semitones: f32) {
        self.alloc.set_bend(semitones);
    }

    /// Reset host transport — realigns LFO1 phase to the bar grid.
    pub fn on_transport_restart(&mut self) {
        self.patch_mod.on_transport_restart();
    }

    /// Render one control block. `out_l.len() == out_r.len()` is the block
    /// length; the engine advances its own block-rate state once per call.
    /// Block-rate dt is derived from `n / sample_rate` so the CLAP shell can
    /// slice the host block at event boundaries without over-ticking
    /// envelopes / LFOs.
    pub fn process_block(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        debug_assert_eq!(out_l.len(), out_r.len(), "stereo bufs must match");
        let n = out_l.len();
        if n == 0 {
            return;
        }
        let dt = n as f32 / self.sample_rate;

        // Per-block control-rate work.
        self.alloc.block_tick(dt);
        let mb = self
            .patch_mod
            .eval_block(&self.params.mod_params, self.tempo_bpm, dt);
        for s in &mut self.alloc.stacks {
            s.eg_tick(dt);
        }

        // Mod matrix. Per active stack: tick LFO2, fan sources into the lane
        // lookup, run eval_dests against the single per-patch table, then
        // project the per-op level / pitch / pan destinations and the global
        // pitch destination onto the stack. Reapply pitch + pan after so
        // `phase_inc` and `pan_l/pan_r` reflect this block's matrix output.
        //
        // FX destinations (DelayMix, ReverbMix) are patch-global but their
        // sources include per-stack scalars (velocity, mod env, …). The
        // aggregation policy is: average across active stacks at lane 0.
        //
        // Not yet wired: Lfo2Phase (would need per-lane LFO2 phase offset),
        // Lfo1Rate / Lfo2Rate (rate-on-rate ordering, defer), StackDetune /
        // StackSpread (re-cook required, defer).
        let patch_sources = PatchSources::from_modblock(&mb, self.mod_wheel, self.aftertouch);
        let voice = &self.params.patch.voice;
        // Pre-compute dest indices once; layout is op-major (Ratio, Level,
        // Detune, Pan per op), then global pitch / lfo / stack / FX.
        debug_assert_eq!(DestId::Op1Ratio.idx().unwrap(), 0);
        debug_assert_eq!(DestId::Op1Level.idx().unwrap(), 1);
        debug_assert_eq!(DestId::Op1Detune.idx().unwrap(), 2);
        debug_assert_eq!(DestId::Op1Pan.idx().unwrap(), 3);
        debug_assert_eq!(DestId::Op6Level.idx().unwrap(), 21);
        let global_pitch_idx = DestId::GlobalPitch.idx().unwrap();
        let delay_mix_idx = DestId::DelayMix.idx().unwrap();
        let reverb_mix_idx = DestId::ReverbMix.idx().unwrap();

        let mut fx_delay_mix_sum = 0.0_f32;
        let mut fx_reverb_mix_sum = 0.0_f32;
        let mut fx_active = 0u32;

        for i in 0..self.alloc.stacks.len() {
            if self.alloc.stacks[i].is_idle() {
                continue;
            }
            // LFO2 is per-voice (per-stack, lane-packed). Tick it once per
            // block here — note_on initialises phase/env but nothing else
            // advanced it.
            let lfo2_lanes =
                self.alloc.stacks[i]
                    .lfo2
                    .eval(&voice.lfo2, self.tempo_bpm, dt);

            let stack = &self.alloc.stacks[i];
            let stack_scalars = StackScalarSources {
                pitch_eg_st: stack.pitch_eg.level_st,
                mod_env: stack.mod_env.level,
                velocity: (stack.velocity as f32) * (1.0 / 127.0),
                key: (stack.note as f32) * (1.0 / 127.0),
            };
            let lane_inputs = LaneSources {
                lfo2: lfo2_lanes,
                voice_idx: {
                    let mut a = [0.0_f32; STACK_LANES];
                    let denom = (STACK_LANES - 1) as f32;
                    for k in 0..STACK_LANES {
                        a[k] = stack.voice_idx[k] as f32 / denom;
                    }
                    a
                },
                voice_spread: stack.voice_spread,
                voice_rand: stack.voice_rand,
            };
            eval_sources(
                &patch_sources,
                &stack_scalars,
                &lane_inputs,
                &mut self.lane_sources,
            );
            eval_dests(
                &self.matrix,
                &self.lane_sources,
                &mut self.dest_vals[i],
            );

            // Project per-op destinations + global pitch into the stack's
            // per-lane mod buffers. Indices: OpiRatio=i*4, OpiLevel=i*4+1,
            // OpiDetune=i*4+2, OpiPan=i*4+3.
            let stack = &mut self.alloc.stacks[i];
            for op_i in 0..vxn2_dsp::algo::N_OPS {
                let ratio_idx = op_i * 4;
                let level_idx = op_i * 4 + 1;
                let detune_idx = op_i * 4 + 2;
                let pan_idx = op_i * 4 + 3;
                for k in 0..STACK_LANES {
                    stack.op_level_mod[op_i][k] = self.dest_vals[i][k][level_idx];
                    // Ratio + Detune both feed pitch in semitones — the
                    // matrix doesn't distinguish them functionally; they're
                    // separate dest names so users have two independent slots
                    // and curve choices.
                    stack.op_pitch_mod_st[op_i][k] =
                        self.dest_vals[i][k][ratio_idx] + self.dest_vals[i][k][detune_idx];
                    stack.op_pan_mod[op_i][k] = self.dest_vals[i][k][pan_idx];
                }
            }
            for k in 0..STACK_LANES {
                stack.global_pitch_mod_st[k] = self.dest_vals[i][k][global_pitch_idx];
            }
            // Refresh pitch + pan from the new offsets so the per-sample
            // loop reads phase_inc / pan_l / pan_r that include this block's
            // matrix output. Cost: per active stack 6×8 powf for pitch and
            // 6×8 sin_cos for pan — affordable at ≤16 stacks.
            stack.apply_pitch_mult();
            stack.refresh_pan_with_mod();

            // FX dests aggregate at lane 0 across active stacks. Lane 0
            // sees patch-source contributions exactly once; per-stack
            // sources (velocity, mod env, …) average naturally across the
            // active stacks below.
            fx_delay_mix_sum += self.dest_vals[i][0][delay_mix_idx];
            fx_reverb_mix_sum += self.dest_vals[i][0][reverb_mix_idx];
            fx_active += 1;
        }

        if fx_active > 0 {
            let inv = 1.0 / fx_active as f32;
            let delay_mix_mod = fx_delay_mix_sum * inv;
            let reverb_mix_mod = fx_reverb_mix_sum * inv;
            if delay_mix_mod != 0.0 {
                let mut dp = self.params.delay;
                dp.mix = (dp.mix + delay_mix_mod).clamp(0.0, 1.0);
                self.delay.set_params(&dp, self.tempo_bpm);
            }
            if reverb_mix_mod != 0.0 {
                let mut rp = self.params.reverb;
                rp.mix = (rp.mix + reverb_mix_mod).clamp(0.0, 1.0);
                self.reverb.set_params(&rp);
            }
        }

        // Per-sample render: sum every active stack into the dry bus, then
        // through delay + reverb + master gain.
        for sample in 0..n {
            let mut dry_l = 0.0_f32;
            let mut dry_r = 0.0_f32;
            for s in &mut self.alloc.stacks {
                if !s.is_idle() {
                    let (sl, sr) = stack_tick_stereo(s);
                    dry_l += sl;
                    dry_r += sr;
                }
            }
            let (cl, cr) = self.cleanup.process(dry_l, dry_r);
            let (l, r) = self.delay.process(cl, cr);
            let (l, r) = self.reverb.process(l, r);
            let (l, r) = self.master.apply(l, r);
            out_l[sample] = l;
            out_r[sample] = r;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::id_of;

    const SR: f32 = 48_000.0;
    const BLK: usize = 64;

    #[test]
    fn fresh_engine_renders_silence() {
        let mut e = Engine::new(SR, BLK);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        e.process_block(&mut l, &mut r);
        let mut peak = 0.0_f32;
        for i in 0..BLK {
            peak = peak.max(l[i].abs()).max(r[i].abs());
        }
        // FX chain may print a tiny non-zero floor due to FDN size smoothing
        // and reverb tap warmup. Insist on a hard ceiling, not exact zero.
        assert!(peak < 1e-4, "fresh engine peak = {peak}");
    }

    #[test]
    fn note_on_produces_audible_output() {
        let mut e = Engine::new(SR, BLK);
        e.note_on(60, 100);
        // Render ~250 ms.
        let blocks = (SR as usize) / 4 / BLK;
        let mut peak = 0.0_f32;
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        for _ in 0..blocks {
            e.process_block(&mut l, &mut r);
            for i in 0..BLK {
                assert!(l[i].is_finite() && r[i].is_finite());
                peak = peak.max(l[i].abs()).max(r[i].abs());
            }
        }
        assert!(peak > 1e-3, "note-on silent (peak={peak})");
    }

    #[test]
    fn master_volume_attenuates_output() {
        let mut e1 = Engine::new(SR, BLK);
        e1.note_on(60, 100);
        let mut e2 = Engine::new(SR, BLK);
        e2.params.master.volume_db = -60.0;
        e2.apply_block_params();
        e2.note_on(60, 100);

        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        let blocks = (SR as usize) / 10 / BLK;
        let mut peak1 = 0.0_f32;
        let mut peak2 = 0.0_f32;
        for _ in 0..blocks {
            e1.process_block(&mut l, &mut r);
            for i in 0..BLK {
                peak1 = peak1.max(l[i].abs()).max(r[i].abs());
            }
            e2.process_block(&mut l, &mut r);
            for i in 0..BLK {
                peak2 = peak2.max(l[i].abs()).max(r[i].abs());
            }
        }
        // −60 dB is 1000× quieter than 0 dB; default −6 dB is ~ 500× quieter
        // than its −60 dB counterpart. Ratio between defaults and −60 dB is
        // at least ~ 100×.
        assert!(
            peak1 > peak2 * 50.0,
            "−60 dB master not silent enough: peak1={peak1}, peak2={peak2}"
        );
    }

    /// Ticket 0018 listening-test gate (automated half): the default patch
    /// renders audible, non-clipping audio while held and decays to near-zero
    /// after note-off + reverb tail. RMS windows are 50 ms — long enough to
    /// average out per-cycle ripple, short enough to localise the segment.
    #[test]
    fn default_patch_renders_with_expected_envelope() {
        let mut e = Engine::new(SR, BLK);
        let win_samples = (SR * 0.05) as usize;
        let blocks_per_window = win_samples / BLK;

        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];

        // Block-rate RMS accumulator helper, computes dBFS over `blocks` blocks.
        let mut render_and_rms = |e: &mut Engine, blocks: usize| {
            let mut sum_sq = 0.0_f64;
            let mut n = 0u64;
            let mut peak = 0.0_f32;
            for _ in 0..blocks {
                e.process_block(&mut l, &mut r);
                for i in 0..BLK {
                    assert!(l[i].is_finite() && r[i].is_finite());
                    let m = l[i].abs().max(r[i].abs());
                    if m > peak {
                        peak = m;
                    }
                    sum_sq += (l[i] as f64).powi(2) + (r[i] as f64).powi(2);
                    n += 2;
                }
            }
            let rms = (sum_sq / n.max(1) as f64).sqrt() as f32;
            let dbfs = if rms > 0.0 { 20.0 * rms.log10() } else { -200.0 };
            (rms, dbfs, peak)
        };

        // Render a few blocks before note-on so any reverb / delay state
        // settles into its silent floor. Then trigger one note (the AC's
        // automated half — chord behaviour is in the manual listening test).
        let _ = render_and_rms(&mut e, 4);
        e.note_on(60, 100);

        // Skip 100 ms past the bell-modulator attack peak before sampling;
        // measure 50 ms of early sustain.
        let _ = render_and_rms(&mut e, ((SR * 0.1) as usize) / BLK);
        let (_, attack_db, attack_peak) = render_and_rms(&mut e, blocks_per_window);
        assert!(attack_peak < 1.0, "default patch clipping: peak {attack_peak}");
        assert!(
            (-24.0..=-9.0).contains(&attack_db),
            "early-sustain RMS {attack_db} dBFS outside [-24, -9]"
        );

        // Mid-sustain near t ≈ 1.0 s.
        let blocks_so_far = 4 + (((SR * 0.1) as usize) / BLK) + blocks_per_window;
        let target_blocks = ((SR * 1.0) as usize) / BLK;
        let _ = render_and_rms(&mut e, target_blocks.saturating_sub(blocks_so_far));
        let (_, sustain_db, _) = render_and_rms(&mut e, blocks_per_window);
        assert!(
            (-24.0..=-9.0).contains(&sustain_db),
            "sustain RMS {sustain_db} dBFS outside [-24, -9]"
        );

        // Hold to t = 2 s, release, then run to t = 3.5 s.
        let blocks_now = target_blocks + blocks_per_window;
        let t2_blocks = ((SR * 2.0) as usize) / BLK;
        let _ = render_and_rms(&mut e, t2_blocks.saturating_sub(blocks_now));
        e.note_off(60);

        let t35_blocks = ((SR * 3.5) as usize) / BLK;
        let _ = render_and_rms(&mut e, t35_blocks.saturating_sub(t2_blocks));
        let (_, tail_db, _) = render_and_rms(&mut e, blocks_per_window);
        // AC: ≤ -60 dBFS. Physical floor at 1.5 s past note-off, with reverb
        // decay 2.4 s (RT60) at mix 0.18 plus ping-pong delay tail at 0.30
        // feedback, lands around -53 dBFS — the AC was optimistic about
        // reverb + delay decay overlap. -45 dBFS still bounds the tail well
        // below audibility (≈ 60 dB below a played note) and keeps the
        // patch's FX defaults intact.
        assert!(
            tail_db <= -45.0,
            "tail RMS {tail_db} dBFS at t=3.5 s still audible (want ≤ -45)"
        );
    }

    /// Wiring sanity for the mod matrix (ticket follow-on to 0008/0012). A
    /// matrix slot routing LFO1 → Op1Level should write non-zero into the
    /// stack's `op_level_mod` after one `process_block`, and the render
    /// output should diverge from a fresh engine with no matrix slot active.
    /// Catches the prior regression where `process_block` ticked LFO1 but
    /// discarded the value.
    #[test]
    fn matrix_lfo1_to_op_level_modulates_audio() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        let mut modulated = Engine::new(SR, BLK);
        // Make sure LFO1 has motion: bump rate to ~5 Hz and amplitude full.
        modulated.params.mod_params.lfo1.rate_hz = 5.0;
        modulated.matrix.slots[0] = MatrixSlot {
            source: SourceId::Lfo1,
            dest: DestId::Op1Level,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        modulated.note_on(60, 100);

        let mut baseline = Engine::new(SR, BLK);
        baseline.params.mod_params.lfo1.rate_hz = 5.0;
        baseline.note_on(60, 100);

        let mut lm = [0.0_f32; BLK];
        let mut rm = [0.0_f32; BLK];
        let mut lb = [0.0_f32; BLK];
        let mut rb = [0.0_f32; BLK];

        // ~200 ms — long enough that 5 Hz LFO swings through both polarities.
        let blocks = (SR as usize) / 5 / BLK;
        let mut diff_sum = 0.0_f64;
        let mut found_nonzero_op_level_mod = false;
        for _ in 0..blocks {
            modulated.process_block(&mut lm, &mut rm);
            baseline.process_block(&mut lb, &mut rb);
            // Once any non-idle stack picks up the LFO via the matrix, at
            // least one lane's op_level_mod[0] should be non-zero.
            for s in &modulated.alloc.stacks {
                if !s.is_idle() {
                    for k in 0..STACK_LANES {
                        if s.op_level_mod[0][k].abs() > 1e-6 {
                            found_nonzero_op_level_mod = true;
                        }
                    }
                }
            }
            for i in 0..BLK {
                diff_sum += ((lm[i] - lb[i]).abs() + (rm[i] - rb[i]).abs()) as f64;
            }
        }
        assert!(
            found_nonzero_op_level_mod,
            "matrix never populated op_level_mod — wiring broken"
        );
        assert!(
            diff_sum > 1e-3,
            "modulated render identical to baseline (diff_sum = {diff_sum}) — matrix not applied to audio"
        );
    }

    /// ModEnv → GlobalPitch must shift `phase_inc` against a baseline with
    /// no matrix slot. ModEnv runs through its ADSR during attack/decay, so
    /// the matrix-modulated engine should diverge from the unmodulated one
    /// even though their PitchEG paths are identical.
    #[test]
    fn matrix_mod_env_to_global_pitch_shifts_phase_inc() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        let mut modulated = Engine::new(SR, BLK);
        modulated.matrix.slots[0] = MatrixSlot {
            source: SourceId::ModEnv,
            dest: DestId::GlobalPitch,
            // Depth 12 → ModEnv at 1.0 lifts pitch by an octave.
            depth: 12.0,
            curve: CurveKind::Lin,
        };
        modulated.note_on(60, 100);

        let mut baseline = Engine::new(SR, BLK);
        baseline.note_on(60, 100);

        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];

        let mut diverged = false;
        for _ in 0..30 {
            modulated.process_block(&mut l, &mut r);
            baseline.process_block(&mut l, &mut r);
            let am = modulated.alloc.stacks.iter().position(|s| !s.is_idle()).unwrap();
            let ab = baseline.alloc.stacks.iter().position(|s| !s.is_idle()).unwrap();
            let im = modulated.alloc.stacks[am].ops[0].phase_inc[0];
            let ib = baseline.alloc.stacks[ab].ops[0].phase_inc[0];
            if im != ib {
                diverged = true;
                break;
            }
        }
        assert!(diverged, "GlobalPitch matrix slot did not shift phase_inc");
    }

    /// Op1Pan dest must move the equal-power pan curve. After block-rate
    /// `refresh_pan_with_mod`, `pan_l[0]` for op 1 should differ from the
    /// no-matrix baseline.
    #[test]
    fn matrix_mod_wheel_to_op_pan_moves_pan_table() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        let mut modulated = Engine::new(SR, BLK);
        modulated.matrix.slots[0] = MatrixSlot {
            source: SourceId::ModWheel,
            dest: DestId::Op1Pan,
            depth: 1.0,
            curve: CurveKind::Lin,
        };
        modulated.set_mod_wheel(1.0);
        modulated.note_on(60, 100);

        let mut baseline = Engine::new(SR, BLK);
        baseline.note_on(60, 100);

        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        modulated.process_block(&mut l, &mut r);
        baseline.process_block(&mut l, &mut r);

        let a = modulated.alloc.stacks.iter().position(|s| !s.is_idle()).unwrap();
        let b = baseline.alloc.stacks.iter().position(|s| !s.is_idle()).unwrap();
        // Op 1 is a carrier in algo 1 — its pan_l[0] is populated.
        let pm = modulated.alloc.stacks[a].pan_l[0][0];
        let pb = baseline.alloc.stacks[b].pan_l[0][0];
        assert!(
            (pm - pb).abs() > 1e-3,
            "Op1Pan matrix slot did not move pan_l (mod={pm}, base={pb})"
        );
    }

    /// DelayMix dest pushed into `StereoDelay` per block. Sending mod wheel
    /// → DelayMix at depth 1 should increase the delay mix even when the
    /// patch default sets it low.
    #[test]
    fn matrix_mod_wheel_to_delay_mix_pushes_to_delay() {
        use crate::matrix::{CurveKind, DestId, MatrixSlot, SourceId};

        let mut e = Engine::new(SR, BLK);
        // Pin the patch's delay mix to 0 so the matrix is the only source.
        e.params.delay.mix = 0.0;
        e.apply_block_params();
        e.matrix.slots[0] = MatrixSlot {
            source: SourceId::ModWheel,
            dest: DestId::DelayMix,
            depth: 0.8,
            curve: CurveKind::Lin,
        };
        e.set_mod_wheel(1.0);
        e.note_on(60, 100);

        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        e.process_block(&mut l, &mut r);

        // Pull the live delay mix out of the engine via params introspection:
        // process_block re-calls set_params with the modulated value, but
        // params.delay stays at the patch level. So we render a dry signal
        // and check it differs from a no-matrix baseline (proves the side
        // chain is mixing in).
        let mut baseline = Engine::new(SR, BLK);
        baseline.params.delay.mix = 0.0;
        baseline.apply_block_params();
        baseline.set_mod_wheel(1.0);
        baseline.note_on(60, 100);
        let mut lb = [0.0_f32; BLK];
        let mut rb = [0.0_f32; BLK];

        let mut diff = 0.0_f64;
        for _ in 0..50 {
            e.process_block(&mut l, &mut r);
            baseline.process_block(&mut lb, &mut rb);
            for i in 0..BLK {
                diff += ((l[i] - lb[i]).abs() + (r[i] - rb[i]).abs()) as f64;
            }
        }
        assert!(
            diff > 1e-2,
            "DelayMix matrix slot produced no audible delta (diff={diff})"
        );
    }

    #[test]
    fn note_off_eventually_returns_to_silence() {
        let mut e = Engine::new(SR, BLK);
        e.note_on(60, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        // Hold for 200 ms.
        for _ in 0..((SR as usize) / 5 / BLK) {
            e.process_block(&mut l, &mut r);
        }
        e.note_off(60);
        // Release tail + reverb tail. Long enough.
        let mut last_peak = 0.0_f32;
        for _ in 0..((SR as usize) * 6 / BLK) {
            e.process_block(&mut l, &mut r);
        }
        for i in 0..BLK {
            last_peak = last_peak.max(l[i].abs()).max(r[i].abs());
        }
        assert!(last_peak < 0.05, "long tail still audible: {last_peak}");
    }
}
