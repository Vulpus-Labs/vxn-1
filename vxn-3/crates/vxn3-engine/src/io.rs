//! Main↔audio I/O for the faceplate (ticket 0052).
//!
//! vxn-3 is MVC with the **model on the main thread**. [`PatternStore`] holds the
//! authoritative per-lane [`Pattern`]; the faceplate is a view over it and reads
//! it back directly, and the audio thread works from an *internal copy* kept in
//! step by deltas. Truth flows main → audio, never the other way (0366).
//!
//! Those deltas are [`EngineCommand`]s over [`EditQueue`], a lock-free SPSC ring
//! drained by the engine at the top of each block. Both sides apply them through
//! the same [`apply_pattern_command`], so "in step" is one implementation rather
//! than two that have to be kept in agreement. Engine *selection* is not a delta —
//! it carries a heap-allocated engine and uses the [`crate::swap::EngineSwap`]
//! retire path instead.
//!
//! When the model is *replaced* rather than edited — a state restore — deltas are
//! the wrong shape and a **full flush** goes down instead: the whole `Pattern`
//! travels out of band through the flush ring, with an
//! [`EngineCommand::LoadPattern`] marker in the edit queue marking where in the
//! delta stream it lands. See [`EngineIo::flush_lane`].
//!
//! Only two things flow **audio → main**, and neither is state the view edits:
//! [`PlayheadState`] atomics (each lane's current subdivision slot, per block) and
//! the retired engines `EngineSwap` hands back to be dropped off-thread.

use std::cell::UnsafeCell;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::engine::N_TRACKS;
use crate::flavour::Flavour;
use crate::grid::Swing;
use crate::sequencer::{Lock, LockParam, Pattern, Retrig};
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
    /// Paint a hit's colour — its **macro vector** (ADR 0007 §7, ticket 0355).
    /// Channels are normalised `0.00–1.00`, the form the macro slots take; there is
    /// no `0–255` anywhere on this path.
    ///
    /// Distinct from [`Self::ClearHitColour`] rather than folded into it with a
    /// sentinel triple, because the two mean opposite things at trig time: black is a
    /// colour that sends zero to all three slots, and no colour sends nothing.
    SetHitColour { track: u8, hit: u16, rgb: [f32; 3] },
    /// Strip a hit's colour, handing its macro slots back to the p-lock/base.
    ClearHitColour { track: u8, hit: u16 },
    /// Quantise a hit toward its nearest subdivision marker, `amount ∈ [0, 1]`.
    QuantiseHitX { track: u8, hit: u16, amount: f32 },
    /// Quantise a hit toward the groove's Y-centre curve. Separate from
    /// [`Self::QuantiseHitX`] on purpose: X and Y are not corrected together.
    QuantiseHitY { track: u8, hit: u16, amount: f32 },
    /// Set a lane's beat count (and its length to match) — polymeter (0348).
    SetGridBeats { track: u8, beats: u8 },
    /// Set a lane's subdivisions per beat — what `step_beats` was, as geometry.
    SetGridSubs { track: u8, subs: u8 },
    /// Drag beat marker `marker` to `pos` beats, **preserving relative position**:
    /// slot `marker - 1` stretches and slot `marker` squashes at once, so hits
    /// rubber-band in both directions from one grab (ADR 0007 §5, ticket 0354).
    ///
    /// `pos` is a request, not a write. It goes through
    /// [`crate::Grid::set_beat_marker`]'s clamp, which is the only path into the
    /// marker array — so no editor can name a position the geometry cannot take, and
    /// the pinned outer markers refuse this outright.
    DragBeatMarker { track: u8, marker: u8, pos: f64 },
    /// Insert a beat marker at `pos`, taking index `marker` and splitting the slot
    /// before it — **preserving absolute time**, so the split moves nothing on
    /// screen. The opposite rule to a drag, and deliberately a separate verb.
    ///
    /// `subs` **states the new beat's sub-count override outright** (`0` = none),
    /// applied inside the same absolute-preserving edit. It rides the insert rather
    /// than following as a [`Self::SetBeatSubs`] because that verb takes the
    /// *relative* door: as two commands they compose to "preserve times, then move
    /// everything in that beat", which is how undoing a marker delete came to
    /// displace the hits it promised not to touch.
    ///
    /// Stated rather than defaulted, because the two readings of "unset" differ and
    /// the editor mirrors this command locally. [`crate::Grid::insert_beat_marker`]
    /// gives the new beat the *split beat's* override, so "leave it alone" and "clear
    /// it" are different outcomes on a tuplet — and a page assuming one while the
    /// engine took the other would part company with the model over a plain insert.
    /// An ordinary insert therefore sends what the split inherits; only undo of a
    /// delete sends something else, to put back the override the merge ate.
    InsertBeatMarker { track: u8, marker: u8, pos: f64, subs: u8 },
    /// Delete beat marker `marker`, merging the slots either side of it — absolute-
    /// preserving like the insert, so the merge moves nothing on screen either.
    DeleteBeatMarker { track: u8, marker: u8 },
    /// Set a lane's swing warp: shape, amount and the period it spans (0347, 0365).
    /// A geometry edit like a marker drag — the subdivision markers move and the
    /// hits welded to them (`f = 0`) move exactly with them.
    SetSwing { track: u8, swing: Swing },
    /// Set or clear one beat's subdivision override — `subs: 0` clears it back to the
    /// lane default. This is where tuplets live (ADR 0007 §2): three in an otherwise
    /// 16ths lane, with no separate tuplet concept.
    SetBeatSubs { track: u8, beat: u8, subs: u8 },
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
    /// Replace a lane wholesale from the **flush ring** (0366): the marker half of
    /// a full model→engine flush, carrying only the track because a `Pattern` is
    /// kilobytes and this ring's slots are words.
    ///
    /// It rides the edit queue so the replacement lands at an exact point in the
    /// delta stream — a flush and the edits around it have one order, not two.
    LoadPattern { track: u8 },
}

impl EngineCommand {
    /// The track this command addresses, or `None` for the master bus. The single
    /// routing table: both the audio engine and the main-thread model dispatch
    /// through it, so a new per-track verb cannot reach one and miss the other.
    pub fn track(&self) -> Option<u8> {
        match self {
            Self::ToggleHit { track, .. }
            | Self::SetHit { track, .. }
            | Self::SetProbability { track, .. }
            | Self::SetRetrig { track, .. }
            | Self::AddHit { track, .. }
            | Self::RemoveHit { track, .. }
            | Self::SetHitPosition { track, .. }
            | Self::SetHitY { track, .. }
            | Self::SetHitNote { track, .. }
            | Self::SetHitProbability { track, .. }
            | Self::SetHitRetrig { track, .. }
            | Self::SetHitColour { track, .. }
            | Self::ClearHitColour { track, .. }
            | Self::QuantiseHitX { track, .. }
            | Self::QuantiseHitY { track, .. }
            | Self::SetGridBeats { track, .. }
            | Self::SetGridSubs { track, .. }
            | Self::DragBeatMarker { track, .. }
            | Self::InsertBeatMarker { track, .. }
            | Self::DeleteBeatMarker { track, .. }
            | Self::SetSwing { track, .. }
            | Self::SetBeatSubs { track, .. }
            | Self::SetGain { track, .. }
            | Self::SetPan { track, .. }
            | Self::SetMacro { track, .. }
            | Self::SetLock { track, .. }
            | Self::ClearLock { track, .. }
            | Self::SetSend { track, .. }
            | Self::SetMute { track, .. }
            | Self::SetChokeGroup { track, .. }
            | Self::LoadPattern { track } => Some(*track),
            Self::SetDelayFeedback { .. }
            | Self::SetDelaySyncBeats { .. }
            | Self::SetDelayReturn { .. }
            | Self::SetMasterVolume { .. } => None,
        }
    }
}

/// Apply a lane edit to one [`Pattern`], reporting whether the command was a lane
/// edit at all (`false` for mix, master and engine verbs, which live elsewhere).
///
/// **The single implementation**, called by the main-thread model in
/// [`PatternStore::apply`] and by the audio thread's copy in
/// [`crate::Engine::apply_command`]. That is what keeps the two in step: they are
/// not two implementations kept in agreement, they are one, fed the same commands
/// in the same order over an in-order queue, mutating a deterministic `Copy`
/// struct. Divergence would need a *dropped* command — see [`EditQueue::push`].
///
/// Pure and allocation-free, because the audio thread's call is on the audio thread.
pub fn apply_pattern_command(pattern: &mut Pattern, cmd: EngineCommand) -> bool {
    match cmd {
        EngineCommand::ToggleHit { slot, .. } => pattern.toggle(slot as usize),
        EngineCommand::SetHit {
            slot, note, velocity, ..
        } => pattern.set(slot as usize, note, velocity),
        EngineCommand::SetProbability { slot, probability, .. } => {
            pattern.set_probability(slot as usize, probability)
        }
        EngineCommand::SetRetrig { slot, retrig, .. } => pattern.set_retrig(slot as usize, retrig),
        // The freely-positioned hit verbs (0353). `insert`'s over-capacity `None`
        // is dropped on purpose: the ceiling is the editor's to show.
        EngineCommand::AddHit {
            beat,
            sub,
            f,
            nudge,
            y,
            note,
            velocity,
            ..
        } => {
            pattern.insert(crate::sequencer::Hit {
                f,
                nudge,
                y,
                note,
                velocity,
                ..crate::sequencer::Hit::at(beat, sub)
            });
        }
        EngineCommand::RemoveHit { hit, .. } => pattern.remove(hit as usize),
        EngineCommand::SetHitPosition {
            hit,
            beat,
            sub,
            f,
            nudge,
            ..
        } => {
            pattern.set_position(hit as usize, beat, sub, f, nudge);
        }
        EngineCommand::SetHitY { hit, y, .. } => pattern.set_hit_y(hit as usize, y),
        EngineCommand::SetHitNote {
            hit, note, velocity, ..
        } => pattern.set_hit_note(hit as usize, note, velocity),
        EngineCommand::SetHitProbability { hit, probability, .. } => {
            pattern.set_hit_probability(hit as usize, probability)
        }
        EngineCommand::SetHitRetrig { hit, retrig, .. } => {
            pattern.set_hit_retrig(hit as usize, retrig)
        }
        // The palette's two verbs (0355). Stored as sent: the editor's display floors
        // a dark colour to keep the diamond visible, and that floor stays in the
        // editor — what reaches the slots is what the user dialled.
        EngineCommand::SetHitColour { hit, rgb, .. } => pattern.set_colour(hit as usize, rgb),
        EngineCommand::ClearHitColour { hit, .. } => pattern.clear_colour(hit as usize),
        EngineCommand::QuantiseHitX { hit, amount, .. } => {
            pattern.quantise_x(hit as usize, amount);
        }
        EngineCommand::QuantiseHitY { hit, amount, .. } => pattern.quantise_y(hit as usize, amount),
        EngineCommand::SetGridBeats { beats, .. } => pattern.set_grid_beats(beats as usize),
        EngineCommand::SetGridSubs { subs, .. } => pattern.set_grid_subs(subs as u32),
        // The marker verbs (0354). Drag is relative and insert/delete absolute
        // (ADR 0007 §5), which is why they are three verbs rather than one with a
        // flag: the asymmetry belongs in the vocabulary, not in a parameter.
        EngineCommand::DragBeatMarker { marker, pos, .. } => {
            pattern.drag_beat_marker(marker as usize, pos);
        }
        EngineCommand::InsertBeatMarker { marker, pos, subs, .. } => {
            let subs = if subs == 0 { None } else { Some(subs as u32) };
            pattern.insert_beat_marker_with_subs(marker as usize, pos, subs);
        }
        EngineCommand::DeleteBeatMarker { marker, .. } => {
            pattern.delete_beat_marker(marker as usize);
        }
        // Swing and sub-count take the *relative* door, like a drag: the markers move
        // and the hits hanging off them move with them, so a welded hit stays welded
        // across a swing sweep instead of being left behind at an absolute time.
        EngineCommand::SetSwing { swing, .. } => pattern.edit_grid(|g| g.set_swing(swing)),
        EngineCommand::SetBeatSubs { beat, subs, .. } => pattern.edit_grid(|g| {
            g.set_beat_subs(beat as usize, if subs == 0 { None } else { Some(subs as u32) })
        }),
        EngineCommand::SetLock {
            hit, param, lock, ..
        } => pattern.set_lock(hit as usize, param, lock),
        EngineCommand::ClearLock { hit, param, .. } => pattern.clear_lock(hit as usize, param),
        // Not lane edits: track mix, engine macros, master bus — and `LoadPattern`,
        // which is a *replacement* and carries its payload out of band.
        _ => return false,
    }
    true
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

    /// **Main thread:** is there room for one more push? A conservative snapshot —
    /// the consumer may free space concurrently, so `false` can go stale but `true`
    /// cannot (only this thread fills it). That asymmetry is what lets
    /// [`EngineIo::flush_lane`] reserve the queue slot for its marker *before*
    /// committing a pattern to the flush ring, so the two can never come apart.
    pub fn can_push(&self) -> bool {
        let next = (self.head.load(Ordering::Relaxed) + 1) % QUEUE_CAP as u32;
        next != self.tail.load(Ordering::Acquire)
    }

    /// **Main thread:** enqueue a command. Dropped (returns `false`) if full —
    /// preferable to blocking; the UI will re-send on the next edit.
    ///
    /// A drop is the one way the main-thread model and the engine's copy can
    /// diverge (0366), so the model is only advanced when the push *succeeds* —
    /// see [`PatternStore::apply`]'s caller.
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

/// The **authoritative per-lane model** (ticket 0366).
///
/// vxn-3's faceplate is a view, not a store: it signals edits to the controller
/// and reads its picture back from here. The audio thread holds its own copy of
/// each lane inside [`crate::Track`], kept in step by the [`EngineCommand`]
/// deltas the controller enqueues alongside every mutation it makes here.
///
/// That direction is the whole point. The engine's copy used to be the *only*
/// copy, which is why a reopened GUI had nothing to read and its hit-keyed edits
/// — 0353's vocabulary is keyed by fire-order index — went to whichever hit
/// happened to sit at that index. Reading the pattern back off the audio thread
/// would have fixed the symptom by making the audio thread custodian of what the
/// view draws. The model belongs on the main thread; the audio thread follows it.
///
/// **Staying in step** costs nothing to arrange: the queue is in-order SPSC, a
/// [`Pattern`] is a deterministic `Copy` struct, and both sides mutate it through
/// the same [`apply_pattern_command`]. Feed one implementation the same commands
/// in the same order and it lands in the same place. The one way they could come
/// apart is a command *dropped* at [`QUEUE_CAP`], so the controller advances this
/// model only when the matching push succeeded — a dropped edit is then an edit
/// that did not happen, on both sides, rather than a silent divergence.
///
/// **Replacement** (a state restore) is not a delta and does not travel as one:
/// [`Self::set`] marks the lane, and [`EngineIo::flush_lane`] sends the whole
/// pattern down through the flush ring. See that method.
///
/// Main-thread only, like [`FlavourStore`] beside it: the `Mutex` is uncontended
/// and exists to keep [`EngineIo`] `Sync`, since the handle is cloned to the
/// audio thread too. The audio thread never touches it — that is what makes the
/// readback free of any RT consideration at all.
pub struct PatternStore {
    lanes: Mutex<Vec<Pattern>>,
    /// Lanes replaced wholesale since the view was last told (see [`Self::set`]).
    dirty: [AtomicBool; N_TRACKS],
    /// Out-of-band payloads for [`EngineCommand::LoadPattern`].
    flush: FlushRing,
}

/// Flush-ring capacity. A flush is a restore or an activation — seconds apart at
/// worst — so a handful of slots is ample, and the ring exists at all only so a
/// second flush cannot overwrite a `Pattern` the audio thread is still copying out.
const FLUSH_CAP: usize = 4;

/// SPSC ring of whole [`Pattern`]s, main → audio. Slots hold the pattern inline;
/// crossing it is a fixed-size `Copy`, never an allocation on either side.
struct FlushRing {
    slots: [UnsafeCell<Pattern>; FLUSH_CAP],
    head: AtomicU32, // producer (main)
    tail: AtomicU32, // consumer (audio)
}

// SAFETY: strict SPSC — `head` written only by the main thread, `tail` only by the
// audio thread, each slot handed off by the Acquire/Release pair on those indices.
// `Pattern` is `Copy + Send`.
unsafe impl Send for PatternStore {}
unsafe impl Sync for PatternStore {}

impl FlushRing {
    fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| UnsafeCell::new(Pattern::default())),
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
        }
    }

    fn push(&self, p: &Pattern) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) % FLUSH_CAP as u32;
        if next == self.tail.load(Ordering::Acquire) {
            return false; // full
        }
        // SAFETY: SPSC — only the producer writes this slot, and the consumer will
        // not read index `head` until the store below publishes it.
        unsafe { *self.slots[head as usize].get() = *p };
        self.head.store(next, Ordering::Release);
        true
    }

    fn pop(&self) -> Option<Pattern> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: SPSC — the producer published this slot via its `head` store
        // (Acquire-loaded above) and will not touch it again until we advance `tail`.
        let p = unsafe { *self.slots[tail as usize].get() };
        self.tail.store((tail + 1) % FLUSH_CAP as u32, Ordering::Release);
        Some(p)
    }
}

impl PatternStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            lanes: Mutex::new((0..N_TRACKS).map(|_| Pattern::default()).collect()),
            dirty: [const { AtomicBool::new(false) }; N_TRACKS],
            flush: FlushRing::new(),
        })
    }

    /// **Main thread:** apply a lane edit to the model. Returns `false` for a
    /// command that is not a lane edit (mix, macro, master), which the caller
    /// still queues for the engine — those live on `Track`, not on `Pattern`.
    pub fn apply(&self, cmd: EngineCommand) -> bool {
        let Some(track) = cmd.track() else {
            return false;
        };
        let Ok(mut g) = self.lanes.lock() else {
            return false;
        };
        match g.get_mut(track as usize) {
            Some(p) => apply_pattern_command(p, cmd),
            None => false,
        }
    }

    /// **Main thread:** a copy of a lane, for the view to draw or the shell to save.
    pub fn get(&self, track: usize) -> Pattern {
        self.lanes
            .lock()
            .ok()
            .and_then(|g| g.get(track).copied())
            .unwrap_or_default()
    }

    /// **Main thread:** every lane, in track order — what the faceplate is built from.
    pub fn snapshot(&self) -> Vec<Pattern> {
        self.lanes
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| (0..N_TRACKS).map(|_| Pattern::default()).collect())
    }

    /// **Main thread:** replace a lane wholesale (a state restore), marking it for
    /// the view. This is the only mutation that is *not* a delta, so it is the only
    /// one the view has to be told about out of band — every other change to the
    /// model came from the view in the first place.
    ///
    /// It does not itself send anything to the audio thread; pair it with
    /// [`EngineIo::flush_lane`], which is the downward half.
    pub fn set(&self, track: usize, pattern: Pattern) {
        if let Ok(mut g) = self.lanes.lock() {
            if let Some(slot) = g.get_mut(track) {
                *slot = pattern;
            }
        }
        if let Some(d) = self.dirty.get(track) {
            d.store(true, Ordering::Release);
        }
    }

    /// **Main thread:** has `track` been replaced since this was last asked? Clears
    /// the mark, so each replacement is announced to the view exactly once.
    pub fn take_dirty(&self, track: usize) -> bool {
        self.dirty.get(track).is_some_and(|d| d.swap(false, Ordering::Acquire))
    }

    /// **Audio thread:** the next flushed pattern, in the order they were sent.
    /// Matched to its [`EngineCommand::LoadPattern`] marker positionally — both are
    /// pushed in the same order by the same thread, and `flush_lane` never commits
    /// one without the other.
    pub fn pop_flush(&self) -> Option<Pattern> {
        self.flush.pop()
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
    /// The authoritative per-lane model — main-thread, the faceplate's source of
    /// truth and the engine's (0366).
    pub patterns: Arc<PatternStore>,
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
            patterns: PatternStore::new(),
            swaps: (0..N_TRACKS).map(|_| crate::swap::EngineSwap::new()).collect(),
            kinds: TrackKinds::new(),
            flavours: FlavourStore::new(),
        }
    }

    /// **Main thread:** send a lane of the model down to the audio thread whole —
    /// the *full flush*, for when the model was replaced rather than edited and a
    /// stream of deltas would be the wrong shape (and, at 64 hits a lane, would not
    /// fit through [`QUEUE_CAP`] anyway).
    ///
    /// Two channels, one order. The pattern goes out of band through the flush
    /// ring, because the edit queue's slots are words and a `Pattern` is kilobytes;
    /// an [`EngineCommand::LoadPattern`] marker goes through the queue, so the
    /// replacement lands at an exact point in the delta stream rather than racing
    /// whatever was already in flight.
    ///
    /// The queue slot is reserved *before* the ring is committed to, so a marker
    /// can never be dropped behind a pattern that was pushed — the two would then
    /// be off by one for the rest of the session. Returns `false` if either channel
    /// is full, having touched neither; the caller may retry on its next tick.
    pub fn flush_lane(&self, track: usize) -> bool {
        if track >= N_TRACKS || !self.edits.can_push() {
            return false;
        }
        if !self.patterns.flush.push(&self.patterns.get(track)) {
            return false;
        }
        // Guaranteed by the `can_push` above: main is the queue's only producer.
        self.edits.push(EngineCommand::LoadPattern { track: track as u8 })
    }

    /// **Main thread:** flush every lane. What a state restore does once it has
    /// loaded the model, and what an already-running engine needs to pick it up.
    /// Returns the number of lanes that got through.
    pub fn flush_all(&self) -> usize {
        (0..N_TRACKS).filter(|&t| self.flush_lane(t)).count()
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

    /// AC (0366): the model is the main thread's, and the engine's copy tracks it
    /// because both run the *same* apply over the same command stream.
    #[test]
    fn model_and_engine_copy_stay_in_step_under_the_same_commands() {
        let store = PatternStore::new();
        let mut engine_copy = Pattern::default();

        // Placed out of fire order, then dragged and quantised — every verb that
        // re-sorts the list, which is where two implementations would drift.
        let cmds = [
            EngineCommand::AddHit {
                track: 0, beat: 3, sub: 0, f: 0.0, nudge: 0, y: 0.5, note: 39.0, velocity: 1.0,
            },
            EngineCommand::AddHit {
                track: 0, beat: 1, sub: 2, f: 0.5, nudge: -7, y: 0.2, note: 36.0, velocity: 0.6,
            },
            EngineCommand::AddHit {
                track: 0, beat: 0, sub: 1, f: 0.25, nudge: 3, y: 0.8, note: 42.0, velocity: 0.9,
            },
            EngineCommand::SetGridSubs { track: 0, subs: 3 },
            EngineCommand::SetHitPosition {
                track: 0, hit: 1, beat: 2, sub: 1, f: 0.75, nudge: 0,
            },
            EngineCommand::QuantiseHitX { track: 0, hit: 2, amount: 1.0 },
            EngineCommand::SetHitProbability { track: 0, hit: 0, probability: 0.25 },
            // A colour is not a position, so it must not re-sort the list either.
            EngineCommand::SetHitColour { track: 0, hit: 2, rgb: [1.0, 0.0, 0.5] },
        ];
        for c in cmds {
            assert!(store.apply(c), "{c:?} is a lane edit");
            assert!(apply_pattern_command(&mut engine_copy, c));
        }
        assert_eq!(store.get(0).hits(), engine_copy.hits());
        assert_eq!(store.get(0).grid(), engine_copy.grid());
        assert_eq!(store.get(0).len(), 3);
        // Other lanes are untouched — the routing is per track.
        assert!(store.get(1).is_empty());
    }

    /// AC (0354): the marker verbs travel as deltas like every other edit, and the
    /// two rules stay opposite through the queue — a drag rubber-bands the hits in
    /// the slots either side, an insert leaves every hit exactly where it was.
    #[test]
    fn marker_commands_keep_drag_relative_and_insert_absolute() {
        let store = PatternStore::new();
        let mut engine_copy = Pattern::default();
        fn both(store: &PatternStore, copy: &mut Pattern, c: EngineCommand) {
            assert!(store.apply(c), "{c:?} is a lane edit");
            assert!(apply_pattern_command(copy, c));
        }
        // One hit welded to a marker, one half way through the slot after it.
        both(&store, &mut engine_copy, EngineCommand::AddHit {
            track: 0, beat: 1, sub: 0, f: 0.0, nudge: 0, y: 0.5, note: 36.0, velocity: 1.0,
        });
        both(&store, &mut engine_copy, EngineCommand::AddHit {
            track: 0, beat: 1, sub: 2, f: 0.5, nudge: 0, y: 0.5, note: 36.0, velocity: 1.0,
        });
        let welded = store.get(0).fire_beat(0);
        let placed = store.get(0).fire_beat(1);

        // Drag beat marker 1 late: both hits hang off slots the drag reshaped, so
        // both move — and not one hit record was written.
        both(&store, &mut engine_copy, EngineCommand::DragBeatMarker { track: 0, marker: 1, pos: 1.5 });
        let p = store.get(0);
        assert_eq!(p.grid().beat_marker(1), 1.5);
        assert!(p.fire_beat(0) > welded, "the welded hit rode its marker");
        assert!(p.fire_beat(1) > placed, "…and so did the one placed in the slot");
        assert_eq!(p.fire_beat(0), 1.5, "welded means welded: exactly on the marker");
        assert_eq!(p.hits(), engine_copy.hits(), "one implementation, two copies");

        // Insert a marker mid-slot: the opposite rule, so nothing moves. The index
        // is the one that *splits the slot the position is in* — `locate(pos).beat + 1`,
        // which is what the editor sends; naming any other index would reach the
        // clamp and reshape a slot the user did not point at.
        let (a, b) = (p.fire_beat(0), p.fire_beat(1));
        both(&store, &mut engine_copy, EngineCommand::InsertBeatMarker { track: 0, marker: 3, pos: 2.5, subs: 0 });
        let p = store.get(0);
        assert_eq!(p.grid().n_beats(), 5);
        assert_eq!(p.grid().beat_marker(3), 2.5);
        assert_eq!((p.fire_beat(0), p.fire_beat(1)), (a, b), "an insert moves nothing");
        // …and deleting it again is equally inert.
        both(&store, &mut engine_copy, EngineCommand::DeleteBeatMarker { track: 0, marker: 3 });
        let p = store.get(0);
        assert_eq!(p.grid().n_beats(), 4);
        assert_eq!((p.fire_beat(0), p.fire_beat(1)), (a, b), "a delete moves nothing");
        assert_eq!(p.hits(), engine_copy.hits());
    }

    /// AC (0354): a drag is a *request*. The editor never writes a marker position,
    /// so a command asking for one past a neighbour clamps at `MIN_SLOT` rather than
    /// producing a degenerate slot, and the pinned outer markers refuse it outright.
    #[test]
    fn a_marker_drag_command_clamps_instead_of_crossing() {
        let store = PatternStore::new();
        assert!(store.apply(EngineCommand::DragBeatMarker { track: 0, marker: 1, pos: 9.0 }));
        assert_eq!(store.get(0).grid().beat_marker(1), 2.0 - crate::grid::MIN_SLOT);
        assert!(store.apply(EngineCommand::DragBeatMarker { track: 0, marker: 1, pos: -5.0 }));
        assert_eq!(store.get(0).grid().beat_marker(1), crate::grid::MIN_SLOT);
        // The outer markers are the pattern bounds; a drag of one is inert.
        for (marker, expect) in [(0_u8, 0.0), (4, 4.0)] {
            assert!(store.apply(EngineCommand::DragBeatMarker { track: 0, marker, pos: 1.5 }));
            assert_eq!(store.get(0).grid().beat_marker(marker as usize), expect);
        }
    }

    /// AC (0354): swing is a geometry edit, and it takes the relative door — a hit at
    /// `f = 0` stays welded to its subdivision marker throughout a sweep, which is the
    /// behaviour the whole storage model exists to give.
    #[test]
    fn a_swing_sweep_keeps_welded_hits_on_their_markers() {
        let store = PatternStore::new();
        for sub in [1_u8, 2, 3] {
            assert!(store.apply(EngineCommand::AddHit {
                track: 0, beat: 0, sub, f: 0.0, nudge: 0, y: 0.5, note: 36.0, velocity: 1.0,
            }));
        }
        for amount in [0.0, 0.25, 0.5, 1.0, -0.6] {
            let swing = crate::grid::Swing::mpc(amount);
            assert!(store.apply(EngineCommand::SetSwing { track: 0, swing }));
            let p = store.get(0);
            for (i, h) in p.hits().iter().enumerate() {
                assert_eq!(
                    p.fire_beat(i),
                    p.grid().sub_pos(h.beat as usize, h.sub as u32),
                    "hit {i} came off its marker at swing {amount}"
                );
            }
        }
        // …and a per-beat override is the tuplet: three evenly spaced subdivisions
        // inside one beat of an otherwise-16ths lane.
        assert!(store.apply(EngineCommand::SetSwing {
            track: 0,
            swing: crate::grid::Swing::straight(),
        }));
        assert!(store.apply(EngineCommand::SetBeatSubs { track: 0, beat: 1, subs: 3 }));
        let g = *store.get(0).grid();
        assert_eq!(g.subs(1), 3);
        assert_eq!(g.sub_pos(1, 1), 1.0 + 1.0 / 3.0);
        assert_eq!(g.subs(0), 4, "only the beat named");
        // `0` clears the override rather than meaning "no subdivisions".
        assert!(store.apply(EngineCommand::SetBeatSubs { track: 0, beat: 1, subs: 0 }));
        assert_eq!(store.get(0).grid().sub_override(1), None);
    }

    /// AC (0355): the palette's verbs are lane edits like any other, and the raw
    /// channels reach the model untouched — black stays black, and "no colour" stays
    /// a different thing from it all the way to the macro vector.
    #[test]
    fn the_colour_verbs_carry_raw_channels_into_the_model() {
        let store = PatternStore::new();
        for beat in [0_u16, 1] {
            assert!(store.apply(EngineCommand::AddHit {
                track: 0, beat, sub: 0, f: 0.0, nudge: 0, y: 0.5, note: 36.0, velocity: 1.0,
            }));
        }
        // A fresh hit is uncoloured, which is not black: it overrides nothing.
        assert_eq!(crate::flavour::colour_override(store.get(0).hits()[0].rgb), None);

        assert!(store.apply(EngineCommand::SetHitColour {
            track: 0,
            hit: 0,
            rgb: [0.0, 0.0, 0.0],
        }));
        assert!(store.apply(EngineCommand::SetHitColour {
            track: 0,
            hit: 1,
            rgb: [1.0, 0.0, 0.5],
        }));
        let p = store.get(0);
        assert_eq!(p.hits()[0].rgb, [0.0, 0.0, 0.0], "black is stored as black");
        assert_eq!(crate::flavour::colour_override(p.hits()[0].rgb), Some([0.0; 3]));
        // Exactly the triple the arcs were dragged to — the three channels are
        // independent, so a value path that rounded or renormalised them would make
        // the middle arc unaddressable.
        assert_eq!(crate::flavour::colour_override(p.hits()[1].rgb), Some([1.0, 0.0, 0.5]));

        assert!(store.apply(EngineCommand::ClearHitColour { track: 0, hit: 0 }));
        assert_eq!(crate::flavour::colour_override(store.get(0).hits()[0].rgb), None);
        // Clearing one hit's colour leaves its neighbour's alone.
        assert_eq!(
            crate::flavour::colour_override(store.get(0).hits()[1].rgb),
            Some([1.0, 0.0, 0.5])
        );
    }

    /// Mix, macro and master verbs are not lane edits: they belong to `Track` and
    /// the master bus, and the model must not claim them.
    #[test]
    fn non_lane_commands_do_not_touch_the_model() {
        let store = PatternStore::new();
        for c in [
            EngineCommand::SetGain { track: 0, gain: 0.5 },
            EngineCommand::SetPan { track: 0, pan: -1.0 },
            EngineCommand::SetMacro { track: 0, slot: 1, value: 0.25 },
            EngineCommand::SetMute { track: 0, muted: true },
            EngineCommand::SetChokeGroup { track: 0, group: 2 },
            EngineCommand::SetSend { track: 0, amount: 0.5 },
            EngineCommand::SetMasterVolume { value: 0.5 },
            EngineCommand::SetDelayReturn { value: 0.5 },
        ] {
            assert!(!store.apply(c), "{c:?} must not be a lane edit");
        }
        assert!(store.get(0).is_empty());
    }

    /// Every per-track verb resolves a track, and only the master bus does not —
    /// the routing table both sides dispatch through.
    #[test]
    fn command_track_routing_covers_the_vocabulary() {
        assert_eq!(EngineCommand::RemoveHit { track: 5, hit: 0 }.track(), Some(5));
        assert_eq!(EngineCommand::LoadPattern { track: 7 }.track(), Some(7));
        assert_eq!(
            EngineCommand::SetHitColour { track: 3, hit: 0, rgb: [0.0; 3] }.track(),
            Some(3)
        );
        assert_eq!(EngineCommand::ClearHitColour { track: 4, hit: 0 }.track(), Some(4));
        assert_eq!(EngineCommand::SetMute { track: 2, muted: true }.track(), Some(2));
        assert_eq!(EngineCommand::SetMasterVolume { value: 1.0 }.track(), None);
        assert_eq!(EngineCommand::SetDelaySyncBeats { beats: 0.5 }.track(), None);
    }

    /// AC (0366): a replacement is announced to the view exactly once, and only a
    /// replacement is — an edit the view made needs no telling.
    #[test]
    fn replacing_a_lane_marks_it_for_the_view_once() {
        let store = PatternStore::new();
        assert!(!store.take_dirty(0));

        let mut p = Pattern::default();
        p.insert(crate::sequencer::Hit::at(2, 1));
        store.set(0, p);
        assert_eq!(store.get(0).len(), 1);
        assert!(store.take_dirty(0), "a replaced lane is announced");
        assert!(!store.take_dirty(0), "…and only once");
        assert!(!store.take_dirty(1), "…and only the lane replaced");

        // An ordinary edit is the view's own doing and is not re-announced.
        assert!(store.apply(EngineCommand::SetHitY { track: 0, hit: 0, y: 0.9 }));
        assert!(!store.take_dirty(0));
    }

    /// AC (0366): the full flush puts the whole pattern on one channel and its
    /// marker on the other, in that order, and never one without the other.
    #[test]
    fn flush_pairs_a_pattern_with_its_marker() {
        let io = EngineIo::new();
        let mut p = Pattern::default();
        p.insert(crate::sequencer::Hit::at(1, 1));
        p.insert(crate::sequencer::Hit::at(0, 0));
        io.patterns.set(4, p);

        assert!(io.flush_lane(4));
        // The marker rides the edit queue, so it lands in order with the deltas.
        assert_eq!(io.edits.pop(), Some(EngineCommand::LoadPattern { track: 4 }));
        // …and the payload is waiting out of band, whole.
        let flushed = io.patterns.pop_flush().expect("pattern on the flush ring");
        assert_eq!(flushed.hits(), io.patterns.get(4).hits());
        assert_eq!(flushed.len(), 2);
        assert!(io.patterns.pop_flush().is_none(), "one flush, one pattern");

        // Out-of-range lanes are inert rather than panicking.
        assert!(!io.flush_lane(N_TRACKS + 1));
    }

    /// A flush must not half-commit. With the edit queue full there is nowhere to
    /// put the marker, so the pattern is not pushed either — the ring and the
    /// marker stream would be off by one from then on.
    #[test]
    fn a_flush_that_cannot_mark_does_not_push_a_pattern() {
        let io = EngineIo::new();
        while io.edits.can_push() {
            assert!(io.edits.push(EngineCommand::SetGain { track: 0, gain: 1.0 }));
        }
        assert!(!io.flush_lane(0));
        assert!(io.patterns.pop_flush().is_none(), "nothing was committed to the ring");
    }

    /// The flush ring is bounded, and a full one refuses rather than overwriting a
    /// pattern the audio thread may still be copying out.
    #[test]
    fn flush_ring_is_bounded() {
        let io = EngineIo::new();
        let sent = io.flush_all();
        assert_eq!(sent, FLUSH_CAP - 1, "a ring of {FLUSH_CAP} holds one fewer in flight");
        for _ in 0..sent {
            assert!(io.patterns.pop_flush().is_some());
        }
        assert!(io.patterns.pop_flush().is_none());
    }
}
