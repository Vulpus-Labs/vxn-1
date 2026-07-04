//! Main↔audio I/O for the faceplate (ticket 0052).
//!
//! UI edits flow **main → audio** over [`EditQueue`], a lock-free SPSC ring of
//! `Copy` [`EngineCommand`]s drained by the engine at the top of each block.
//! Engine *selection* is not here — it carries a heap-allocated engine and so
//! uses the [`crate::swap::EngineSwap`] retire path instead.
//!
//! Playhead state flows **audio → main** through [`PlayheadState`] atomics: the
//! engine publishes each lane's current step index every block; the GUI timer
//! reads them to drive the per-lane playhead.

use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::engine::N_TRACKS;
use crate::sequencer::{Lock, LockParam, Retrig};
use crate::track_engine::EngineKind;

/// A data-only edit from the UI to the engine. `Copy` so the queue is a plain
/// ring with no heap ownership transfer (engine *swaps* go via `EngineSwap`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum EngineCommand {
    /// Toggle a step's active flag.
    ToggleStep { track: u8, step: u8 },
    /// Set (and enable) a step's note + velocity.
    SetStep { track: u8, step: u8, note: f32, velocity: f32 },
    /// Set (and enable) a step's fire probability.
    SetProbability { track: u8, step: u8, probability: f32 },
    /// Set (and enable) a step's retrig macro.
    SetRetrig { track: u8, step: u8, retrig: Retrig },
    /// Set a lane's active length (steps) — polymeter.
    SetLength { track: u8, len: u8 },
    /// Set a lane's step duration in beats (lane-local tick).
    SetStepBeats { track: u8, beats: f32 },
    /// Set a track's linear gain.
    SetGain { track: u8, gain: f32 },
    /// Set a track's pan (-1..1).
    SetPan { track: u8, pan: f32 },
    /// Set one of a track engine's generic macro slots (0..1). The active engine
    /// reinterprets the slot onto its patch (ADR 0003 §2).
    SetMacro { track: u8, slot: u8, value: f32 },
    /// Set a per-step p-lock on a continuous param.
    SetLock {
        track: u8,
        step: u8,
        param: LockParam,
        lock: Lock,
    },
    /// Clear a per-step p-lock.
    ClearLock {
        track: u8,
        step: u8,
        param: LockParam,
    },
    /// Set a track's delay-send amount (0..1).
    SetSend { track: u8, amount: f32 },
    /// Mute / unmute a track (gates its mix contribution).
    SetMute { track: u8, muted: bool },
    /// Master output volume (linear gain, applied pre-limiter).
    SetMasterVolume { value: f32 },
    /// Master delay feedback (0..~1.3; >1 self-oscillates).
    SetDelayFeedback { value: f32 },
    /// Master delay time as a tempo-synced subdivision in beats.
    SetDelaySyncBeats { beats: f32 },
    /// Master delay return level into the mix (0..1).
    SetDelayReturn { value: f32 },
}

/// SPSC ring capacity. UI edits are human-paced; a tick's worth fits easily.
const QUEUE_CAP: usize = 256;

/// Lock-free SPSC queue of edit commands (main = producer, audio = consumer).
pub struct EditQueue {
    slots: [UnsafeCell<EngineCommand>; QUEUE_CAP],
    head: AtomicU32, // producer
    tail: AtomicU32, // consumer
}

// SAFETY: strict SPSC — `head` written only by the producer, `tail` only by the
// consumer, each slot handed off via the Acquire/Release pair on those indices.
// `EngineCommand` is `Copy + Send`.
unsafe impl Send for EditQueue {}
unsafe impl Sync for EditQueue {}

impl EditQueue {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            slots: [const { UnsafeCell::new(EngineCommand::ToggleStep { track: 0, step: 0 }) };
                QUEUE_CAP],
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
        })
    }

    /// **Main thread:** enqueue a command. Dropped (returns `false`) if full —
    /// preferable to blocking; the UI will re-send on the next edit.
    pub fn push(&self, cmd: EngineCommand) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) % QUEUE_CAP as u32;
        if next == self.tail.load(Ordering::Acquire) {
            return false; // full
        }
        // SAFETY: SPSC — only the producer writes this slot before publishing head.
        unsafe { *self.slots[head as usize].get() = cmd };
        self.head.store(next, Ordering::Release);
        true
    }

    /// **Audio thread:** pop the next command, or `None` when empty.
    pub fn pop(&self) -> Option<EngineCommand> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: SPSC — producer published this slot via its head store.
        let cmd = unsafe { *self.slots[tail as usize].get() };
        self.tail.store((tail + 1) % QUEUE_CAP as u32, Ordering::Release);
        Some(cmd)
    }
}

/// Per-lane playhead, published by the audio thread, read by the GUI timer.
///
/// `step[t]` is the lane's current step index, or [`PlayheadState::STOPPED`]
/// when not playing. `generation` bumps every block so the UI can tell "still
/// alive" from "stalled".
pub struct PlayheadState {
    step: [AtomicU32; N_TRACKS],
    generation: AtomicU32,
    playing: AtomicBool,
}

impl PlayheadState {
    /// Sentinel for "no current step" (transport stopped).
    pub const STOPPED: u32 = u32::MAX;

    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            step: [const { AtomicU32::new(Self::STOPPED) }; N_TRACKS],
            generation: AtomicU32::new(0),
            playing: AtomicBool::new(false),
        })
    }

    /// **Audio thread:** publish this block's lane step indices + play state.
    pub fn publish(&self, steps: &[u32; N_TRACKS], playing: bool) {
        for (a, &s) in self.step.iter().zip(steps.iter()) {
            a.store(s, Ordering::Relaxed);
        }
        self.playing.store(playing, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// **Main thread:** read a lane's current step (or [`Self::STOPPED`]).
    pub fn step(&self, track: usize) -> u32 {
        self.step[track].load(Ordering::Relaxed)
    }

    pub fn playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    pub fn generation(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }
}

/// Main-thread mirror of each track's active [`EngineKind`]. The app writes it
/// when it issues an engine swap (`SetEngine`); the CLAP shell reads it so
/// `value_to_text` can render a macro slot engine-aware (0172) without touching
/// the live engine on the audio thread. Seeded to the default engine a fresh
/// track loads (`KickTone`).
pub struct TrackKinds {
    kinds: [AtomicU32; N_TRACKS],
}

impl TrackKinds {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            kinds: [const { AtomicU32::new(0) }; N_TRACKS], // 0 = KickTone
        })
    }

    /// Record a track's active engine kind (app main thread, on swap).
    pub fn set(&self, track: usize, kind: EngineKind) {
        if let Some(a) = self.kinds.get(track) {
            a.store(kind.as_u8() as u32, Ordering::Relaxed);
        }
    }

    /// Read a track's active engine kind (CLAP main thread, for value-text).
    pub fn get(&self, track: usize) -> EngineKind {
        EngineKind::from_u8(self.kinds.get(track).map_or(0, |a| a.load(Ordering::Relaxed) as u8))
    }
}

/// The shared main↔audio I/O handles, created once and cloned to both threads.
#[derive(Clone)]
pub struct EngineIo {
    pub edits: Arc<EditQueue>,
    pub playhead: Arc<PlayheadState>,
    pub swaps: Vec<Arc<crate::swap::EngineSwap>>,
    /// Per-track active engine kind (main-thread mirror, for value-text; 0172).
    pub kinds: Arc<TrackKinds>,
}

impl EngineIo {
    pub fn new() -> Self {
        Self {
            edits: EditQueue::new(),
            playhead: PlayheadState::new(),
            swaps: (0..N_TRACKS).map(|_| crate::swap::EngineSwap::new()).collect(),
            kinds: TrackKinds::new(),
        }
    }
}

impl Default for EngineIo {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_fifo_roundtrip() {
        let q = EditQueue::new();
        assert!(q.push(EngineCommand::SetGain { track: 1, gain: 0.5 }));
        assert!(q.push(EngineCommand::SetPan { track: 2, pan: -0.3 }));
        assert_eq!(q.pop(), Some(EngineCommand::SetGain { track: 1, gain: 0.5 }));
        assert_eq!(q.pop(), Some(EngineCommand::SetPan { track: 2, pan: -0.3 }));
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn playhead_publishes_and_reads() {
        let p = PlayheadState::new();
        assert_eq!(p.step(0), PlayheadState::STOPPED);
        let mut steps = [PlayheadState::STOPPED; N_TRACKS];
        steps[0] = 3;
        steps[1] = 7;
        p.publish(&steps, true);
        assert_eq!(p.step(0), 3);
        assert_eq!(p.step(1), 7);
        assert!(p.playing());
        assert_eq!(p.generation(), 1);
    }
}
