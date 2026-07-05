//! The vxn-3 instrument engine: 8 heterogeneous tracks summed to stereo.
//!
//! Per block it (1) installs any pending off-thread engine swaps, (2) maps the
//! host beat clock onto each track's step grid and schedules trigs
//! **sample-accurately** by slicing the block at trig boundaries, (3) renders
//! each track's active engine into mono scratch, and (4) mixes to stereo with
//! per-track gain/pan. Allocation-free throughout.
//!
//! Naming: the per-track voicing abstraction is [`crate::track_engine::TrackEngine`];
//! this `Engine` is the whole instrument (one per plugin instance).

use std::sync::Arc;

use crate::io::{EngineCommand, EngineIo, PlayheadState};
use crate::lane::{Hit, LaneState};
use crate::sequencer::{LockParam, Pattern};
use crate::swap::EngineSwap;
use crate::track::Track;
use crate::transport::Transport;

/// Map a generic macro slot to its lockable-param lane (slots 0/1/2 →
/// decay/tone/pitch — the three engine-reinterpreted lanes; ADR 0003 §2). Slots
/// outside the budget have no lane.
fn macro_to_lock_param(slot: u8) -> Option<LockParam> {
    match slot {
        0 => Some(LockParam::Decay),
        1 => Some(LockParam::Tone),
        2 => Some(LockParam::Pitch),
        _ => None,
    }
}

/// Fixed track count (ADR 0001 — eight tracks for the minimal-techno kit).
pub const N_TRACKS: usize = 8;

/// Per-track per-block hit-buffer capacity. Generous vs. the realistic worst
/// case (a fast lane + a dense retrig in one block); the scheduler drops beyond
/// this rather than allocate.
const HIT_CAPACITY: usize = 256;

/// Max live MIDI free-play notes buffered per block (0186) before dropping —
/// generous vs. a human hammering pads; keeps the per-block merge allocation-free.
const FREE_NOTE_CAPACITY: usize = 64;

/// A live MIDI note queued for the next block: which `track` to trig, at what `frame`,
/// with what `note` (fractional MIDI, for pitch) + `velocity` (0..1).
struct FreeNote {
    track: u8,
    frame: u32,
    note: f32,
    velocity: f32,
}

/// Master limiter look-ahead in samples — the plugin's reported latency (PDC).
/// Constant, so the CLAP shell reports it once.
pub const LIMITER_LOOKAHEAD: u32 = 64;
/// Master limiter ceiling (linear).
const LIMITER_CEILING: f32 = 0.95;
/// Max delay time the send bus pre-allocates for.
const DELAY_MAX_SECONDS: f32 = 2.0;

pub struct Engine {
    sample_rate: f32,
    transport: Transport,
    tracks: Vec<Track>,
    /// Per-track sequencer state (polymeter phase, probability RNG, in-flight
    /// retrig). Parallel to `tracks`.
    lanes: Vec<LaneState>,
    /// Reused per-track scratch for one block's scheduled hits — pre-allocated,
    /// cleared (not freed) each use, so scheduling never allocates.
    hits: Vec<Hit>,
    /// Shared main↔audio I/O: edit-command queue, playhead, engine-swap mailboxes.
    io: EngineIo,
    /// Scratch for publishing per-lane playhead positions (avoids per-block alloc).
    playhead_scratch: [u32; N_TRACKS],
    /// Beat position used when the host exposes no beats timeline — advanced by
    /// the block length each playing block so the sequencer free-runs.
    free_run_beats: f64,
    /// Live MIDI free-play notes to trig this block (0186), merged into each track's
    /// scheduled hits. Pre-allocated + cleared (not freed) each block.
    free_notes: Vec<FreeNote>,

    // ── master FX (0051) ──
    /// Stereo delay send bus (the dub throw).
    delay: vxn3_dsp::Delay,
    /// Terminal master limiter.
    limiter: vxn3_dsp::Limiter,
    /// Delay return level into the master mix.
    return_level: f32,
    /// Delay time as a tempo-synced subdivision in beats.
    delay_sync_beats: f64,
    /// Master output volume (linear), applied to the mix before the limiter.
    master_volume: f32,
    /// Cached delay feedback (the `Delay` doesn't expose a getter) — mirrored so
    /// the host-param echo (0173) can report its effective value.
    delay_feedback: f32,
    /// Pre-allocated send / wet scratch (avoids per-block alloc).
    send_l: Vec<f32>,
    send_r: Vec<f32>,
    wet_l: Vec<f32>,
    wet_r: Vec<f32>,
}

impl Engine {
    /// Build an engine with its own private I/O (tests / standalone use).
    pub fn new(sample_rate: f32, block_size: usize) -> Self {
        Self::with_io(sample_rate, block_size, EngineIo::new())
    }

    /// Build an engine sharing the given I/O handles with the main thread (the
    /// CLAP shell path): the edit queue, playhead, and per-track swap mailboxes
    /// are the same `Arc`s the UI drives.
    pub fn with_io(sample_rate: f32, block_size: usize, io: EngineIo) -> Self {
        let max_block = block_size.max(1);
        let tracks = (0..N_TRACKS)
            .map(|t| {
                // Build each track's engine from the shared kind mirror so a
                // freshly (re)activated plugin reflects a restored project's
                // engine selection (0174). Fresh instances default to KickTone
                // (mirror seeded to 0), preserving prior behaviour.
                let mut track = Track::new(sample_rate, max_block, io.swaps[t].clone());
                track.engine = crate::engines::make(io.kinds.get(t), sample_rate);
                track
            })
            .collect();
        let lanes = (0..N_TRACKS).map(LaneState::new).collect();
        Self {
            sample_rate,
            transport: Transport::default(),
            tracks,
            lanes,
            hits: Vec::with_capacity(HIT_CAPACITY),
            io,
            playhead_scratch: [PlayheadState::STOPPED; N_TRACKS],
            free_run_beats: 0.0,
            free_notes: Vec::with_capacity(FREE_NOTE_CAPACITY),
            delay: vxn3_dsp::Delay::new(sample_rate, DELAY_MAX_SECONDS),
            limiter: vxn3_dsp::Limiter::new(sample_rate, LIMITER_LOOKAHEAD as usize, LIMITER_CEILING),
            return_level: 0.35,
            delay_sync_beats: 0.75, // dotted-8th — a classic dub time
            master_volume: 1.0,
            delay_feedback: 0.5,
            send_l: vec![0.0; max_block],
            send_r: vec![0.0; max_block],
            wet_l: vec![0.0; max_block],
            wet_r: vec![0.0; max_block],
        }
    }

    /// The shared I/O handle set (clone for the main thread to drive the UI).
    pub fn io(&self) -> EngineIo {
        self.io.clone()
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        for t in &mut self.tracks {
            t.set_sample_rate(sample_rate);
        }
        // Rebuild rate-dependent FX (not on the audio path — activate builds a
        // fresh engine; this only fires from tests / explicit reconfig).
        self.delay = vxn3_dsp::Delay::new(sample_rate, DELAY_MAX_SECONDS);
        self.limiter =
            vxn3_dsp::Limiter::new(sample_rate, LIMITER_LOOKAHEAD as usize, LIMITER_CEILING);
    }

    pub fn set_transport(&mut self, transport: Transport) {
        self.transport = transport;
    }

    pub fn transport(&self) -> Transport {
        self.transport
    }

    /// Mutable access to a track's pattern (main-thread / tests). Panics if out
    /// of range.
    pub fn pattern_mut(&mut self, track: usize) -> &mut Pattern {
        &mut self.tracks[track].pattern
    }

    /// Mutable access to a track (gain/pan/engine). Panics if out of range.
    pub fn track_mut(&mut self, track: usize) -> &mut Track {
        &mut self.tracks[track]
    }

    /// A clone of a track's swap mailbox so the main thread can hand it a
    /// freshly built engine. Panics if out of range.
    pub fn track_swap(&self, track: usize) -> Arc<EngineSwap> {
        self.tracks[track].swap.clone()
    }

    /// Queue a live MIDI note (free-play, 0186) to trig `track`'s engine at `frame`
    /// within the **next** [`Engine::process_block`], sample-accurately. `note` is a
    /// (fractional) MIDI note driving the engine's pitch; `velocity` is `0..1`. Bounded
    /// — drops beyond [`FREE_NOTE_CAPACITY`] rather than allocate; an out-of-range track
    /// is ignored. Free-play trigs share the sequencer's hit path, so they mix with
    /// sequenced trigs into the same voice allocator without special-casing.
    pub fn queue_free_note(&mut self, track: usize, note: f32, velocity: f32, frame: u32) {
        if track < self.tracks.len() && self.free_notes.len() < FREE_NOTE_CAPACITY {
            self.free_notes.push(FreeNote { track: track as u8, frame, note, velocity });
        }
    }

    /// Render `left.len().min(right.len())` frames of stereo audio.
    /// Allocation-free.
    pub fn process_block(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len().min(right.len()).min(self.send_l.len());
        left[..frames].fill(0.0);
        right[..frames].fill(0.0);
        self.send_l[..frames].fill(0.0);
        self.send_r[..frames].fill(0.0);

        // 1. Apply queued UI edits, then install any pending engine swaps
        //    (both alloc-free on the audio thread).
        while let Some(cmd) = self.io.edits.pop() {
            self.apply_command(cmd);
        }
        let sr = self.sample_rate;
        for t in &mut self.tracks {
            // A freshly swapped-in engine may have been built at a stale sample
            // rate on the main thread; re-cook it for ours on install.
            if t.poll_swap() {
                t.engine.set_sample_rate(sr);
                // A swapped-in engine starts at its default patch — force the
                // next `apply_effective` to re-push every macro/lock value so a
                // restored (0174) or re-selected engine picks up the current
                // settings instead of staying at defaults.
                t.invalidate_applied();
            }
        }

        // 2/3/4. Sequence + render + mix per track.
        let bps = (self.transport.tempo_bpm / 60.0) / self.sample_rate as f64;
        let playing = self.transport.playing;
        let beat0 = if playing {
            self.transport.song_pos_beats.unwrap_or(self.free_run_beats)
        } else {
            self.free_run_beats
        };

        // Publish each lane's current step for the UI playhead.
        for t in 0..self.tracks.len() {
            self.playhead_scratch[t] = if playing {
                let sb = self.tracks[t].pattern.step_beats.max(1e-9);
                let len = self.tracks[t].pattern.len.clamp(1, crate::sequencer::MAX_STEPS) as i64;
                ((beat0 / sb).floor() as i64).rem_euclid(len) as u32
            } else {
                PlayheadState::STOPPED
            };
        }
        self.io.playhead.publish(&self.playhead_scratch, playing);

        for t in 0..self.tracks.len() {
            // Schedule this track's hits + advance its p-lock resolver (lane
            // state + pattern + hit scratch are disjoint fields), resolve the
            // effective params, then render + mix.
            self.lanes[t].schedule(
                &self.tracks[t].pattern,
                beat0,
                bps,
                frames,
                playing,
                &mut self.hits,
            );
            // Merge live MIDI free-play notes for this track (0186) into the scheduled
            // hits, then re-sort by frame so `render_with_hits` slices at each. Bounded
            // by hit capacity; `sort_unstable_by_key` is in-place (allocation-free).
            for fnote in &self.free_notes {
                if fnote.track as usize == t && self.hits.len() < HIT_CAPACITY {
                    self.hits.push(Hit {
                        frame: (fnote.frame as usize).min(frames),
                        note: fnote.note,
                        velocity: fnote.velocity,
                    });
                }
            }
            self.hits.sort_unstable_by_key(|h| h.frame);
            self.tracks[t].apply_effective(&self.lanes[t]);
            self.tracks[t].render_with_hits(&self.hits, frames);
            self.tracks[t].mix_into(
                &mut left[..frames],
                &mut right[..frames],
                &mut self.send_l[..frames],
                &mut self.send_r[..frames],
                frames,
            );
        }
        // Free-play notes were consumed by every track this block — clear (not free).
        self.free_notes.clear();

        // 5. Delay send bus → return into the master, then the terminal limiter.
        //    Delay time tracks host tempo (synced subdivision). Runs even when
        //    stopped so tails / self-oscillation ring out.
        if bps > 0.0 {
            let samps = (self.delay_sync_beats / bps).round() as usize;
            self.delay.set_delay_samples(samps);
        }
        self.delay.process(
            &self.send_l[..frames],
            &self.send_r[..frames],
            &mut self.wet_l[..frames],
            &mut self.wet_r[..frames],
        );
        for f in 0..frames {
            left[f] = (left[f] + self.wet_l[f] * self.return_level) * self.master_volume;
            right[f] = (right[f] + self.wet_r[f] * self.return_level) * self.master_volume;
        }
        self.limiter.process(&mut left[..frames], &mut right[..frames]);

        if playing {
            self.free_run_beats = beat0 + frames as f64 * bps;
        }
    }

    /// Apply one edit command to the engine. Bounds-checked; an out-of-range
    /// track is ignored. Allocation-free.
    ///
    /// Public so the CLAP shell can apply **host parameter automation** straight
    /// to the engine on the audio thread (0171) — the UI [`crate::EditQueue`] is
    /// strict SPSC and must keep a single producer, so host writes bypass it.
    pub fn apply_command(&mut self, cmd: EngineCommand) {
        // Master-bus commands carry no track.
        match cmd {
            EngineCommand::SetDelayFeedback { value } => {
                self.delay.set_feedback(value);
                self.delay_feedback = value;
                return;
            }
            EngineCommand::SetDelaySyncBeats { beats } => {
                self.delay_sync_beats = beats.max(0.001) as f64;
                return;
            }
            EngineCommand::SetMasterVolume { value } => {
                self.master_volume = value.clamp(0.0, 4.0);
                return;
            }
            EngineCommand::SetDelayReturn { value } => {
                self.return_level = value.clamp(0.0, 1.0);
                return;
            }
            _ => {}
        }
        let t = match &cmd {
            EngineCommand::ToggleStep { track, .. }
            | EngineCommand::SetStep { track, .. }
            | EngineCommand::SetProbability { track, .. }
            | EngineCommand::SetRetrig { track, .. }
            | EngineCommand::SetLength { track, .. }
            | EngineCommand::SetStepBeats { track, .. }
            | EngineCommand::SetGain { track, .. }
            | EngineCommand::SetPan { track, .. }
            | EngineCommand::SetMacro { track, .. }
            | EngineCommand::SetLock { track, .. }
            | EngineCommand::ClearLock { track, .. }
            | EngineCommand::SetSend { track, .. }
            | EngineCommand::SetMute { track, .. } => *track as usize,
            // Master commands handled above.
            EngineCommand::SetDelayFeedback { .. }
            | EngineCommand::SetDelaySyncBeats { .. }
            | EngineCommand::SetDelayReturn { .. }
            | EngineCommand::SetMasterVolume { .. } => return,
        };
        let Some(track) = self.tracks.get_mut(t) else {
            return;
        };
        match cmd {
            EngineCommand::ToggleStep { step, .. } => {
                let s = step as usize;
                if s < crate::sequencer::MAX_STEPS {
                    track.pattern.steps[s].active = !track.pattern.steps[s].active;
                }
            }
            EngineCommand::SetStep {
                step, note, velocity, ..
            } => track.pattern.set(step as usize, note, velocity),
            EngineCommand::SetProbability { step, probability, .. } => {
                track.pattern.set_probability(step as usize, probability)
            }
            EngineCommand::SetRetrig { step, retrig, .. } => {
                track.pattern.set_retrig(step as usize, retrig)
            }
            EngineCommand::SetLength { len, .. } => {
                track.pattern.len = (len as usize).clamp(1, crate::sequencer::MAX_STEPS)
            }
            EngineCommand::SetStepBeats { beats, .. } => {
                track.pattern.step_beats = beats.max(1e-4) as f64
            }
            EngineCommand::SetGain { gain, .. } => {
                track.set_base(LockParam::Gain, gain.max(0.0))
            }
            EngineCommand::SetPan { pan, .. } => {
                track.set_base(LockParam::Pan, pan.clamp(-1.0, 1.0))
            }
            EngineCommand::SetMacro { slot, value, .. } => {
                if let Some(p) = macro_to_lock_param(slot) {
                    track.set_base(p, value)
                }
            }
            EngineCommand::SetLock {
                step, param, lock, ..
            } => track.pattern.set_lock(step as usize, param, lock),
            EngineCommand::ClearLock { step, param, .. } => {
                track.pattern.clear_lock(step as usize, param)
            }
            EngineCommand::SetSend { amount, .. } => {
                track.set_base(LockParam::Send, amount.clamp(0.0, 1.0))
            }
            EngineCommand::SetMute { muted, .. } => track.set_muted(muted),
            // Master commands were dispatched above.
            EngineCommand::SetDelayFeedback { .. }
            | EngineCommand::SetDelaySyncBeats { .. }
            | EngineCommand::SetDelayReturn { .. }
            | EngineCommand::SetMasterVolume { .. } => {}
        }
    }

    // ── Effective host-param readback (for the 0173 echo pump) ───────────────
    // These report the *resolved* value the mix currently uses (base value, or a
    // p-lock override this block for the lockable lanes), so faceplate edits and
    // p-locks can be echoed back to the host.

    /// A track's effective (post-p-lock) lockable-param value, or 0.0 if the
    /// track is out of range.
    pub fn track_effective(&self, track: usize, param: LockParam) -> f32 {
        self.tracks.get(track).map_or(0.0, |t| t.effective(param))
    }

    /// Whether a track is muted (0.0/1.0 as a host mix param).
    pub fn track_muted(&self, track: usize) -> bool {
        self.tracks.get(track).is_some_and(|t| t.is_muted())
    }

    /// A track's active engine kind, or `None` if out of range (introspection /
    /// state round-trip assertions).
    pub fn track_kind(&self, track: usize) -> Option<crate::EngineKind> {
        self.tracks.get(track).map(|t| t.engine.kind())
    }

    /// Master output volume (linear).
    pub fn master_volume(&self) -> f32 {
        self.master_volume
    }

    /// Delay feedback amount.
    pub fn delay_feedback(&self) -> f32 {
        self.delay_feedback
    }

    /// Delay time as a tempo-synced subdivision in beats.
    pub fn delay_time_beats(&self) -> f32 {
        self.delay_sync_beats as f32
    }

    /// Delay return level into the master mix.
    pub fn delay_return(&self) -> f32 {
        self.return_level
    }

    /// Drop voices / decaying state on every track, reset lane phase, and rewind
    /// the free-run clock.
    pub fn reset(&mut self) {
        for t in &mut self.tracks {
            t.reset();
        }
        for l in &mut self.lanes {
            l.reset();
        }
        self.delay.reset();
        self.limiter.reset();
        self.free_run_beats = 0.0;
    }

    /// The plugin's reported processing latency in samples (the master limiter
    /// look-ahead). Constant — the CLAP shell reports it once.
    pub fn latency_samples(&self) -> u32 {
        LIMITER_LOOKAHEAD
    }
}
