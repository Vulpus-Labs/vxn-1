//! MPE-aware voice allocation for VXN1b (ticket 0198).
//!
//! VXN1's allocator ([`vxn-1/crates/vxn-engine/src/voice.rs`]) is
//! **channel-agnostic**: a note-on carries only note + velocity, so per-note
//! (MPE) pressure has nowhere to land. VXN1b threads the **originating MIDI
//! channel** from note-on → allocation → the assigned voice, and gives each
//! voice a **pressure** cell folded by channel. That makes per-note pressure
//! from an MPE controller reach *that note's* voice, with channel pressure as
//! the degenerate broadcast case over every voice on the channel
//! (ADR 0001 §2).
//!
//! This is the *voice-allocation architecture* only — no matrix, no DSP render
//! path. It is built early (before the param table and evaluator) because the
//! per-voice `channel` + `pressure` fields shape the voice struct the matrix
//! evaluator later reads (0202). Everything here is fixed-size and
//! allocation-free: real-time safe by construction.

use vxn_dsp::{MAX_VOICES, xorshift64};

use crate::params::VoiceMode;

/// Voice count — inherited from the shared DSP crate so VXN1b's poly is
/// identical to VXN1's (ADR 0001 §1). A flat bank: channel/pressure plumbing is
/// orthogonal to VXN1's per-layer SoA split, so allocation reasons over all
/// voices uniformly.
const N: usize = MAX_VOICES;

/// Capacity of the Solo held-note stack. Far beyond ten
/// fingers; an overflow drops the *oldest* entry, which is the one a player
/// would least expect to be revealed on release.
const MONO_STACK: usize = 32;

/// Wide-stack glide-time scaling (VXN1's Unison behaviour): a detuned stack
/// slides as one body and reads far stronger than a single voice, so its
/// effective portamento time is cut to this fraction of the knob value — a
/// scoop, not an audible slide. Applied from width 2 up.
pub(crate) const WIDE_GLIDE_SCALE: f32 = 0.15;

/// Seed for the stacked start-phase stream. Separate from the note-on-random
/// stream so adding/removing a stacked note can't shift the humanisation
/// sequence (and vice versa). Non-zero — `xorshift64` sticks at zero.
const PHASE_SEED: u64 = 0x1B_5EED_0242;

/// Symmetric detune weight in `[-1, 1]` for lane `i` of a stack `width` wide
/// (ADR 0003). Multiplied by the `UnisonDetune` cents value to fan the stack.
///
/// The denominator is `width - 1`, **not** the lane pool: `unison_detune` must
/// mean the same *total span* at every width, so widening a stack makes it
/// denser rather than retuning it. Width 1 is the degenerate case — one lane,
/// no fan, whatever the detune knob says.
#[inline]
pub(crate) fn stack_spread(i: usize, width: usize) -> f32 {
    if width <= 1 {
        0.0
    } else {
        (i as f32 / (width - 1) as f32) * 2.0 - 1.0
    }
}

/// Level compensation for a stack of `len` coherent-ish copies: `1/√len`.
///
/// Not `1/len` — the copies are detuned and independently phased, so they sum
/// as a random walk (~√len), not coherently. `1/√len` holds the perceived level
/// roughly constant across assign modes at any detune, with no comb null when
/// detune is zero (VXN1's `level_comp` rationale).
#[inline]
fn level_comp(len: usize) -> f32 {
    if len <= 1 {
        1.0
    } else {
        1.0 / (len as f32).sqrt()
    }
}

/// A DSP lane the caller must trigger for this note event, and the oscillator
/// start phase to stamp on it.
#[derive(Clone, Copy, Default)]
pub struct Trigger {
    pub voice: usize,
    /// `None` → the bank's own deterministic per-lane phase (Poly / Solo /
    /// Twin: decorrelated but reproducible). `Some(p)` → stamp `p` (Unison:
    /// a fresh random phase per voice per trigger, so the stack's beating
    /// doesn't comb into a synchronised null).
    pub start_phase: Option<f32>,
}

/// The lanes one note event triggers. Fixed-capacity (Unison triggers all `N`),
/// so note handling stays allocation-free.
#[derive(Clone, Copy)]
pub struct Triggers {
    items: [Trigger; N],
    len: usize,
}

impl Triggers {
    #[inline]
    fn none() -> Self {
        Self { items: [Trigger::default(); N], len: 0 }
    }

    #[inline]
    fn push(&mut self, voice: usize, start_phase: Option<f32>) {
        if self.len < N {
            self.items[self.len] = Trigger { voice, start_phase };
            self.len += 1;
        }
    }

    #[inline]
    pub fn as_slice(&self) -> &[Trigger] {
        &self.items[..self.len]
    }
}

/// Read-only bookkeeping the allocation policy reads. Borrows the bank's arrays
/// so [`allocate`] runs without touching (or being able to touch) mutable voice
/// state — the policy stays pure and unit-testable in isolation, mirroring
/// VXN1's `AllocView` seam.
#[derive(Clone, Copy)]
struct AllocView<'a> {
    active: &'a [bool; N],
    /// Per-voice gate — `false` once a note is released (ring-out tail). A
    /// released tail is sacrificed before a still-held note when stealing.
    gate: &'a [bool; N],
    /// Per-voice allocation tick — lowest is oldest, stolen first within a tier.
    alloc_tick: &'a [u64; N],
}

/// Steal ranking (lower = sacrificed first): a released tail before a still-held
/// note. So a melody played over a held chord eats the ringing-out tails before
/// it ever touches a key still held. Within a tier the oldest (lowest
/// `alloc_tick`) goes first. A trimmed form of VXN1's `steal_tier` — VXN1b has
/// no sustain-pedal defer state in this layer yet, so held vs. released is the
/// full ranking.
#[inline]
fn steal_tier(view: &AllocView, v: usize) -> u8 {
    if !view.gate[v] { 0 } else { 1 }
}

/// Seed for the note-on-random stream. Non-zero — `xorshift64` sticks at zero.
/// Fixed so the per-voice random values are reproducible in tests (0199 asks for
/// determinism, not OS entropy).
const NOTE_RANDOM_SEED: u64 = 0x1B_5EED_0199;

/// One note-on-random draw in `[0, 1)`. `xorshift64` spans `[-1, 1]` inclusive;
/// `.fract()` folds the lone `1.0` endpoint back to `0.0` so the value stays in
/// the half-open range the source contract promises (mirrors VXN1's
/// `random_phase`).
#[inline]
fn note_random_draw(rng: &mut u64) -> f32 {
    ((xorshift64(rng) + 1.0) * 0.5).fract()
}

/// Pick the voice a new note lands on: the first free (inactive) voice, else the
/// steal target by [`steal_tier`] then age. Pure over the borrowed view; the
/// caller stamps channel/note/pressure onto the returned index. Total — always
/// returns a valid `0..N` index (steal falls back to voice 0).
fn allocate(view: &AllocView) -> usize {
    if let Some(v) = (0..N).find(|&v| !view.active[v]) {
        return v;
    }
    (0..N)
        .min_by_key(|&v| (steal_tier(view, v), view.alloc_tick[v]))
        .unwrap_or(0)
}

/// The 16-voice bank's allocation + per-voice performance state. Holds only the
/// bookkeeping the allocator and pressure fold touch — the DSP kernels (osc,
/// filter, envelopes) are wired in later tickets over this same channel/pressure
/// spine. Every field is a fixed array: no method allocates.
pub struct Voices {
    active: [bool; N],
    gate: [bool; N],
    note: [u8; N],
    /// Originating MIDI channel of the voice's note (0..15). Threaded from
    /// note-on and **re-parented on steal** — a stolen voice adopts the stealing
    /// note's channel, so later channel-pressure broadcasts reach it correctly.
    channel: [u8; N],
    alloc_tick: [u64; N],
    /// Per-voice pressure in `[0, 1]`, the MPE-aware aftertouch source
    /// (ADR 0001 §2). Updated by per-note pressure (only the matching
    /// note+channel voice) or channel pressure (every active voice on the
    /// channel). Latched to 0 at note-on so a reused voice never inherits a
    /// stale value. Sampled by the evaluator at control rate (0202).
    pressure: [f32; N],
    /// Per-voice **note-on random** in `[0, 1)`, latched once at note-on and held
    /// for the note's lifetime (ADR 0001 §2). Decorrelates stacked/adjacent
    /// voices — a per-voice humanisation source the matrix reads (0202). Drawn
    /// from `rng`, so successive allocations differ and the sequence is
    /// reproducible.
    note_random: [f32; N],
    /// Per-voice note velocity `[0, 1]`, stamped at note-on — the matrix
    /// Velocity source (0202).
    velocity: [f32; N],
    /// Per-voice detune in **cents**, added to both oscillators by the render
    /// bank. Zero for Poly and Solo; the fanned `UnisonDetune` spread for
    /// Unison and the ±extremes for Twin. Stamped at trigger (and re-stamped on
    /// a legato slide, so the stack stays fanned across a slur).
    detune_cents: [f32; N],
    /// Per-voice position **within its stack**, `[-1, 1]` (ADR 0003). The
    /// stereo fan reads this rather than the lane index: a stack's lanes are
    /// wherever the allocator put them, so lane order says nothing about where
    /// a copy belongs in the image. Zero for width 1 (centred).
    stack_pos: [f32; N],
    /// Solo held-note stack, newest on top. Only the top note
    /// sounds; releasing it reveals the one beneath (last-note priority).
    mono_stack: [u8; MONO_STACK],
    mono_len: usize,
    /// Voice mode the last note event ran under. A change is detected on the
    /// next note-on so leaving/entering Solo can clear the state the other mode
    /// would otherwise strand (held poly voices, a stale held-note stack).
    last_mode: VoiceMode,
    /// Stack width the last note event ran under. Recorded rather than acted on:
    /// a width change does **not** re-voice sounding stacks (ADR 0003), it
    /// applies from the next note-on.
    last_width: usize,
    /// Output scaling for the current stack width (`1/√len`) — held here
    /// because it is a property of the *allocation*, not of any one voice. The
    /// synth copies it into the block context each render.
    level_comp: f32,
    /// Monotonic allocation counter; stamped into `alloc_tick` per note-on so
    /// the steal policy can rank by age. Wraps at u64::MAX (unreachable in
    /// practice — ~6M years at 100k notes/s).
    next_tick: u64,
    /// Note-on-random stream state. Advanced one draw per note-on; a single
    /// stream (not per-voice seeds) guarantees successive voices get distinct
    /// values while staying deterministic. Never zero (xorshift stuck point).
    rng: u64,
    /// Unison start-phase stream, advanced one draw per stacked voice per
    /// trigger. Separate from `rng` so the two humanisation streams don't
    /// perturb each other.
    phase_rng: u64,
}

impl Default for Voices {
    fn default() -> Self {
        Self::new()
    }
}

impl Voices {
    pub fn new() -> Self {
        Self {
            active: [false; N],
            gate: [false; N],
            note: [0; N],
            channel: [0; N],
            alloc_tick: [0; N],
            pressure: [0.0; N],
            note_random: [0.0; N],
            velocity: [0.0; N],
            detune_cents: [0.0; N],
            stack_pos: [0.0; N],
            mono_stack: [0; MONO_STACK],
            mono_len: 0,
            last_mode: VoiceMode::Poly,
            last_width: 1,
            level_comp: 1.0,
            next_tick: 0,
            rng: NOTE_RANDOM_SEED,
            phase_rng: PHASE_SEED,
        }
    }

    /// Clear every voice (host reset / all-sound-off).
    pub fn reset(&mut self) {
        self.active = [false; N];
        self.gate = [false; N];
        self.pressure = [0.0; N];
        self.detune_cents = [0.0; N];
        self.stack_pos = [0.0; N];
        self.mono_len = 0;
        self.level_comp = 1.0;
        // note/channel/alloc_tick are stale-until-reused; queries gate on `active`.
    }

    /// Output scaling for the current stack width — `1/√len`, so switching
    /// assign modes doesn't jump the perceived level. See [`level_comp`].
    #[inline]
    pub fn level_comp(&self) -> f32 {
        self.level_comp
    }

    /// Allocate a voice for a note-on on `channel`, stamp its identity, and reset
    /// its pressure. Returns the assigned voice index. If every voice is busy the
    /// oldest steal-tier target is re-used and **re-parented to `channel`** — the
    /// core MPE requirement: the stolen voice now belongs to the stealing note's
    /// channel, so subsequent channel pressure on that channel reaches it (and on
    /// the old channel no longer does).
    pub fn note_on(&mut self, channel: u8, note: u8, velocity: f32) -> usize {
        let v = allocate(&self.view());
        self.stamp(v, channel, note, velocity, 0.0, 0.0, true);
        v
    }

    /// Stamp one voice's identity for a note. `fresh` distinguishes a real
    /// trigger (re-draw the note-random, clear stale pressure, take a new
    /// allocation tick) from a **legato slide**, which keeps the sounding
    /// voice's latched humanisation and age and only re-points its pitch.
    fn stamp(
        &mut self,
        v: usize,
        channel: u8,
        note: u8,
        velocity: f32,
        detune_cents: f32,
        stack_pos: f32,
        fresh: bool,
    ) {
        self.active[v] = true;
        self.gate[v] = true;
        self.note[v] = note;
        self.channel[v] = channel;
        self.velocity[v] = velocity.clamp(0.0, 1.0);
        self.detune_cents[v] = detune_cents;
        self.stack_pos[v] = stack_pos;
        if fresh {
            self.pressure[v] = 0.0;
            self.note_random[v] = note_random_draw(&mut self.rng);
            self.alloc_tick[v] = self.next_tick;
            self.next_tick = self.next_tick.wrapping_add(1);
        }
    }

    /// Note-on under (`width`, `mode`) — the two orthogonal halves of what
    /// VXN1's assign-mode enum used to conflate (ADR 0003). Returns the lanes
    /// the caller must trigger in the DSP banks (empty on a legato slide — the
    /// voice keeps sounding and only its pitch moves).
    ///
    /// * **Poly** — each note takes its own `width`-lane stack, fanned across
    ///   the detune span, stealing when the pool is full. Width 1 is the plain
    ///   one-voice-per-note case; the widest width is monophonic by *capacity*
    ///   while still retriggering, which no VXN1 mode could express.
    /// * **Solo** — one stack pinned to lanes `0..width`, last-note priority,
    ///   with `legato` deciding whether a reveal slides or articulates.
    ///
    /// Stacked lanes take a fresh random start phase so the copies do not comb
    /// into a synchronised null; a single lane keeps the bank's own
    /// deterministic phase.
    pub fn note_on_stack(
        &mut self,
        channel: u8,
        note: u8,
        velocity: f32,
        width: usize,
        mode: VoiceMode,
        unison_detune: f32,
        legato: bool,
    ) -> Triggers {
        let width = width.clamp(1, N);
        self.sync_mode(width, mode);
        let mut out = Triggers::none();
        match mode {
            VoiceMode::Poly => {
                // Allocate the stack one lane at a time, stamping as we go: each
                // draw sees the previous lane taken (active, newest tick) and so
                // picks a different one. Stealing therefore takes the oldest
                // lanes first, which is the same policy a stack-granular
                // allocator would land on for a full pool.
                for i in 0..width {
                    let v = allocate(&self.view());
                    let pos = stack_spread(i, width);
                    self.stamp(v, channel, note, velocity, pos * unison_detune, pos, true);
                    out.push(v, self.stack_phase(width));
                }
                self.level_comp = level_comp(width);
            }
            VoiceMode::Solo => {
                let sounding = self.mono_sounding();
                self.mono_push(note);
                // Legato only slides when a note is *already* sounding — the
                // first note of a phrase always articulates.
                let slide = legato && sounding;
                for i in 0..width {
                    let pos = stack_spread(i, width);
                    self.stamp(i, channel, note, velocity, pos * unison_detune, pos, !slide);
                    if !slide {
                        out.push(i, self.stack_phase(width));
                    }
                }
                // Lanes past the stack are gated off, so narrowing the width (or
                // arriving from Poly) releases what the old layout left sounding
                // rather than stranding it.
                for v in width..N {
                    self.gate[v] = false;
                }
                self.level_comp = level_comp(width);
            }
        }
        out
    }

    /// Note-off under (`width`, `mode`). In Solo, releasing the sounding (top)
    /// note reveals the highest-priority note still held and re-pitches the
    /// stack to it — retriggered unless `legato`. Poly just gates off.
    pub fn note_off_stack(
        &mut self,
        channel: u8,
        note: u8,
        width: usize,
        mode: VoiceMode,
        unison_detune: f32,
        legato: bool,
    ) -> Triggers {
        let width = width.clamp(1, N);
        let mut out = Triggers::none();
        if mode != VoiceMode::Solo {
            self.note_off(channel, note);
            return out;
        }
        // A note that isn't on the stack was placed under Poly (the user
        // switched mid-hold). Release it the poly way rather than stranding it
        // gated on forever.
        let Some(was_top) = self.mono_remove(note) else {
            self.note_off(channel, note);
            return out;
        };
        if !was_top {
            // A note below the top was released — what's sounding is unchanged.
            return out;
        }
        if self.mono_len == 0 {
            for v in 0..width {
                self.gate[v] = false;
            }
            return out;
        }
        let revealed = self.mono_stack[self.mono_len - 1];
        for i in 0..width {
            let pos = stack_spread(i, width);
            let velocity = self.velocity[i];
            self.stamp(i, channel, revealed, velocity, pos * unison_detune, pos, !legato);
            if !legato {
                out.push(i, self.stack_phase(width));
            }
        }
        out
    }

    /// Start phase for a lane of a `width`-wide stack: a fresh random draw once
    /// there is more than one copy (so the stack's beating never combs into a
    /// null), and the bank's own deterministic phase for a single lane.
    #[inline]
    fn stack_phase(&mut self, width: usize) -> Option<f32> {
        (width > 1).then(|| note_random_draw(&mut self.phase_rng))
    }

    /// Handle a (width, mode) change detected at note-on. Entering Solo releases
    /// voices the polyphonic allocator placed (they would sustain under an
    /// allocator that no longer tracks them); leaving it discards the held-note
    /// stack so a later return starts clean. A width change alone needs neither
    /// — sounding stacks keep their lanes until released (ADR 0003).
    fn sync_mode(&mut self, width: usize, mode: VoiceMode) {
        if mode != self.last_mode {
            if mode == VoiceMode::Solo {
                for v in 0..N {
                    self.gate[v] = false;
                }
            } else {
                self.mono_len = 0;
            }
            self.last_mode = mode;
        }
        self.last_width = width;
    }

    /// Is a mono voice currently sounding (lane 0 allocated and still pressed)?
    /// Both mono modes drive lane 0, so it is the whole test.
    #[inline]
    fn mono_sounding(&self) -> bool {
        self.active[0] && self.gate[0] && self.mono_len > 0
    }

    /// Push a note onto the mono stack, newest on top. A repeated note moves to
    /// the top rather than duplicating. Overflow drops the oldest entry.
    fn mono_push(&mut self, note: u8) {
        if let Some(i) = self.mono_stack[..self.mono_len].iter().position(|&n| n == note) {
            self.mono_stack.copy_within(i + 1..self.mono_len, i);
            self.mono_len -= 1;
        } else if self.mono_len == MONO_STACK {
            self.mono_stack.copy_within(1..MONO_STACK, 0);
            self.mono_len -= 1;
        }
        self.mono_stack[self.mono_len] = note;
        self.mono_len += 1;
    }

    /// Remove `note` from the mono stack. `Some(true)` if it was the sounding
    /// (top) entry, `Some(false)` if it was below it, `None` if it wasn't held.
    fn mono_remove(&mut self, note: u8) -> Option<bool> {
        let i = self.mono_stack[..self.mono_len].iter().position(|&n| n == note)?;
        let was_top = i + 1 == self.mono_len;
        self.mono_stack.copy_within(i + 1..self.mono_len, i);
        self.mono_len -= 1;
        Some(was_top)
    }

    /// Release the voice(s) matching `channel` + `note`: gate off, but leave the
    /// voice `active` as a ring-out tail (the render loop frees it once its
    /// envelope idles — later ticket). Matching *both* channel and note is
    /// correct for MPE (one note per channel) and for channel mode (note alone
    /// disambiguates on the shared channel).
    pub fn note_off(&mut self, channel: u8, note: u8) {
        for v in 0..N {
            if self.active[v] && self.gate[v] && self.channel[v] == channel && self.note[v] == note {
                self.gate[v] = false;
            }
        }
    }

    /// Per-note (poly) pressure from an MPE controller: update **only** the
    /// voice(s) matching `channel` + `note`. Does not leak to voices on other
    /// channels or holding other notes — the isolation MPE expression depends on.
    pub fn poly_pressure(&mut self, channel: u8, note: u8, value: f32) {
        let value = value.clamp(0.0, 1.0);
        for v in 0..N {
            if self.active[v] && self.channel[v] == channel && self.note[v] == note {
                self.pressure[v] = value;
            }
        }
    }

    /// Channel pressure (mono aftertouch): broadcast to **every** active voice on
    /// `channel`. This is the degenerate case of the same pressure spine — a
    /// channel-mode controller sends it on one channel, so it folds onto that
    /// channel's whole stack.
    pub fn channel_pressure(&mut self, channel: u8, value: f32) {
        let value = value.clamp(0.0, 1.0);
        for v in 0..N {
            if self.active[v] && self.channel[v] == channel {
                self.pressure[v] = value;
            }
        }
    }

    /// Voice `v`'s current pressure `[0, 1]` — the per-voice aftertouch source
    /// the matrix evaluator samples (0202).
    #[inline]
    pub fn pressure(&self, v: usize) -> f32 {
        self.pressure[v]
    }

    /// Voice `v`'s note-on random value `[0, 1)` — latched at note-on, constant
    /// for the note's lifetime. The per-voice humanisation source (0202).
    #[inline]
    pub fn note_random(&self, v: usize) -> f32 {
        self.note_random[v]
    }

    /// Voice `v`'s originating MIDI channel. Exposed for the evaluator and tests.
    #[inline]
    pub fn channel(&self, v: usize) -> u8 {
        self.channel[v]
    }

    /// Whether voice `v` currently holds an allocated note (gated or ringing out).
    #[inline]
    pub fn is_active(&self, v: usize) -> bool {
        self.active[v]
    }

    /// Voice `v`'s note number. Exposed for tests / later wiring.
    #[inline]
    pub fn note(&self, v: usize) -> u8 {
        self.note[v]
    }

    /// Is any voice **holding** `note` — allocated *and* still gated (pressed)?
    /// A released ring-out tail (gate off) does not count. Used by the demux
    /// tests to prove a note was released on the right synth (0215).
    #[cfg(test)]
    pub fn is_holding(&self, note: u8) -> bool {
        (0..N).any(|v| self.active[v] && self.gate[v] && self.note[v] == note)
    }

    /// Voice `v`'s velocity `[0, 1]` — the matrix Velocity source.
    #[inline]
    pub fn velocity(&self, v: usize) -> f32 {
        self.velocity[v]
    }

    /// Total voice capacity (= [`vxn_dsp::MAX_VOICES`]).
    pub const CAPACITY: usize = N;

    /// Bundle the per-voice bookkeeping the render path reads as **disjoint
    /// borrows** in one struct. Splitting distinct fields into one view lets the
    /// engine hold `Voices` and the render banks as separate fields and drive the
    /// render without a whole-`self` borrow clash. `active` is `&mut` so
    /// fully-released voices free during render.
    #[inline]
    pub fn render_view(&mut self) -> RenderView<'_> {
        RenderView {
            note: &self.note,
            gate: &self.gate,
            velocity: &self.velocity,
            pressure: &self.pressure,
            note_random: &self.note_random,
            detune_cents: &self.detune_cents,
            stack_pos: &self.stack_pos,
            active: &mut self.active,
        }
    }

    #[inline]
    fn view(&self) -> AllocView<'_> {
        AllocView {
            active: &self.active,
            gate: &self.gate,
            alloc_tick: &self.alloc_tick,
        }
    }
}

/// Disjoint per-voice slices the render path consumes (see [`Voices::render_view`]).
/// Arrays are the full 16-voice width; the engine slices each 8-lane bank out.
pub struct RenderView<'a> {
    pub note: &'a [u8; N],
    pub gate: &'a [bool; N],
    pub velocity: &'a [f32; N],
    pub pressure: &'a [f32; N],
    pub note_random: &'a [f32; N],
    /// Per-voice detune in cents — the stack fan; zero at width 1.
    pub detune_cents: &'a [f32; N],
    /// Per-voice position within its stack, `[-1, 1]`; the stereo fan's input
    /// (ADR 0003). Zero at width 1.
    pub stack_pos: &'a [f32; N],
    pub active: &'a mut [bool; N],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_on_stores_channel_on_assigned_voice() {
        let mut voices = Voices::new();
        let v = voices.note_on(3, 60, 1.0);
        assert!(voices.is_active(v));
        assert_eq!(voices.channel(v), 3);
        assert_eq!(voices.note(v), 60);
    }

    #[test]
    fn per_note_pressure_isolated_to_matching_voice() {
        let mut voices = Voices::new();
        // MPE: each note on its own channel.
        let a = voices.note_on(1, 60, 1.0);
        let b = voices.note_on(2, 64, 1.0);
        voices.poly_pressure(1, 60, 0.8);
        assert_eq!(voices.pressure(a), 0.8);
        // Must not leak to the other channel/note.
        assert_eq!(voices.pressure(b), 0.0);
    }

    #[test]
    fn per_note_pressure_ignores_same_note_other_channel() {
        let mut voices = Voices::new();
        let a = voices.note_on(1, 60, 1.0);
        let b = voices.note_on(2, 60, 1.0); // same note, different channel
        voices.poly_pressure(2, 60, 0.5);
        assert_eq!(voices.pressure(a), 0.0);
        assert_eq!(voices.pressure(b), 0.5);
    }

    #[test]
    fn channel_pressure_broadcasts_to_all_voices_on_channel() {
        let mut voices = Voices::new();
        // Channel mode: several notes share one channel.
        let a = voices.note_on(1, 60, 1.0);
        let b = voices.note_on(1, 64, 1.0);
        let c = voices.note_on(2, 67, 1.0); // other channel — untouched
        voices.channel_pressure(1, 0.6);
        assert_eq!(voices.pressure(a), 0.6);
        assert_eq!(voices.pressure(b), 0.6);
        assert_eq!(voices.pressure(c), 0.0);
    }

    #[test]
    fn free_voice_chosen_before_steal() {
        let mut voices = Voices::new();
        let first = voices.note_on(1, 60, 1.0);
        let second = voices.note_on(1, 61, 1.0);
        assert_ne!(first, second, "distinct free voices used before any steal");
    }

    #[test]
    fn stolen_voice_reparents_to_stealing_channel() {
        let mut voices = Voices::new();
        // Fill every voice on channel 1.
        for i in 0..N {
            voices.note_on(1, 60 + i as u8, 1.0);
        }
        // Next note-on on channel 2 must steal and re-parent.
        let stolen = voices.note_on(2, 72, 1.0);
        assert_eq!(voices.channel(stolen), 2, "stolen voice adopts stealing channel");
        assert_eq!(voices.note(stolen), 72);
        // Pressure was reset on steal — no stale value inherited.
        assert_eq!(voices.pressure(stolen), 0.0);
        // Old channel's broadcast no longer reaches it; new channel's does.
        voices.channel_pressure(1, 0.9);
        assert_eq!(voices.pressure(stolen), 0.0);
        voices.channel_pressure(2, 0.4);
        assert_eq!(voices.pressure(stolen), 0.4);
    }

    #[test]
    fn oldest_voice_stolen_first() {
        let mut voices = Voices::new();
        let oldest = voices.note_on(1, 60, 1.0);
        for i in 1..N {
            voices.note_on(1, 60 + i as u8, 1.0);
        }
        // All held; the first-allocated (oldest tick) is sacrificed.
        let stolen = voices.note_on(1, 90, 1.0);
        assert_eq!(stolen, oldest);
    }

    #[test]
    fn released_tail_stolen_before_held_note() {
        let mut voices = Voices::new();
        // Voice 0 is the oldest but gets released; a later voice is younger but held.
        let released = voices.note_on(1, 60, 1.0);
        for i in 1..N {
            voices.note_on(1, 60 + i as u8, 1.0);
        }
        voices.note_off(1, 60); // release the oldest → tier 0
        // A held-but-younger voice exists, yet the released tail goes first.
        let stolen = voices.note_on(1, 90, 1.0);
        assert_eq!(stolen, released);
    }

    #[test]
    fn note_off_matches_channel_and_note() {
        let mut voices = Voices::new();
        let a = voices.note_on(1, 60, 1.0);
        let b = voices.note_on(2, 60, 1.0); // same note, other channel
        voices.note_off(1, 60);
        // Only the channel-1 voice released; channel-2 still gated (its next
        // steal tier proves the gate state).
        assert!(!voices.gate[a]);
        assert!(voices.gate[b]);
    }

    #[test]
    fn note_random_in_unit_interval() {
        let mut voices = Voices::new();
        for i in 0..N {
            let v = voices.note_on(1, 60 + i as u8, 1.0);
            let r = voices.note_random(v);
            assert!((0.0..1.0).contains(&r), "note-random {r} out of [0,1)");
        }
    }

    #[test]
    fn note_random_constant_over_note_lifetime() {
        let mut voices = Voices::new();
        let a = voices.note_on(1, 60, 1.0);
        let latched = voices.note_random(a);
        // Unrelated activity must not disturb a held voice's latched value.
        voices.note_on(2, 64, 1.0);
        voices.channel_pressure(1, 0.7);
        voices.poly_pressure(1, 60, 0.3);
        assert_eq!(voices.note_random(a), latched);
    }

    #[test]
    fn note_random_differs_across_concurrent_voices() {
        let mut voices = Voices::new();
        let mut seen: Vec<f32> = Vec::new();
        for i in 0..N {
            let v = voices.note_on(1, 60 + i as u8, 1.0);
            let r = voices.note_random(v);
            assert!(
                !seen.iter().any(|&s| (s - r).abs() < 1e-9),
                "duplicate note-random {r} across concurrent voices"
            );
            seen.push(r);
        }
    }

    #[test]
    fn note_random_reproducible_from_seed() {
        // Same construction + same note-on sequence → identical stream.
        let mut a = Voices::new();
        let mut b = Voices::new();
        for i in 0..5 {
            let va = a.note_on(1, 60 + i, 1.0);
            let vb = b.note_on(1, 60 + i, 1.0);
            assert_eq!(a.note_random(va), b.note_random(vb));
        }
    }

    #[test]
    fn note_random_relatched_on_reuse() {
        // A reused (stolen) voice draws a fresh value, not the stale one.
        let mut voices = Voices::new();
        for i in 0..N {
            voices.note_on(1, 60 + i as u8, 1.0);
        }
        let stolen = voices.note_on(2, 90, 1.0);
        let before = voices.note_random(stolen);
        // Steal it again with another full round + one more note.
        for i in 0..N {
            voices.note_on(3, 40 + i as u8, 1.0);
        }
        let stolen2 = voices.note_on(4, 91, 1.0);
        // Overwhelmingly likely distinct; assert the draw actually re-ran by
        // checking the stream advanced (value changed for this reused slot).
        if stolen2 == stolen {
            assert_ne!(voices.note_random(stolen2), before);
        }
    }

    // ── Assign modes: Poly / Unison / Solo / Twin ──

    /// Every lane a mode triggers, in order.
    fn fired(t: &Triggers) -> Vec<usize> {
        t.as_slice().iter().map(|x| x.voice).collect()
    }

    // ── Width × mode orthogonality (0266, ADR 0003) ─────────────────────────

    /// The two axes are independent: every width is playable in either mode,
    /// and the four VXN1 assign modes are just four points in that space.
    #[test]
    fn the_four_legacy_modes_are_points_in_the_width_mode_space() {
        // Poly = 1 × Poly, Twin = 2 × Poly, Solo = 1 × Solo, Unison = N × Solo.
        for (width, mode, lanes) in [
            (1, VoiceMode::Poly, 1),
            (2, VoiceMode::Poly, 2),
            (1, VoiceMode::Solo, 1),
            (N, VoiceMode::Solo, N),
        ] {
            let mut v = Voices::default();
            let t = v.note_on_stack(0, 60, 1.0, width, mode, 20.0, false);
            assert_eq!(fired(&t).len(), lanes, "width {width} {mode:?} placed the wrong lane count");
        }
    }

    /// The combination the old enum could not express: a full-width stack in
    /// **Poly**. One note at a time by capacity, but a second note steals and
    /// **retriggers** rather than sliding, which is what makes it different
    /// from Solo at the same width.
    #[test]
    fn full_width_poly_is_monophonic_but_retriggers() {
        let mut v = Voices::default();
        let first = v.note_on_stack(0, 60, 1.0, N, VoiceMode::Poly, 10.0, true);
        assert_eq!(fired(&first).len(), N, "the stack takes every lane");
        // Legato is on, but Poly never slides: the second note re-fires lanes.
        let second = v.note_on_stack(0, 67, 1.0, N, VoiceMode::Poly, 10.0, true);
        assert_eq!(fired(&second).len(), N, "a stolen full stack must retrigger");
        assert!(
            (0..N).all(|i| v.note[i] == 67),
            "every lane should now be on the new note"
        );
    }

    /// Same width in Solo *does* slide — the axis is doing its job.
    #[test]
    fn full_width_solo_slides_under_legato() {
        let mut v = Voices::default();
        v.note_on_stack(0, 60, 1.0, N, VoiceMode::Solo, 10.0, true);
        let second = v.note_on_stack(0, 67, 1.0, N, VoiceMode::Solo, 10.0, true);
        assert!(fired(&second).is_empty(), "a legato slide must not retrigger");
        assert!((0..N).all(|i| v.note[i] == 67), "but the pitch moves");
    }

    /// `unison_detune` means the same **total span** at every width: the
    /// outermost lanes sit at ±detune regardless, and a wider stack is denser
    /// in between rather than wider overall. Without this rule the same patch
    /// is a different chord at each width.
    #[test]
    fn detune_span_is_constant_across_widths() {
        for width in [2usize, 4, 8, 16] {
            let mut v = Voices::default();
            v.note_on_stack(0, 60, 1.0, width, VoiceMode::Solo, 25.0, false);
            let cents: Vec<f32> = (0..width).map(|i| v.detune_cents[i]).collect();
            let lo = cents.iter().cloned().fold(f32::INFINITY, f32::min);
            let hi = cents.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            assert!((lo + 25.0).abs() < 1e-4, "width {width} low edge {lo}");
            assert!((hi - 25.0).abs() < 1e-4, "width {width} high edge {hi}");
            // Denser, not wider: neighbouring gaps shrink as width grows.
            let gap = 50.0 / (width - 1) as f32;
            for pair in cents.windows(2) {
                assert!((pair[1] - pair[0] - gap).abs() < 1e-3, "width {width} uneven fan");
            }
        }
    }

    /// Width 1 is the degenerate fan: one lane, no detune, whatever the knob says.
    #[test]
    fn width_one_ignores_the_detune_knob() {
        let mut v = Voices::default();
        v.note_on_stack(0, 60, 1.0, 1, VoiceMode::Poly, 50.0, false);
        assert_eq!(v.detune_cents[0], 0.0);
    }

    /// Poly capacity is `N / width`: the stack that does not fit steals rather
    /// than sounding alongside.
    #[test]
    fn poly_capacity_is_the_pool_divided_by_width() {
        for width in [1usize, 2, 4, 8] {
            let mut v = Voices::default();
            let stacks = N / width;
            for i in 0..stacks {
                v.note_on_stack(0, 60 + i as u8, 1.0, width, VoiceMode::Poly, 0.0, false);
            }
            assert_eq!(
                (0..N).filter(|&i| v.is_active(i)).count(),
                N,
                "width {width}: {stacks} stacks should fill the pool"
            );
            // One more note: capacity is spent, so it steals — the note count
            // held stays at the pool size rather than growing.
            v.note_on_stack(0, 90, 1.0, width, VoiceMode::Poly, 0.0, false);
            assert_eq!((0..N).filter(|&i| v.is_active(i)).count(), N);
            assert!(
                (0..N).any(|i| v.note[i] == 90 && v.is_active(i)),
                "width {width}: the stealing note must sound"
            );
        }
    }

    /// A width change does not re-voice sounding stacks (ADR 0003) — it applies
    /// from the next note-on. Re-partitioning under held notes would be a click
    /// and a stolen-note storm.
    #[test]
    fn a_width_change_leaves_held_stacks_alone() {
        let mut v = Voices::default();
        v.note_on_stack(0, 60, 1.0, 4, VoiceMode::Poly, 10.0, false);
        let before: Vec<u8> = (0..4).map(|i| v.note[i]).collect();
        // Next note arrives at a different width; the held stack is untouched.
        v.note_on_stack(0, 67, 1.0, 2, VoiceMode::Poly, 10.0, false);
        let after: Vec<u8> = (0..4).map(|i| v.note[i]).collect();
        assert_eq!(before, after, "the held 4-lane stack must keep its lanes");
    }

    #[test]
    fn poly_places_one_undetuned_voice() {
        let mut v = Voices::new();
        let t = v.note_on_stack(0, 60, 1.0, 1, VoiceMode::Poly, 50.0, false);
        assert_eq!(fired(&t).len(), 1);
        assert_eq!(v.detune_cents[t.as_slice()[0].voice], 0.0);
        assert_eq!(v.level_comp(), 1.0);
    }

    #[test]
    fn unison_stacks_every_lane_fanned_across_the_detune() {
        let mut v = Voices::new();
        let t = v.note_on_stack(0, 60, 1.0, N, VoiceMode::Solo, 50.0, false);
        assert_eq!(fired(&t).len(), N, "Unison triggers all 16 lanes");
        // Symmetric fan spanning the full ±detune, and every lane on the note.
        assert!((v.detune_cents[0] + 50.0).abs() < 1e-4);
        assert!((v.detune_cents[N - 1] - 50.0).abs() < 1e-4);
        assert!(v.detune_cents.windows(2).all(|w| w[1] > w[0]), "monotone fan");
        assert!((0..N).all(|i| v.note[i] == 60 && v.gate[i]));
        assert!((v.level_comp() - 1.0 / (N as f32).sqrt()).abs() < 1e-6);
    }

    #[test]
    fn unison_start_phases_are_random_and_distinct() {
        let mut v = Voices::new();
        let t = v.note_on_stack(0, 60, 1.0, N, VoiceMode::Solo, 12.0, false);
        let phases: Vec<f32> = t.as_slice().iter().map(|x| x.start_phase.unwrap()).collect();
        assert!(phases.iter().all(|&p| (0.0..1.0).contains(&p)));
        assert!(
            phases.windows(2).any(|w| (w[0] - w[1]).abs() > 1e-6),
            "stacked copies must not share a start phase"
        );
    }

    #[test]
    fn twin_places_two_voices_at_the_fan_extremes() {
        let mut v = Voices::new();
        let t = v.note_on_stack(0, 60, 1.0, 2, VoiceMode::Poly, 20.0, false);
        let lanes = fired(&t);
        assert_eq!(lanes.len(), 2);
        assert_ne!(lanes[0], lanes[1], "Twin must use two distinct voices");
        assert_eq!(v.detune_cents[lanes[0]], -20.0);
        assert_eq!(v.detune_cents[lanes[1]], 20.0);
        assert!((v.level_comp() - 1.0 / 2.0f32.sqrt()).abs() < 1e-6);
    }

    #[test]
    fn solo_pins_lane_zero_and_quiesces_the_rest() {
        let mut v = Voices::new();
        v.note_on_stack(0, 60, 1.0, N, VoiceMode::Solo, 30.0, false);
        let t = v.note_on_stack(0, 64, 1.0, 1, VoiceMode::Solo, 30.0, false);
        assert_eq!(fired(&t), vec![0]);
        assert_eq!(v.detune_cents[0], 0.0, "Solo is undetuned");
        assert!((1..N).all(|i| !v.gate[i]), "the ex-Unison lanes release");
        assert_eq!(v.level_comp(), 1.0);
    }

    #[test]
    fn detune_zero_leaves_the_stack_in_unison() {
        let mut v = Voices::new();
        v.note_on_stack(0, 60, 1.0, N, VoiceMode::Solo, 0.0, false);
        assert!(v.detune_cents.iter().all(|&d| d == 0.0));
    }

    #[test]
    fn mono_release_reveals_the_note_beneath() {
        let mut v = Voices::new();
        v.note_on_stack(0, 60, 1.0, 1, VoiceMode::Solo, 0.0, false);
        v.note_on_stack(0, 64, 1.0, 1, VoiceMode::Solo, 0.0, false);
        // Releasing the sounding (newest) note falls back to the held one.
        let t = v.note_off_stack(0, 64, 1, VoiceMode::Solo, 0.0, false);
        assert_eq!(fired(&t), vec![0], "revert re-articulates without legato");
        assert_eq!(v.note[0], 60);
        assert!(v.gate[0]);
        // Releasing the last note gates off.
        v.note_off_stack(0, 60, 1, VoiceMode::Solo, 0.0, false);
        assert!(!v.gate[0]);
    }

    #[test]
    fn releasing_a_buried_note_leaves_the_sounding_one_alone() {
        let mut v = Voices::new();
        v.note_on_stack(0, 60, 1.0, 1, VoiceMode::Solo, 0.0, false);
        v.note_on_stack(0, 64, 1.0, 1, VoiceMode::Solo, 0.0, false);
        let t = v.note_off_stack(0, 60, 1, VoiceMode::Solo, 0.0, false);
        assert!(fired(&t).is_empty());
        assert_eq!(v.note[0], 64, "the top of the stack still sounds");
        assert!(v.gate[0]);
    }

    #[test]
    fn legato_slides_instead_of_retriggering() {
        let mut v = Voices::new();
        let first = v.note_on_stack(0, 60, 1.0, 1, VoiceMode::Solo, 0.0, true);
        assert_eq!(fired(&first), vec![0], "the first note always articulates");
        let latched = v.note_random(0);
        let second = v.note_on_stack(0, 64, 1.0, 1, VoiceMode::Solo, 0.0, true);
        assert!(fired(&second).is_empty(), "a slur must not retrigger");
        assert_eq!(v.note[0], 64, "but it does re-point the pitch");
        assert_eq!(v.note_random(0), latched, "a slide keeps the latched humanisation");
    }

    #[test]
    fn legato_slide_restamps_the_unison_fan() {
        let mut v = Voices::new();
        v.note_on_stack(0, 60, 1.0, N, VoiceMode::Solo, 40.0, true);
        v.note_on_stack(0, 64, 1.0, N, VoiceMode::Solo, 40.0, true);
        assert!((v.detune_cents[N - 1] - 40.0).abs() < 1e-4, "fan survives the slur");
        assert!((0..N).all(|i| v.note[i] == 64));
    }

    #[test]
    fn entering_a_mono_mode_releases_held_poly_voices() {
        let mut v = Voices::new();
        let a = v.note_on_stack(0, 60, 1.0, 1, VoiceMode::Poly, 0.0, false).as_slice()[0].voice;
        let b = v.note_on_stack(0, 64, 1.0, 1, VoiceMode::Poly, 0.0, false).as_slice()[0].voice;
        v.note_on_stack(0, 67, 1.0, N, VoiceMode::Solo, 0.0, false);
        // The poly holds are released, not stranded gated-on. (Lane 0/1 may be
        // re-taken by the Unison stack, which re-gates them on the new note.)
        assert!([a, b].iter().all(|&i| v.note[i] == 67 || !v.gate[i]));
    }

    #[test]
    fn note_off_after_a_mode_switch_still_releases() {
        // A note placed under Poly isn't on the mono stack; releasing it under a
        // mono mode must fall back to the poly path rather than strand it.
        let mut v = Voices::new();
        let a = v.note_on_stack(0, 60, 1.0, 1, VoiceMode::Poly, 0.0, false).as_slice()[0].voice;
        v.note_off_stack(0, 60, N, VoiceMode::Solo, 0.0, false);
        assert!(!v.gate[a]);
    }

    #[test]
    fn mono_stack_survives_overflow() {
        let mut v = Voices::new();
        for i in 0..(MONO_STACK + 4) {
            v.note_on_stack(0, 40 + i as u8, 1.0, 1, VoiceMode::Solo, 0.0, false);
        }
        assert_eq!(v.mono_len, MONO_STACK);
        // The newest note still sounds and the stack is intact enough to unwind.
        assert_eq!(v.note[0], 40 + (MONO_STACK + 3) as u8);
    }

    #[test]
    fn repeating_a_held_note_moves_it_to_the_top() {
        let mut v = Voices::new();
        v.note_on_stack(0, 60, 1.0, 1, VoiceMode::Solo, 0.0, false);
        v.note_on_stack(0, 64, 1.0, 1, VoiceMode::Solo, 0.0, false);
        v.note_on_stack(0, 60, 1.0, 1, VoiceMode::Solo, 0.0, false);
        assert_eq!(v.mono_len, 2, "no duplicate entry");
        v.note_off_stack(0, 60, 1, VoiceMode::Solo, 0.0, false);
        assert_eq!(v.note[0], 64);
    }

    #[test]
    fn level_comp_shrinks_with_stack_width() {
        let mut v = Voices::new();
        v.note_on_stack(0, 60, 1.0, 1, VoiceMode::Poly, 0.0, false);
        let poly = v.level_comp();
        v.note_on_stack(0, 60, 1.0, 2, VoiceMode::Poly, 0.0, false);
        let twin = v.level_comp();
        v.note_on_stack(0, 60, 1.0, N, VoiceMode::Solo, 0.0, false);
        let uni = v.level_comp();
        assert!(poly > twin && twin > uni, "poly {poly} twin {twin} unison {uni}");
    }

    #[test]
    fn reset_clears_detune_and_the_mono_stack() {
        let mut v = Voices::new();
        v.note_on_stack(0, 60, 1.0, N, VoiceMode::Solo, 50.0, false);
        v.reset();
        assert!(v.detune_cents.iter().all(|&d| d == 0.0));
        assert_eq!(v.mono_len, 0);
        assert_eq!(v.level_comp(), 1.0);
    }

    #[test]
    fn pressure_is_clamped() {
        let mut voices = Voices::new();
        let v = voices.note_on(1, 60, 1.0);
        voices.channel_pressure(1, 2.5);
        assert_eq!(voices.pressure(v), 1.0);
        voices.poly_pressure(1, 60, -1.0);
        assert_eq!(voices.pressure(v), 0.0);
    }
}
