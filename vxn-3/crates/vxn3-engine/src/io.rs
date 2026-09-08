//! Main↔audio I/O for the faceplate (ticket 0052).
//!
//! UI edits flow **main → audio** over [`EditQueue`], a lock-free SPSC ring of
//! `Copy` [`EngineCommand`]s drained by the engine at the top of each block.
//! Engine *selection* is not here — it carries a heap-allocated engine and so
//! uses the [`crate::swap::EngineSwap`] retire path instead.
//!
//! Playhead state flows **audio → main** through [`PlayheadState`] atomics: the
//! engine publishes each lane's current subdivision-slot index every block; the
//! GUI timer reads them to drive the per-lane playhead.

use std::cell::UnsafeCell;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::engine::N_TRACKS;
use crate::flavour::Flavour;
use crate::sequencer::{Lock, LockParam, Retrig};
use crate::track_engine::EngineKind;

/// A data-only edit from the UI to the engine. `Copy` so the queue is a plain
/// ring with no heap ownership transfer (engine *swaps* go via `EngineSwap`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum EngineCommand {
    /// Add or remove a hit welded to a subdivision slot (the snapped-cell verb).
    ToggleHit { track: u8, slot: u16 },
    /// Set a hit's note + velocity, adding one in `slot` if there is none.
    SetHit { track: u8, slot: u16, note: f32, velocity: f32 },
    /// Set a hit's fire probability, adding one in `slot` if there is none.
    SetProbability { track: u8, slot: u16, probability: f32 },
    /// Set a hit's retrig macro, adding one in `slot` if there is none.
    SetRetrig { track: u8, slot: u16, retrig: Retrig },
    /// Add a freely-positioned hit (ADR 0007 §4, ticket 0353). The lane strip's
    /// placement verb: the position is a coordinate, not a slot index, and an
    /// over-capacity add **drops** — the editor enforces [`crate::MAX_HITS`]
    /// itself so the user sees the ceiling rather than a hit vanishing.
    AddHit {
        track: u8,
        beat: u16,
        sub: u8,
        f: f32,
        nudge: i16,
        y: f32,
        note: f32,
        velocity: f32,
    },
    /// Remove a hit by **fire-order index**, the key every verb below shares: with
    /// hits placed freely a slot can hold several, so a slot cannot name one.
    RemoveHit { track: u8, hit: u16 },
    /// Move a hit — its `(beat, sub)` and in-slot offset together, because a drag
    /// across a beat marker changes both (the drag verb).
    SetHitPosition {
        track: u8,
        hit: u16,
        beat: u16,
        sub: u8,
        f: f32,
        nudge: i16,
    },
    /// Set a hit's position on the lane's modulation axis (ADR 0007 §6).
    SetHitY { track: u8, hit: u16, y: f32 },
    /// Re-pitch a hit — what reassigning a lane's voice has to do to every hit it
    /// holds, since a hat's open/closed identity *is* its note.
    SetHitNote { track: u8, hit: u16, note: f32, velocity: f32 },
    /// Set a hit's fire probability (hit-keyed form of [`Self::SetProbability`]).
    SetHitProbability { track: u8, hit: u16, probability: f32 },
    /// Set a hit's retrig macro (hit-keyed form of [`Self::SetRetrig`]).
    SetHitRetrig { track: u8, hit: u16, retrig: Retrig },
    /// Quantise a hit toward its nearest subdivision marker, `amount ∈ [0, 1]`.
    QuantiseHitX { track: u8, hit: u16, amount: f32 },
    /// Quantise a hit toward the groove's Y-centre curve. Separate from
    /// [`Self::QuantiseHitX`] on purpose: X and Y are not corrected together.
    QuantiseHitY { track: u8, hit: u16, amount: f32 },
    /// Set a lane's beat count (and its length to match) — polymeter (0348).
    SetGridBeats { track: u8, beats: u8 },
    /// Set a lane's subdivisions per beat — what `step_beats` was, as geometry.
    SetGridSubs { track: u8, subs: u8 },
    /// Set a track's linear gain.
    SetGain { track: u8, gain: f32 },
    /// Set a track's pan (-1..1).
    SetPan { track: u8, pan: f32 },
    /// Set one of a track engine's generic macro slots (0..1). The active engine
    /// reinterprets the slot onto its patch (ADR 0003 §2).
    SetMacro { track: u8, slot: u8, value: f32 },
    /// Set a p-lock on a continuous param. Keyed by **hit index** (ADR 0007 §4):
    /// a lock belongs to a hit, not to a grid cell.
    SetLock {
        track: u8,
        hit: u16,
        param: LockParam,
        lock: Lock,
    },
    /// Clear a hit's p-lock.
    ClearLock {
        track: u8,
        hit: u16,
        param: LockParam,
    },
    /// Set a track's delay-send amount (0..1).
    SetSend { track: u8, amount: f32 },
    /// Mute / unmute a track (gates its mix contribution).
    SetMute { track: u8, muted: bool },
    /// Assign a track's choke group (0 = none). Members of a non-zero group cut each other.
    SetChokeGroup { track: u8, group: u8 },
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
            slots: [const { UnsafeCell::new(EngineCommand::ToggleHit { track: 0, slot: 0 }) };
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
/// `step[t]` is the lane's current subdivision-slot index within its pass, or
/// [`PlayheadState::STOPPED`] when not playing. `generation` bumps every block so
/// the UI can tell "still alive" from "stalled".
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

    /// **Audio thread:** publish this block's lane slot indices + play state.
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

/// Per-track main-thread mirror of each lane's active deep patch (**flavour**) — the
/// source of truth the CLAP shell serialises into `clap.state` and reads for
/// flavour-aware `value_to_text` (0185). Written by the app when a voice is assigned;
/// only ever touched on the main thread, so the `Mutex` is uncontended (it exists to
/// keep [`EngineIo`] `Sync`, since the handle is cloned to the audio thread too).
pub struct FlavourStore {
    flavours: Mutex<Vec<Flavour>>,
}

impl FlavourStore {
    pub fn new() -> Arc<Self> {
        let init = (0..N_TRACKS)
            .map(|_| crate::engines::default_flavour_for(EngineKind::KickTone))
            .collect();
        Arc::new(Self { flavours: Mutex::new(init) })
    }

    /// Store a track's active flavour (main thread, on voice assign / state load).
    pub fn set(&self, track: usize, flavour: Flavour) {
        if let Ok(mut g) = self.flavours.lock() {
            if let Some(slot) = g.get_mut(track) {
                *slot = flavour;
            }
        }
    }

    /// A clone of a track's flavour (for `clap.state` save).
    pub fn get(&self, track: usize) -> Flavour {
        self.flavours
            .lock()
            .ok()
            .and_then(|g| g.get(track).cloned())
            .unwrap_or_else(|| crate::engines::default_flavour_for(EngineKind::KickTone))
    }

    /// Read a track's flavour without cloning (for `value_to_text`). `None` if the lock
    /// is poisoned or the track is out of range.
    pub fn with<R>(&self, track: usize, f: impl FnOnce(&Flavour) -> R) -> Option<R> {
        self.flavours.lock().ok().and_then(|g| g.get(track).map(f))
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
    /// Per-track active flavour (main-thread deep-patch store; 0185).
    pub flavours: Arc<FlavourStore>,
}

impl EngineIo {
    pub fn new() -> Self {
        Self {
            edits: EditQueue::new(),
            playhead: PlayheadState::new(),
            swaps: (0..N_TRACKS).map(|_| crate::swap::EngineSwap::new()).collect(),
            kinds: TrackKinds::new(),
            flavours: FlavourStore::new(),
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
