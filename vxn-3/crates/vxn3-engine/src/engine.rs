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
use crate::track_engine::Knob;
use crate::transport::Transport;

/// Map a faceplate knob to its lockable-param slot.
fn knob_to_lock_param(knob: Knob) -> LockParam {
    match knob {
        Knob::Decay => LockParam::Decay,
        Knob::Tone => LockParam::Tone,
        Knob::Pitch => LockParam::Pitch,
    }
}

/// Fixed track count (ADR 0001 — eight tracks for the minimal-techno kit).
pub const N_TRACKS: usize = 8;

/// Per-track per-block hit-buffer capacity. Generous vs. the realistic worst
/// case (a fast lane + a dense retrig in one block); the scheduler drops beyond
/// this rather than allocate.
const HIT_CAPACITY: usize = 256;

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
            .map(|t| Track::new(sample_rate, max_block, io.swaps[t].clone()))
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

    /// Render `left.len().min(right.len())` frames of stereo audio.
    /// Allocation-free.
    pub fn process_block(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len().min(right.len());
        left[..frames].fill(0.0);
        right[..frames].fill(0.0);

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
            self.tracks[t].apply_effective(&self.lanes[t]);
            self.tracks[t].render_with_hits(&self.hits, frames);
            self.tracks[t].mix_into(&mut left[..frames], &mut right[..frames], frames);
        }

        if playing {
            self.free_run_beats = beat0 + frames as f64 * bps;
        }
    }

    /// Apply one UI edit command to the addressed track. Bounds-checked; an
    /// out-of-range track is ignored. Allocation-free.
    fn apply_command(&mut self, cmd: EngineCommand) {
        let t = match &cmd {
            EngineCommand::ToggleStep { track, .. }
            | EngineCommand::SetStep { track, .. }
            | EngineCommand::SetProbability { track, .. }
            | EngineCommand::SetRetrig { track, .. }
            | EngineCommand::SetLength { track, .. }
            | EngineCommand::SetStepBeats { track, .. }
            | EngineCommand::SetGain { track, .. }
            | EngineCommand::SetPan { track, .. }
            | EngineCommand::SetKnob { track, .. }
            | EngineCommand::SetLock { track, .. }
            | EngineCommand::ClearLock { track, .. } => *track as usize,
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
            EngineCommand::SetKnob { knob, value, .. } => {
                track.set_base(knob_to_lock_param(knob), value)
            }
            EngineCommand::SetLock {
                step, param, lock, ..
            } => track.pattern.set_lock(step as usize, param, lock),
            EngineCommand::ClearLock { step, param, .. } => {
                track.pattern.clear_lock(step as usize, param)
            }
        }
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
        self.free_run_beats = 0.0;
    }
}
