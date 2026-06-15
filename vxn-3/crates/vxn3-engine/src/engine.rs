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

use crate::lane::{Hit, LaneState};
use crate::sequencer::Pattern;
use crate::swap::EngineSwap;
use crate::track::Track;
use crate::transport::Transport;

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
    /// Beat position used when the host exposes no beats timeline — advanced by
    /// the block length each playing block so the sequencer free-runs.
    free_run_beats: f64,
}

impl Engine {
    pub fn new(sample_rate: f32, block_size: usize) -> Self {
        let max_block = block_size.max(1);
        let tracks = (0..N_TRACKS)
            .map(|_| Track::new(sample_rate, max_block))
            .collect();
        let lanes = (0..N_TRACKS).map(LaneState::new).collect();
        Self {
            sample_rate,
            transport: Transport::default(),
            tracks,
            lanes,
            hits: Vec::with_capacity(HIT_CAPACITY),
            free_run_beats: 0.0,
        }
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

        // 1. Install any pending off-thread engine swaps (alloc-free).
        for t in &mut self.tracks {
            t.poll_swap();
        }

        // 2/3/4. Sequence + render + mix per track.
        let bps = (self.transport.tempo_bpm / 60.0) / self.sample_rate as f64;
        let playing = self.transport.playing;
        let beat0 = if playing {
            self.transport.song_pos_beats.unwrap_or(self.free_run_beats)
        } else {
            self.free_run_beats
        };

        for t in 0..self.tracks.len() {
            // Schedule this track's hits (lane state + pattern + hit scratch are
            // disjoint fields), then render + mix.
            self.lanes[t].schedule(
                &self.tracks[t].pattern,
                beat0,
                bps,
                frames,
                playing,
                &mut self.hits,
            );
            self.tracks[t].render_with_hits(&self.hits, frames);
            self.tracks[t].mix_into(&mut left[..frames], &mut right[..frames], frames);
        }

        if playing {
            self.free_run_beats = beat0 + frames as f64 * bps;
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
