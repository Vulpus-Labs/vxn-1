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

use vxn_dsp::xorshift64;

use crate::params::{StackDistrib, VoiceMode};

/// Lanes per synth (0264). **Local to VXN1b**, deliberately not
/// `vxn_dsp::MAX_VOICES`: that const is vxn-1's, and raising it there would drag
/// VXN1 along for a capacity decision that is VXN1b's alone.
///
/// 32 rather than VXN1's 16 because [`StackWidth`](crate::params::StackWidth)
/// spends the pool — a width-2 patch is only 16-note polyphonic over 32 lanes,
/// and 8 over VXN1b's original 16, which is thin for exactly the fat detuned
/// patches that want a wide stack. Simultaneous notes are `N / width`.
pub(crate) const MAX_VOICES_1B: usize = 32;

/// Voice count. A flat bank: channel/pressure plumbing is orthogonal to VXN1's
/// per-layer SoA split, so allocation reasons over all voices uniformly.
const N: usize = MAX_VOICES_1B;

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

/// Seed for the [`StackDistrib::Random`] lane-position stream (0284). A third
/// stream for the same reason the second one exists: a note played under Random
/// must not shift the start-phase or humanisation sequences the next note draws
/// from.
const SPREAD_SEED: u64 = 0x1B_5EED_0284;

/// Symmetric detune weight in `[-1, 1]` for lane `i` of a stack `width` wide
/// (ADR 0003). Multiplied by the `UnisonDetune` cents value to fan the stack,
/// and — scaled by `Spread` — by the stereo fan, so one position drives both.
///
/// The denominator is `width - 1`, **not** the lane pool: `unison_detune` must
/// mean the same *total span* at every width, so widening a stack makes it
/// denser rather than retuning it. Width 1 is the degenerate case — one lane,
/// no fan, whatever the detune knob says.
///
/// This is [`StackDistrib::Linear`]; the other laws bend it (see
/// [`Voices::fill_stack_pos`]).
#[inline]
pub(crate) fn stack_spread(i: usize, width: usize) -> f32 {
    if width <= 1 {
        0.0
    } else {
        (i as f32 / (width - 1) as f32) * 2.0 - 1.0
    }
}

/// [`StackDistrib::Geometric`] over a linear position: `sign(t) * |t|^0.5`
/// (VXN2's law). Pulls the inner lanes toward the centre while leaving the outer
/// pair pinned at ±1, so a wide stack reads as a dense core with two outliers
/// rather than an even comb.
#[inline]
fn geometric(t: f32) -> f32 {
    t.signum() * t.abs().sqrt()
}

/// The voicing half of a note event: everything about *how* a note is spread
/// across lanes, read off the patch once per note-on and passed as one value.
///
/// A struct rather than six positional arguments (the call 0283's `TriggerOpts`
/// made on the DSP side of the same event) — `width`/`mode`/`detune`/`legato`
/// were already four, and 0284's phase depth and distribution law take it past
/// what a reader can keep straight at a call site.
#[derive(Clone, Copy, Debug)]
pub struct StackVoicing {
    /// Lanes per note, clamped to the pool by the allocator.
    pub width: usize,
    pub mode: VoiceMode,
    /// Total detune span in cents across the stack, fanned by lane position.
    pub unison_detune: f32,
    pub legato: bool,
    /// Start-phase decorrelation depth in `[0, 1]` (0284). Scales the per-lane
    /// random draw: 0 starts every lane coherent at phase 0, 1 is the full
    /// scatter. Ignored at width 1, which keeps its deterministic `lane_phase`.
    pub phase: f32,
    /// Lane layout law for the detune + stereo fan.
    pub distrib: StackDistrib,
}

impl Default for StackVoicing {
    fn default() -> Self {
        Self {
            width: 1,
            mode: VoiceMode::Poly,
            unison_detune: 0.0,
            legato: false,
            // Matches the `stack_phase` descriptor default: full scatter is what
            // a stacked note did before the knob existed.
            phase: 1.0,
            distrib: StackDistrib::Linear,
        }
    }
}

/// Level compensation for a stack of `len` coherent-ish copies: `1/√len`.
///
/// Not `1/len` — the copies are detuned and independently phased, so they sum
/// as a random walk (~√len), not coherently. `1/√len` holds the perceived level
/// roughly constant across stack widths at any detune, with no comb null when
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

/// Steal ranking (lower = sacrificed first): a released tail before a still-held
/// note. So a melody played over a held chord eats the ringing-out tails before
/// it ever touches a key still held. Within a tier the oldest (lowest
/// `alloc_tick`) goes first. A trimmed form of VXN1's `steal_tier` — VXN1b has
/// no sustain-pedal defer state in this layer yet, so held vs. released is the
/// full ranking.
///
/// Takes the gate array directly. It used to take an `AllocView` borrowing
/// three arrays, so the retired single-lane `allocate` could rank a lane
/// without touching mutable state; `worst_stack` is the only caller now and
/// gate is the only field the ranking ever read (0311).
#[inline]
fn steal_tier(gate: &[bool; N], v: usize) -> u8 {
    if !gate[v] { 0 } else { 1 }
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


/// The lane pool's allocation + per-voice performance state
/// ([`crate::MAX_VOICES`] voices). Holds only the
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
    /// Which stack a lane belongs to (0266). Lanes claimed by one note-on share
    /// an id, which is what makes stealing stack-granular: a victim is chosen
    /// by lane but released by *stack*, so no note is ever left half-sounding
    /// with an asymmetric detune fan and a `level_comp` computed for a width it
    /// no longer has. Stale-until-reused, like `note`/`channel`.
    stack_id: [u32; N],
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
    /// Output scaling for the current stack width (`1/√len`) — held here
    /// because it is a property of the *allocation*, not of any one voice. The
    /// synth copies it into the block context each render.
    level_comp: f32,
    /// Monotonic allocation counter; stamped into `alloc_tick` per note-on so
    /// the steal policy can rank by age. Wraps at u64::MAX (unreachable in
    /// practice — ~6M years at 100k notes/s).
    next_tick: u64,
    /// Monotonic stack counter, stamped into [`Voices::stack_id`] once per
    /// note-on so every lane of one note shares an identity.
    next_stack_id: u32,
    /// Note-on-random stream state. Advanced one draw per note-on; a single
    /// stream (not per-voice seeds) guarantees successive voices get distinct
    /// values while staying deterministic. Never zero (xorshift stuck point).
    rng: u64,
    /// Unison start-phase stream, advanced one draw per stacked voice per
    /// trigger. Separate from `rng` so the two humanisation streams don't
    /// perturb each other.
    phase_rng: u64,
    /// [`StackDistrib::Random`] lane-position stream (0284). Advanced only under
    /// Random, and separate from the other two for the same reason they are
    /// separate from each other.
    spread_rng: u64,
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
            stack_id: [0; N],
            pressure: [0.0; N],
            note_random: [0.0; N],
            velocity: [0.0; N],
            detune_cents: [0.0; N],
            stack_pos: [0.0; N],
            mono_stack: [0; MONO_STACK],
            mono_len: 0,
            last_mode: VoiceMode::Poly,
            level_comp: 1.0,
            next_tick: 0,
            next_stack_id: 0,
            rng: NOTE_RANDOM_SEED,
            phase_rng: PHASE_SEED,
            spread_rng: SPREAD_SEED,
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

    /// Output scaling for the current stack width — `1/√len`, so changing width
    /// doesn't jump the perceived level. See [`level_comp`].
    #[inline]
    pub fn level_comp(&self) -> f32 {
        self.level_comp
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

    /// The worst-ranked stack still available to steal, or `None` when every
    /// lane is already spoken for. Ranked by [`steal_tier`] then age, judged on
    /// the stack's *oldest* lane — so a pool of width-1 stacks ranks lane for
    /// lane, which is why 0311 could retire the separate single-lane policy and
    /// point its tests here.
    fn worst_stack(&self, taken: &[bool; N]) -> Option<u32> {
        (0..N)
            .filter(|&v| !taken[v] && self.active[v])
            .min_by_key(|&v| (steal_tier(&self.gate, v), self.alloc_tick[v]))
            .map(|v| self.stack_id[v])
    }

    /// Claim `width` lanes for one note as **whole stacks** (0266).
    ///
    /// Free lanes first, then victim stacks worst-ranked first. The victim is
    /// picked by lane but released by *stack*: every lane sharing the victim's
    /// [`stack_id`](Voices::stack_id) is gated off together. Claiming lane by
    /// lane instead — as this did before — could take part of a held note and
    /// leave the rest of it sounding with a fan missing its outer voices and a
    /// `level_comp` for a width it no longer has. The two policies agree while
    /// every stack is the same width, which is why the seam only shows once a
    /// width change leaves mixed widths held (ADR 0003).
    ///
    /// Surplus lanes from a wider victim are released, not deactivated, so they
    /// ring out on their own envelope tails rather than being cut mid-note.
    /// They rank tier 0 from here on, so the next claim reuses them first.
    ///
    /// Returns the number of lanes claimed — always `width` unless the pool
    /// itself is smaller.
    fn claim_lanes(&mut self, width: usize, out: &mut [usize; N]) -> usize {
        let mut taken = [false; N];
        let mut n = 0;

        for v in 0..N {
            if n == width {
                return n;
            }
            if !self.active[v] {
                out[n] = v;
                taken[v] = true;
                n += 1;
            }
        }
        while n < width {
            let Some(victim) = self.worst_stack(&taken) else { break };
            for v in 0..N {
                if taken[v] || !self.active[v] || self.stack_id[v] != victim {
                    continue;
                }
                // Release the whole victim, whether or not this note needs the
                // lane — that is the point of stack granularity.
                self.gate[v] = false;
                taken[v] = true;
                if n < width {
                    out[n] = v;
                    n += 1;
                }
            }
        }
        n
    }

    /// Take the next stack identity. Wraps; collisions would need `u32::MAX`
    /// note-ons to alias against a lane still holding the old id.
    #[inline]
    fn next_stack(&mut self) -> u32 {
        let id = self.next_stack_id;
        self.next_stack_id = self.next_stack_id.wrapping_add(1);
        id
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
    /// Stacked lanes take a scaled random start phase so the copies do not comb
    /// into a synchronised null — how far they scatter is
    /// [`StackVoicing::phase`]. A single lane keeps the bank's own deterministic
    /// phase whatever that reads.
    pub fn note_on_stack(
        &mut self,
        channel: u8,
        note: u8,
        velocity: f32,
        voicing: StackVoicing,
    ) -> Triggers {
        let StackVoicing { mode, unison_detune, legato, .. } = voicing;
        let width = voicing.width.clamp(1, N);
        self.sync_mode(mode);
        // One draw of the layout per note-on: Random must not re-roll per lane
        // read, and the other laws are pure.
        let pos = self.fill_stack_pos(width, voicing.distrib);
        let mut out = Triggers::none();
        match mode {
            VoiceMode::Poly => {
                // Claim the whole stack up front, so a steal sacrifices whole
                // notes rather than slicing lanes off a held one.
                let mut lanes = [0usize; N];
                let n = self.claim_lanes(width, &mut lanes);
                debug_assert_eq!(n, width, "the pool must always yield a full stack");
                let id = self.next_stack();
                for (i, &v) in lanes[..n].iter().enumerate() {
                    let p = pos[i];
                    self.stamp(v, channel, note, velocity, p * unison_detune, p, true);
                    self.stack_id[v] = id;
                    out.push(v, self.stack_phase(width, voicing.phase));
                }
                self.level_comp = level_comp(width);
            }
            VoiceMode::Solo => {
                let sounding = self.mono_sounding();
                self.mono_push(note);
                // Legato only slides when a note is *already* sounding — the
                // first note of a phrase always articulates.
                let slide = legato && sounding;
                // A slide keeps the sounding stack's identity along with its age
                // and humanisation; an articulation is a new stack.
                let id = if slide { self.stack_id[0] } else { self.next_stack() };
                for (i, &p) in pos.iter().take(width).enumerate() {
                    self.stamp(i, channel, note, velocity, p * unison_detune, p, !slide);
                    self.stack_id[i] = id;
                    if !slide {
                        out.push(i, self.stack_phase(width, voicing.phase));
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
        voicing: StackVoicing,
    ) -> Triggers {
        let StackVoicing { mode, unison_detune, legato, .. } = voicing;
        let width = voicing.width.clamp(1, N);
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
        // A legato reveal keeps the stack's identity; a re-articulated one is a
        // new stack, matching the note-on path.
        let id = if legato { self.stack_id[0] } else { self.next_stack() };
        // A reveal re-lays the stack out, so Random re-rolls here too — the
        // revealed note is a new note in every other respect as well.
        let pos = self.fill_stack_pos(width, voicing.distrib);
        for (i, &p) in pos.iter().take(width).enumerate() {
            let velocity = self.velocity[i];
            self.stamp(i, channel, revealed, velocity, p * unison_detune, p, !legato);
            self.stack_id[i] = id;
            if !legato {
                out.push(i, self.stack_phase(width, voicing.phase));
            }
        }
        out
    }

    /// Lane positions in `[-1, 1]` for a `width`-wide stack under `distrib`.
    /// Lanes past `width` are left at 0 and never read.
    ///
    /// `&mut self` because [`StackDistrib::Random`] draws — which is also why the
    /// caller takes the whole array once per note-on rather than calling a pure
    /// helper per lane.
    fn fill_stack_pos(&mut self, width: usize, distrib: StackDistrib) -> [f32; N] {
        let mut out = [0.0; N];
        for (i, slot) in out.iter_mut().enumerate().take(width) {
            *slot = match distrib {
                StackDistrib::Linear => stack_spread(i, width),
                StackDistrib::Geometric => geometric(stack_spread(i, width)),
                // `note_random_draw` is `[0, 1)`; the fan wants `[-1, 1)`.
                StackDistrib::Random => note_random_draw(&mut self.spread_rng) * 2.0 - 1.0,
            };
        }
        out
    }

    /// Start phase for a lane of a `width`-wide stack: a random draw scaled by
    /// `depth` once there is more than one copy (so the stack's beating need not
    /// comb into a null), and the bank's own deterministic phase — `None`, i.e.
    /// `lane_phase(v)` — for a single lane.
    ///
    /// The draw is scaled *after* it is taken, so the knob cannot shift the
    /// stream a later note pulls from: a phrase played at depth 0 and one played
    /// at depth 1 see the same underlying sequence. At depth 0 every lane lands
    /// on 0.0 and the stack starts coherent. (Width still gates the draw, as it
    /// always has — a one-lane note does not consume from the stream.)
    #[inline]
    fn stack_phase(&mut self, width: usize, depth: f32) -> Option<f32> {
        (width > 1).then(|| note_random_draw(&mut self.phase_rng) * depth.clamp(0.0, 1.0))
    }

    /// Handle a mode change detected at note-on. Entering Solo releases voices
    /// the polyphonic allocator placed (they would sustain under an allocator
    /// that no longer tracks them); leaving it discards the held-note stack so a
    /// later return starts clean.
    ///
    /// Width is deliberately not a parameter: a width change alone needs neither
    /// action — sounding stacks keep their lanes until released (ADR 0003). It
    /// used to be passed in only to be recorded in a `last_width` field nothing
    /// ever read.
    fn sync_mode(&mut self, mode: VoiceMode) {
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

    /// Total voice capacity (= [`MAX_VOICES_1B`]).
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

}

/// Disjoint per-voice slices the render path consumes (see [`Voices::render_view`]).
/// Arrays are the full pool width ([`Voices::CAPACITY`]); the engine slices each
/// 8-lane bank out of them.
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

/// One bank's `LANES`-wide window onto a [`RenderView`] — the slices
/// [`crate::bank::RenderBank::render`] consumes.
///
/// A struct rather than eight positional arguments because five of them are
/// `&[f32]` of identical length: `velocity`, `pressure`, `note_random`,
/// `detune_cents`, `stack_pos`. Transposing any two compiled, ran, and produced
/// plausible-but-wrong audio — velocity landing where stack position was
/// expected does not crash, it just makes stacked notes sound subtly off. Named
/// fields make that unrepresentable.
pub struct LaneView<'a> {
    pub note: &'a [u8],
    pub gate: &'a [bool],
    pub velocity: &'a [f32],
    pub pressure: &'a [f32],
    pub note_random: &'a [f32],
    pub detune_cents: &'a [f32],
    pub stack_pos: &'a [f32],
    pub active: &'a mut [bool],
}

impl<'a> RenderView<'a> {
    /// Split the full-width view into consecutive `lanes`-wide windows, one per
    /// bank. Consumes the view because `active` is `&mut` and each window takes
    /// a disjoint piece of it.
    pub fn banks(self, lanes: usize) -> impl Iterator<Item = LaneView<'a>> {
        let RenderView {
            note,
            gate,
            velocity,
            pressure,
            note_random,
            detune_cents,
            stack_pos,
            active,
        } = self;
        active.chunks_mut(lanes).enumerate().map(move |(b, active)| {
            let r = b * lanes..b * lanes + lanes;
            LaneView {
                note: &note[r.clone()],
                gate: &gate[r.clone()],
                velocity: &velocity[r.clone()],
                pressure: &pressure[r.clone()],
                note_random: &note_random[r.clone()],
                detune_cents: &detune_cents[r.clone()],
                stack_pos: &stack_pos[r],
                active,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::StackWidth;

    /// The four voicing axes these tests actually vary, over 0284's defaults
    /// (full phase scatter, Linear layout) — which is the pre-0284 behaviour, so
    /// every test written before it still exercises what it used to.
    fn voicing(width: usize, mode: VoiceMode, unison_detune: f32, legato: bool) -> StackVoicing {
        StackVoicing { width, mode, unison_detune, legato, ..StackVoicing::default() }
    }

    /// One note through the **shipping** allocator at width 1 — what the
    /// retired `Voices::note_on` did. These tests cover voice stealing, which
    /// is real behaviour; they were the only thing keeping a second allocation
    /// policy alive, so 0311 re-pointed them at `note_on_stack` rather than
    /// keeping the code to keep the tests.
    fn note_on_1(voices: &mut Voices, channel: u8, note: u8, velocity: f32) -> usize {
        voices
            .note_on_stack(channel, note, velocity, voicing(1, VoiceMode::Poly, 0.0, false))
            .as_slice()
            .first()
            .map_or(0, |t| t.voice)
    }

    #[test]
    fn note_on_stores_channel_on_assigned_voice() {
        let mut voices = Voices::new();
        let v = note_on_1(&mut voices, 3, 60, 1.0);
        assert!(voices.is_active(v));
        assert_eq!(voices.channel(v), 3);
        assert_eq!(voices.note(v), 60);
    }

    #[test]
    fn per_note_pressure_isolated_to_matching_voice() {
        let mut voices = Voices::new();
        // MPE: each note on its own channel.
        let a = note_on_1(&mut voices, 1, 60, 1.0);
        let b = note_on_1(&mut voices, 2, 64, 1.0);
        voices.poly_pressure(1, 60, 0.8);
        assert_eq!(voices.pressure(a), 0.8);
        // Must not leak to the other channel/note.
        assert_eq!(voices.pressure(b), 0.0);
    }

    #[test]
    fn per_note_pressure_ignores_same_note_other_channel() {
        let mut voices = Voices::new();
        let a = note_on_1(&mut voices, 1, 60, 1.0);
        let b = note_on_1(&mut voices, 2, 60, 1.0); // same note, different channel
        voices.poly_pressure(2, 60, 0.5);
        assert_eq!(voices.pressure(a), 0.0);
        assert_eq!(voices.pressure(b), 0.5);
    }

    #[test]
    fn channel_pressure_broadcasts_to_all_voices_on_channel() {
        let mut voices = Voices::new();
        // Channel mode: several notes share one channel.
        let a = note_on_1(&mut voices, 1, 60, 1.0);
        let b = note_on_1(&mut voices, 1, 64, 1.0);
        let c = note_on_1(&mut voices, 2, 67, 1.0); // other channel — untouched
        voices.channel_pressure(1, 0.6);
        assert_eq!(voices.pressure(a), 0.6);
        assert_eq!(voices.pressure(b), 0.6);
        assert_eq!(voices.pressure(c), 0.0);
    }

    #[test]
    fn free_voice_chosen_before_steal() {
        let mut voices = Voices::new();
        let first = note_on_1(&mut voices, 1, 60, 1.0);
        let second = note_on_1(&mut voices, 1, 61, 1.0);
        assert_ne!(first, second, "distinct free voices used before any steal");
    }

    #[test]
    fn stolen_voice_reparents_to_stealing_channel() {
        let mut voices = Voices::new();
        // Fill every voice on channel 1.
        for i in 0..N {
            note_on_1(&mut voices, 1, 60 + i as u8, 1.0);
        }
        // Next note-on on channel 2 must steal and re-parent.
        let stolen = note_on_1(&mut voices, 2, 72, 1.0);
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
        let oldest = note_on_1(&mut voices, 1, 60, 1.0);
        for i in 1..N {
            note_on_1(&mut voices, 1, 60 + i as u8, 1.0);
        }
        // All held; the first-allocated (oldest tick) is sacrificed.
        let stolen = note_on_1(&mut voices, 1, 90, 1.0);
        assert_eq!(stolen, oldest);
    }

    #[test]
    fn released_tail_stolen_before_held_note() {
        let mut voices = Voices::new();
        // Voice 0 is the oldest but gets released; a later voice is younger but held.
        let released = note_on_1(&mut voices, 1, 60, 1.0);
        for i in 1..N {
            note_on_1(&mut voices, 1, 60 + i as u8, 1.0);
        }
        voices.note_off(1, 60); // release the oldest → tier 0
        // A held-but-younger voice exists, yet the released tail goes first.
        let stolen = note_on_1(&mut voices, 1, 90, 1.0);
        assert_eq!(stolen, released);
    }

    #[test]
    fn note_off_matches_channel_and_note() {
        let mut voices = Voices::new();
        let a = note_on_1(&mut voices, 1, 60, 1.0);
        let b = note_on_1(&mut voices, 2, 60, 1.0); // same note, other channel
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
            let v = note_on_1(&mut voices, 1, 60 + i as u8, 1.0);
            let r = voices.note_random(v);
            assert!((0.0..1.0).contains(&r), "note-random {r} out of [0,1)");
        }
    }

    #[test]
    fn note_random_constant_over_note_lifetime() {
        let mut voices = Voices::new();
        let a = note_on_1(&mut voices, 1, 60, 1.0);
        let latched = voices.note_random(a);
        // Unrelated activity must not disturb a held voice's latched value.
        note_on_1(&mut voices, 2, 64, 1.0);
        voices.channel_pressure(1, 0.7);
        voices.poly_pressure(1, 60, 0.3);
        assert_eq!(voices.note_random(a), latched);
    }

    #[test]
    fn note_random_differs_across_concurrent_voices() {
        let mut voices = Voices::new();
        let mut seen: Vec<f32> = Vec::new();
        for i in 0..N {
            let v = note_on_1(&mut voices, 1, 60 + i as u8, 1.0);
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
            let va = note_on_1(&mut a, 1, 60 + i, 1.0);
            let vb = note_on_1(&mut b, 1, 60 + i, 1.0);
            assert_eq!(a.note_random(va), b.note_random(vb));
        }
    }

    #[test]
    fn note_random_relatched_on_reuse() {
        // A reused (stolen) voice draws a fresh value, not the stale one.
        let mut voices = Voices::new();
        for i in 0..N {
            note_on_1(&mut voices, 1, 60 + i as u8, 1.0);
        }
        let stolen = note_on_1(&mut voices, 2, 90, 1.0);
        let before = voices.note_random(stolen);
        // Steal it again with another full round + one more note.
        for i in 0..N {
            note_on_1(&mut voices, 3, 40 + i as u8, 1.0);
        }
        let stolen2 = note_on_1(&mut voices, 4, 91, 1.0);
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

    /// Every selectable stack width, straight off the param enum. Tests that
    /// sweep widths iterate this rather than a literal list, so adding a width
    /// (0264 added 32) widens their coverage instead of silently escaping it.
    fn stack_widths() -> Vec<usize> {
        (0..StackWidth::COUNT).map(|i| StackWidth::from_index(i).lanes()).collect()
    }

    /// The widest selectable stack is exactly the lane pool — no width can ask
    /// for more lanes than exist, and none leaves the pool partly unreachable.
    #[test]
    fn the_widest_stack_is_the_whole_pool() {
        let widths = stack_widths();
        assert_eq!(*widths.last().unwrap(), N);
        for w in widths {
            assert_eq!(N % w, 0, "width {w} does not tile the {N}-lane pool");
        }
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
            let t = v.note_on_stack(0, 60, 1.0, voicing(width, mode, 20.0, false));
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
        let first = v.note_on_stack(0, 60, 1.0, voicing(N, VoiceMode::Poly, 10.0, true));
        assert_eq!(fired(&first).len(), N, "the stack takes every lane");
        // Legato is on, but Poly never slides: the second note re-fires lanes.
        let second = v.note_on_stack(0, 67, 1.0, voicing(N, VoiceMode::Poly, 10.0, true));
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
        v.note_on_stack(0, 60, 1.0, voicing(N, VoiceMode::Solo, 10.0, true));
        let second = v.note_on_stack(0, 67, 1.0, voicing(N, VoiceMode::Solo, 10.0, true));
        assert!(fired(&second).is_empty(), "a legato slide must not retrigger");
        assert!((0..N).all(|i| v.note[i] == 67), "but the pitch moves");
    }

    /// `unison_detune` means the same **total span** at every width: the
    /// outermost lanes sit at ±detune regardless, and a wider stack is denser
    /// in between rather than wider overall. Without this rule the same patch
    /// is a different chord at each width.
    #[test]
    fn detune_span_is_constant_across_widths() {
        // Enumerated from `StackWidth`, not a literal list, so a width added
        // later (as 32 was, in 0264) cannot slip past this rule unchecked.
        for width in stack_widths().into_iter().filter(|&w| w > 1) {
            let mut v = Voices::default();
            v.note_on_stack(0, 60, 1.0, voicing(width, VoiceMode::Solo, 25.0, false));
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
        v.note_on_stack(0, 60, 1.0, voicing(1, VoiceMode::Poly, 50.0, false));
        assert_eq!(v.detune_cents[0], 0.0);
    }

    /// Poly capacity is `N / width`: the stack that does not fit steals rather
    /// than sounding alongside.
    #[test]
    fn poly_capacity_is_the_pool_divided_by_width() {
        for width in stack_widths() {
            let mut v = Voices::default();
            let stacks = N / width;
            for i in 0..stacks {
                v.note_on_stack(0, 60 + i as u8, 1.0, voicing(width, VoiceMode::Poly, 0.0, false));
            }
            assert_eq!(
                (0..N).filter(|&i| v.is_active(i)).count(),
                N,
                "width {width}: {stacks} stacks should fill the pool"
            );
            // One more note: capacity is spent, so it steals — the note count
            // held stays at the pool size rather than growing.
            v.note_on_stack(0, 90, 1.0, voicing(width, VoiceMode::Poly, 0.0, false));
            assert_eq!((0..N).filter(|&i| v.is_active(i)).count(), N);
            assert!(
                (0..N).any(|i| v.note[i] == 90 && v.is_active(i)),
                "width {width}: the stealing note must sound"
            );
        }
    }

    /// The stereo fan spreads each stack evenly across the image whatever its
    /// width, because `stack_pos` comes off the same `stack_spread` helper as
    /// the detune fan — a lane's position *within its stack*, not within a bank.
    ///
    /// This is the assumption 0260's `SourceId::Spread` originally got wrong: it
    /// read an 8-entry per-bank table, which is only right while a stack *is* a
    /// bank. At width 2 that handed a stack two arbitrary points off the table,
    /// and at width 32 it repeated the same 8 positions four times. Invisible at
    /// width 8, which is why it needs an explicit test.
    #[test]
    fn the_stereo_fan_spans_the_image_at_every_width() {
        // Width 1 sits dead centre — nothing to spread.
        let mut v = Voices::default();
        v.note_on_stack(0, 60, 1.0, voicing(1, VoiceMode::Poly, 0.0, false));
        assert_eq!(v.stack_pos[0], 0.0, "a single lane must be centred");

        for width in stack_widths().into_iter().filter(|&w| w > 1) {
            let mut v = Voices::default();
            let t = v.note_on_stack(0, 60, 1.0, voicing(width, VoiceMode::Poly, 0.0, false));
            let pos: Vec<f32> = fired(&t).iter().map(|&l| v.stack_pos[l]).collect();
            assert_eq!(pos.len(), width);
            // Outermost lanes reach the edges: a 2-lane stack is hard L/R, a
            // 32-lane stack fills the same span densely.
            assert!((pos[0] + 1.0).abs() < 1e-6, "width {width} left edge {}", pos[0]);
            assert!(
                (pos[width - 1] - 1.0).abs() < 1e-6,
                "width {width} right edge {}",
                pos[width - 1]
            );
            // Evenly spaced, so the image fills rather than clumping.
            let step = 2.0 / (width - 1) as f32;
            for pair in pos.windows(2) {
                assert!(
                    (pair[1] - pair[0] - step).abs() < 1e-5,
                    "width {width} uneven stereo fan"
                );
            }
        }
    }

    // ── Stack-granular allocation (0266) ────────────────────────────────────

    /// Lanes claimed by one note-on share a stack identity — the thing that
    /// makes a steal able to release a whole note.
    #[test]
    fn one_note_on_claims_one_stack() {
        let mut v = Voices::default();
        let t = v.note_on_stack(0, 60, 1.0, voicing(8, VoiceMode::Poly, 10.0, false));
        let lanes = fired(&t);
        let id = v.stack_id[lanes[0]];
        assert!(lanes.iter().all(|&l| v.stack_id[l] == id), "one note, one stack id");

        let u = v.note_on_stack(0, 67, 1.0, voicing(8, VoiceMode::Poly, 10.0, false));
        assert_ne!(v.stack_id[fired(&u)[0]], id, "a second note is a second stack");
    }

    /// The divergence this policy exists for: a claim needing fewer lanes than
    /// the victim holds still releases the victim **whole**, so no note is left
    /// sounding with part of its stack gone and a fan missing its outer voices.
    /// Only reachable with mixed widths held, which is exactly what a width
    /// change under held notes produces (ADR 0003).
    #[test]
    fn a_steal_releases_the_whole_victim_stack() {
        let mut v = Voices::default();
        for i in 0..(N / 8) {
            v.note_on_stack(0, 60 + i as u8, 1.0, voicing(8, VoiceMode::Poly, 10.0, false));
        }
        assert!((0..N).all(|i| v.gate[i]), "the pool must start fully held");

        // Width drops to 4, so the next note wants half of a victim's lanes.
        v.note_on_stack(0, 90, 1.0, voicing(4, VoiceMode::Poly, 10.0, false));

        assert!(
            (0..N).all(|i| !(v.gate[i] && v.note[i] == 60)),
            "the victim must be released whole, not sliced"
        );
        for note in 61..(60 + (N / 8) as u8) {
            assert_eq!(
                (0..N).filter(|&i| v.gate[i] && v.note[i] == note).count(),
                8,
                "note {note} lost lanes to a steal that should not have touched it"
            );
        }
        assert_eq!(
            (0..N).filter(|&i| v.gate[i] && v.note[i] == 90).count(),
            4,
            "the stealing note must get its full stack"
        );
    }

    /// Lanes of the victim the new note doesn't need are *released*, not
    /// deactivated: cutting them dead mid-note would click. They stay active
    /// (ringing out) and gate-off, which also ranks them tier 0 for reuse.
    #[test]
    fn surplus_lanes_of_a_stolen_stack_ring_out_rather_than_being_cut() {
        let mut v = Voices::default();
        for i in 0..(N / 8) {
            v.note_on_stack(0, 60 + i as u8, 1.0, voicing(8, VoiceMode::Poly, 10.0, false));
        }
        v.note_on_stack(0, 90, 1.0, voicing(4, VoiceMode::Poly, 10.0, false));

        let ringing: Vec<usize> =
            (0..N).filter(|&i| v.note[i] == 60 && v.active[i] && !v.gate[i]).collect();
        assert_eq!(ringing.len(), 4, "the unused half of the victim must ring out");
    }

    /// Releasing a note frees every lane of its stack — no lane outlives the
    /// note that owns it.
    #[test]
    fn releasing_a_note_frees_every_lane_of_its_stack() {
        let mut v = Voices::default();
        v.note_on_stack(0, 60, 1.0, voicing(8, VoiceMode::Poly, 10.0, false));
        assert_eq!((0..N).filter(|&i| v.gate[i]).count(), 8);
        v.note_off_stack(0, 60, voicing(8, VoiceMode::Poly, 10.0, false));
        assert!((0..N).all(|i| !v.gate[i]), "every lane of the stack must release");
    }

    /// Stack granularity must not change the uniform-width case, which is every
    /// patch that never touches the Width control mid-hold. Filling the pool at
    /// width 1 and stealing still takes the oldest lane.
    #[test]
    fn uniform_widths_steal_exactly_as_the_lane_policy_did() {
        let mut v = Voices::default();
        for i in 0..N {
            v.note_on_stack(0, 24 + i as u8, 1.0, voicing(1, VoiceMode::Poly, 0.0, false));
        }
        let t = v.note_on_stack(0, 120, 1.0, voicing(1, VoiceMode::Poly, 0.0, false));
        assert_eq!(fired(&t), vec![0], "the oldest lane is still the steal target");
    }

    /// A width change does not re-voice sounding stacks (ADR 0003) — it applies
    /// from the next note-on. Re-partitioning under held notes would be a click
    /// and a stolen-note storm.
    #[test]
    fn a_width_change_leaves_held_stacks_alone() {
        let mut v = Voices::default();
        v.note_on_stack(0, 60, 1.0, voicing(4, VoiceMode::Poly, 10.0, false));
        let before: Vec<u8> = (0..4).map(|i| v.note[i]).collect();
        // Next note arrives at a different width; the held stack is untouched.
        v.note_on_stack(0, 67, 1.0, voicing(2, VoiceMode::Poly, 10.0, false));
        let after: Vec<u8> = (0..4).map(|i| v.note[i]).collect();
        assert_eq!(before, after, "the held 4-lane stack must keep its lanes");
    }

    #[test]
    fn poly_places_one_undetuned_voice() {
        let mut v = Voices::new();
        let t = v.note_on_stack(0, 60, 1.0, voicing(1, VoiceMode::Poly, 50.0, false));
        assert_eq!(fired(&t).len(), 1);
        assert_eq!(v.detune_cents[t.as_slice()[0].voice], 0.0);
        assert_eq!(v.level_comp(), 1.0);
    }

    #[test]
    fn unison_stacks_every_lane_fanned_across_the_detune() {
        let mut v = Voices::new();
        let t = v.note_on_stack(0, 60, 1.0, voicing(N, VoiceMode::Solo, 50.0, false));
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
        let t = v.note_on_stack(0, 60, 1.0, voicing(N, VoiceMode::Solo, 12.0, false));
        let phases: Vec<f32> = t.as_slice().iter().map(|x| x.start_phase.unwrap()).collect();
        assert!(phases.iter().all(|&p| (0.0..1.0).contains(&p)));
        assert!(
            phases.windows(2).any(|w| (w[0] - w[1]).abs() > 1e-6),
            "stacked copies must not share a start phase"
        );
    }

    /// 0284: the depth knob scales the draw, it does not replace it. At 1.0 the
    /// phases are the ones the pre-0284 engine produced from the same seed, and
    /// at 0.0 the whole stack starts coherent.
    #[test]
    fn stack_phase_depth_scales_the_start_phase_draw() {
        let full: Vec<f32> = {
            let mut v = Voices::new();
            let t = v.note_on_stack(0, 60, 1.0, voicing(8, VoiceMode::Poly, 12.0, false));
            t.as_slice().iter().map(|x| x.start_phase.unwrap()).collect()
        };

        let mut v = Voices::new();
        let half = StackVoicing { phase: 0.5, ..voicing(8, VoiceMode::Poly, 12.0, false) };
        let t = v.note_on_stack(0, 60, 1.0, half);
        let scaled: Vec<f32> = t.as_slice().iter().map(|x| x.start_phase.unwrap()).collect();
        assert_eq!(scaled.len(), full.len());
        for (s, f) in scaled.iter().zip(&full) {
            assert!((s - f * 0.5).abs() < 1e-6, "half depth must halve the draw");
        }

        let mut v = Voices::new();
        let none = StackVoicing { phase: 0.0, ..voicing(8, VoiceMode::Poly, 12.0, false) };
        let t = v.note_on_stack(0, 60, 1.0, none);
        assert!(
            t.as_slice().iter().all(|x| x.start_phase == Some(0.0)),
            "depth 0 must start every lane of the stack coherent"
        );
    }

    /// The knob must not perturb the stream: two phrases played at different
    /// depths draw the same underlying sequence, so a patch tweak cannot change
    /// which random values a later note lands on.
    #[test]
    fn stack_phase_depth_does_not_shift_the_random_stream() {
        let draws = |depth: f32| {
            let mut v = Voices::new();
            let mut out = Vec::new();
            for note in [60_u8, 64, 67] {
                let t = v.note_on_stack(
                    0,
                    note,
                    1.0,
                    StackVoicing { phase: depth, ..voicing(4, VoiceMode::Poly, 12.0, false) },
                );
                out.extend(t.as_slice().iter().map(|x| x.start_phase.unwrap() / depth));
            }
            out
        };
        let a = draws(1.0);
        let b = draws(0.25);
        assert_eq!(a.len(), 12);
        for (x, y) in a.iter().zip(&b) {
            assert!((x - y).abs() < 1e-6, "the draw sequence must not depend on depth");
        }
    }

    /// Width 1 has no stack to decorrelate, so it keeps deferring to the bank's
    /// deterministic `lane_phase` whatever the depth reads.
    #[test]
    fn stack_phase_is_inert_at_width_one() {
        for depth in [0.0, 0.5, 1.0] {
            let mut v = Voices::new();
            let t = v.note_on_stack(
                0,
                60,
                1.0,
                StackVoicing { phase: depth, ..voicing(1, VoiceMode::Poly, 0.0, false) },
            );
            assert_eq!(t.as_slice()[0].start_phase, None, "depth {depth}");
        }
    }

    /// 0284's layout laws, judged on the positions they hand the fan. Linear is
    /// the pre-existing even comb; Geometric keeps the edges and pulls the inner
    /// lanes in; Random fills the span without ordering.
    #[test]
    fn stack_distrib_laws_lay_the_lanes_out_differently() {
        let pos_for = |distrib| {
            let mut v = Voices::new();
            let t = v.note_on_stack(
                0,
                60,
                1.0,
                StackVoicing { distrib, ..voicing(8, VoiceMode::Poly, 0.0, false) },
            );
            fired(&t).iter().map(|&l| v.stack_pos[l]).collect::<Vec<f32>>()
        };

        let lin = pos_for(StackDistrib::Linear);
        for (i, p) in lin.iter().enumerate() {
            assert!((p - stack_spread(i, 8)).abs() < 1e-6);
        }

        let geo = pos_for(StackDistrib::Geometric);
        // Same edges — the span is the law's, not the width's.
        assert!((geo[0] + 1.0).abs() < 1e-6);
        assert!((geo[7] - 1.0).abs() < 1e-6);
        assert!(geo.windows(2).all(|w| w[1] > w[0]), "still monotone");
        // Every interior lane sits further out than Linear put it (|t|^0.5 > |t|
        // for |t| < 1), which is what "denser at the edges" means for the fan.
        for i in 1..7 {
            assert!(
                geo[i].abs() > lin[i].abs() - 1e-6,
                "lane {i}: geo {} vs lin {}",
                geo[i],
                lin[i]
            );
        }

        let rnd = pos_for(StackDistrib::Random);
        assert!(rnd.iter().all(|p| (-1.0..1.0).contains(p)));
        assert!(
            rnd.windows(2).any(|w| w[1] < w[0]),
            "a random layout must not come out ordered"
        );
    }

    /// The Random layout draws from its own stream, so playing under it cannot
    /// shift the start phases or the humanisation values a later note sees.
    #[test]
    fn random_distrib_does_not_disturb_the_other_streams() {
        let phases = |distrib| {
            let mut v = Voices::new();
            let t = v.note_on_stack(
                0,
                60,
                1.0,
                StackVoicing { distrib, ..voicing(4, VoiceMode::Poly, 0.0, false) },
            );
            let lanes = fired(&t);
            let ph: Vec<f32> = t.as_slice().iter().map(|x| x.start_phase.unwrap()).collect();
            let nr: Vec<f32> = lanes.iter().map(|&l| v.note_random[l]).collect();
            (ph, nr)
        };
        assert_eq!(phases(StackDistrib::Linear), phases(StackDistrib::Random));
    }

    #[test]
    fn twin_places_two_voices_at_the_fan_extremes() {
        let mut v = Voices::new();
        let t = v.note_on_stack(0, 60, 1.0, voicing(2, VoiceMode::Poly, 20.0, false));
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
        v.note_on_stack(0, 60, 1.0, voicing(N, VoiceMode::Solo, 30.0, false));
        let t = v.note_on_stack(0, 64, 1.0, voicing(1, VoiceMode::Solo, 30.0, false));
        assert_eq!(fired(&t), vec![0]);
        assert_eq!(v.detune_cents[0], 0.0, "Solo is undetuned");
        assert!((1..N).all(|i| !v.gate[i]), "the ex-Unison lanes release");
        assert_eq!(v.level_comp(), 1.0);
    }

    #[test]
    fn detune_zero_leaves_the_stack_in_unison() {
        let mut v = Voices::new();
        v.note_on_stack(0, 60, 1.0, voicing(N, VoiceMode::Solo, 0.0, false));
        assert!(v.detune_cents.iter().all(|&d| d == 0.0));
    }

    #[test]
    fn mono_release_reveals_the_note_beneath() {
        let mut v = Voices::new();
        v.note_on_stack(0, 60, 1.0, voicing(1, VoiceMode::Solo, 0.0, false));
        v.note_on_stack(0, 64, 1.0, voicing(1, VoiceMode::Solo, 0.0, false));
        // Releasing the sounding (newest) note falls back to the held one.
        let t = v.note_off_stack(0, 64, voicing(1, VoiceMode::Solo, 0.0, false));
        assert_eq!(fired(&t), vec![0], "revert re-articulates without legato");
        assert_eq!(v.note[0], 60);
        assert!(v.gate[0]);
        // Releasing the last note gates off.
        v.note_off_stack(0, 60, voicing(1, VoiceMode::Solo, 0.0, false));
        assert!(!v.gate[0]);
    }

    #[test]
    fn releasing_a_buried_note_leaves_the_sounding_one_alone() {
        let mut v = Voices::new();
        v.note_on_stack(0, 60, 1.0, voicing(1, VoiceMode::Solo, 0.0, false));
        v.note_on_stack(0, 64, 1.0, voicing(1, VoiceMode::Solo, 0.0, false));
        let t = v.note_off_stack(0, 60, voicing(1, VoiceMode::Solo, 0.0, false));
        assert!(fired(&t).is_empty());
        assert_eq!(v.note[0], 64, "the top of the stack still sounds");
        assert!(v.gate[0]);
    }

    #[test]
    fn legato_slides_instead_of_retriggering() {
        let mut v = Voices::new();
        let first = v.note_on_stack(0, 60, 1.0, voicing(1, VoiceMode::Solo, 0.0, true));
        assert_eq!(fired(&first), vec![0], "the first note always articulates");
        let latched = v.note_random(0);
        let second = v.note_on_stack(0, 64, 1.0, voicing(1, VoiceMode::Solo, 0.0, true));
        assert!(fired(&second).is_empty(), "a slur must not retrigger");
        assert_eq!(v.note[0], 64, "but it does re-point the pitch");
        assert_eq!(v.note_random(0), latched, "a slide keeps the latched humanisation");
    }

    #[test]
    fn legato_slide_restamps_the_unison_fan() {
        let mut v = Voices::new();
        v.note_on_stack(0, 60, 1.0, voicing(N, VoiceMode::Solo, 40.0, true));
        v.note_on_stack(0, 64, 1.0, voicing(N, VoiceMode::Solo, 40.0, true));
        assert!((v.detune_cents[N - 1] - 40.0).abs() < 1e-4, "fan survives the slur");
        assert!((0..N).all(|i| v.note[i] == 64));
    }

    #[test]
    fn entering_a_mono_mode_releases_held_poly_voices() {
        let mut v = Voices::new();
        let a = v.note_on_stack(0, 60, 1.0, voicing(1, VoiceMode::Poly, 0.0, false)).as_slice()[0].voice;
        let b = v.note_on_stack(0, 64, 1.0, voicing(1, VoiceMode::Poly, 0.0, false)).as_slice()[0].voice;
        v.note_on_stack(0, 67, 1.0, voicing(N, VoiceMode::Solo, 0.0, false));
        // The poly holds are released, not stranded gated-on. (Lane 0/1 may be
        // re-taken by the Unison stack, which re-gates them on the new note.)
        assert!([a, b].iter().all(|&i| v.note[i] == 67 || !v.gate[i]));
    }

    #[test]
    fn note_off_after_a_mode_switch_still_releases() {
        // A note placed under Poly isn't on the mono stack; releasing it under a
        // mono mode must fall back to the poly path rather than strand it.
        let mut v = Voices::new();
        let a = v.note_on_stack(0, 60, 1.0, voicing(1, VoiceMode::Poly, 0.0, false)).as_slice()[0].voice;
        v.note_off_stack(0, 60, voicing(N, VoiceMode::Solo, 0.0, false));
        assert!(!v.gate[a]);
    }

    #[test]
    fn mono_stack_survives_overflow() {
        let mut v = Voices::new();
        for i in 0..(MONO_STACK + 4) {
            v.note_on_stack(0, 40 + i as u8, 1.0, voicing(1, VoiceMode::Solo, 0.0, false));
        }
        assert_eq!(v.mono_len, MONO_STACK);
        // The newest note still sounds and the stack is intact enough to unwind.
        assert_eq!(v.note[0], 40 + (MONO_STACK + 3) as u8);
    }

    #[test]
    fn repeating_a_held_note_moves_it_to_the_top() {
        let mut v = Voices::new();
        v.note_on_stack(0, 60, 1.0, voicing(1, VoiceMode::Solo, 0.0, false));
        v.note_on_stack(0, 64, 1.0, voicing(1, VoiceMode::Solo, 0.0, false));
        v.note_on_stack(0, 60, 1.0, voicing(1, VoiceMode::Solo, 0.0, false));
        assert_eq!(v.mono_len, 2, "no duplicate entry");
        v.note_off_stack(0, 60, voicing(1, VoiceMode::Solo, 0.0, false));
        assert_eq!(v.note[0], 64);
    }

    #[test]
    fn level_comp_shrinks_with_stack_width() {
        let mut v = Voices::new();
        v.note_on_stack(0, 60, 1.0, voicing(1, VoiceMode::Poly, 0.0, false));
        let poly = v.level_comp();
        v.note_on_stack(0, 60, 1.0, voicing(2, VoiceMode::Poly, 0.0, false));
        let twin = v.level_comp();
        v.note_on_stack(0, 60, 1.0, voicing(N, VoiceMode::Solo, 0.0, false));
        let uni = v.level_comp();
        assert!(poly > twin && twin > uni, "poly {poly} twin {twin} unison {uni}");
    }

    #[test]
    fn reset_clears_detune_and_the_mono_stack() {
        let mut v = Voices::new();
        v.note_on_stack(0, 60, 1.0, voicing(N, VoiceMode::Solo, 50.0, false));
        v.reset();
        assert!(v.detune_cents.iter().all(|&d| d == 0.0));
        assert_eq!(v.mono_len, 0);
        assert_eq!(v.level_comp(), 1.0);
    }

    #[test]
    fn pressure_is_clamped() {
        let mut voices = Voices::new();
        let v = note_on_1(&mut voices, 1, 60, 1.0);
        voices.channel_pressure(1, 2.5);
        assert_eq!(voices.pressure(v), 1.0);
        voices.poly_pressure(1, 60, -1.0);
        assert_eq!(voices.pressure(v), 0.0);
    }
}
