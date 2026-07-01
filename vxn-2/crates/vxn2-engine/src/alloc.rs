//! Polyphony allocator: 16 fixed `Stack` slots with quietest-voice stealing,
//! Poly / Solo assignment modes, glide (portamento), and channel-wide pitch
//! bend forwarding.
//!
//! A "voice" here is a *stack* of up to 8 lane-packed op-instances (ticket
//! 0005). The allocator's poly cap is 16 stacks, giving up to 16 × 8 = 128
//! op-voice instances in flight when `stack_density = 8`. Stealing operates
//! on whole stacks — picking off the quietest reclaims all of its lanes
//! together.
//!
//! Lifecycle per audio block:
//!
//! 1. Apply MIDI events: [`PolyAlloc::note_on`], [`PolyAlloc::note_off`],
//!    [`PolyAlloc::set_bend`].
//! 2. [`PolyAlloc::block_tick`] — advance glide ramps and free idled stacks.
//! 3. Render: walk `stacks`, call [`vxn2_dsp::stack::stack_tick_stereo`] /
//!    [`vxn2_dsp::stack::stack_tick_mono`] per sample; per-stack EG tick at
//!    control rate.
//!
//! No allocation, no `unwrap`, no panics on the audio thread.
//!
//! ## Solo mode
//!
//! Single stack (`SOLO_SLOT`). A new note on top of a held note re-uses the
//! slot — retriggering the EGs when `legato = false`, continuing them when
//! `legato = true`. Releasing the active note falls back to the most
//! recently played still-held note (last-note-priority stack).
//!
//! ## Glide
//!
//! Linear pitch ramp in semitones over `glide_time_ms`, applied via
//! [`vxn2_dsp::stack::Stack::set_glide`]. Block-rate ramp (one update per
//! `block_tick`).
//!
//! Glide is true portamento in both modes (vxn-1 parity, ticket 0125): every
//! note after the first slides from the previous sounding pitch regardless of
//! note overlap, and independent of `legato` (which controls EG retrigger
//! only). In Poly each reused/stolen voice glides from its own previous pitch;
//! a new note prefers the free slot nearest in pitch (see [`Self::pick_idle`])
//! so glides stay short and musical. Glide is off only when `glide_time_ms`
//! is 0 or the chosen slot has never sounded.

use vxn2_dsp::stack::{Stack, StackParams, VoicePhase};
use vxn2_dsp::voice::VoiceParams;

use crate::shared::Patch;

/// Polyphony cap: a new note past this many *active* (Held/Releasing) voices
/// declicks the quietest one to stay within it. ADR §3 sets this at 16 for v1.
pub const N_ACTIVE: usize = 16;

/// Declick headroom: spare stacks above the active cap. A stolen voice is
/// declicked *in place* (keeping its own filter/interp state — click-free) over
/// `DECLICK_SECS` while the new note takes a spare idle stack. These lanes only
/// ever hold short declick tails, so a handful covers any realistic burst; a
/// new note only hard-reuses a sounding slot if every spare is mid-declick.
pub const N_DECLICK: usize = 4;

/// Physical stack count — what the engine renders and sizes its per-stack DSP
/// buffers (filters, interpolators, smoothers) by. Active voices are capped at
/// [`N_ACTIVE`]; the remaining [`N_DECLICK`] absorb declick tails.
pub const N_STACKS: usize = N_ACTIVE + N_DECLICK;

const IDLE_SEQ: u64 = u64::MAX;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AssignMode {
    #[default]
    Poly,
    Solo,
}

#[derive(Clone, Copy, Debug)]
pub struct AllocParams {
    pub assign_mode: AssignMode,
    /// Solo: skip EG retrigger on a slurred note change. No effect in Poly,
    /// and never gates glide in either mode (glide is governed by
    /// `glide_time_ms` alone — ticket 0125).
    pub legato: bool,
    /// Portamento time in milliseconds. 0 disables glide regardless of mode.
    pub glide_time_ms: f32,
}

impl Default for AllocParams {
    fn default() -> Self {
        Self {
            assign_mode: AssignMode::Poly,
            legato: false,
            glide_time_ms: 12.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct GlideState {
    from_st: f32,
    time_remaining_s: f32,
    time_total_s: f32,
}

pub struct PolyAlloc {
    pub stacks: [Stack; N_STACKS],
    /// Monotonic note-on counter per slot; `IDLE_SEQ` when free.
    seq: [u64; N_STACKS],
    next_seq: u64,
    sample_rate: f32,
    glides: [Option<GlideState>; N_STACKS],
    bend_st: f32,
    /// Last-note-priority stack of currently held notes (Solo bookkeeping).
    held: [u8; N_STACKS],
    held_len: usize,
    /// True when a solo voice is currently live (held or releasing).
    solo_active: bool,
    /// The slot carrying the live solo note (`None` when solo is silent).
    /// Solo rotates a fresh slot per note (round-robin via [`Self::pick_idle`])
    /// and declicks the previous one, so this replaces the old fixed slot 0.
    solo_slot: Option<usize>,
    /// Last solo note pitch (semitones), surviving the slot being freed/reused,
    /// so detached-note portamento still glides from the previous pitch.
    solo_last_pitch: f32,
    /// True once Solo has sounded at least one note since the last
    /// [`Self::clear`]. Unlike `solo_active` it survives note-off, so a *new*
    /// Solo note glides from the previous pitch even when the prior key was
    /// already released (vxn-1 portamento semantics: glide is governed by
    /// glide-time alone, not by note overlap or legato — ticket 0125).
    solo_voiced: bool,
    /// Per-slot "has sounded a note since the last [`Self::clear`]" flag.
    /// Survives note-off/free so a reused Poly slot glides from its previous
    /// pitch (always-glide, vxn-1 parity) and so the nearest-pitch slot pick
    /// only considers slots with a meaningful last pitch (ticket 0125).
    voiced: [bool; N_STACKS],
    /// Sustain pedal (CC64) state. While true, a poly note-off flags the
    /// matching stacks `held_by_pedal` instead of gating them; releasing the
    /// pedal gates every flagged stack off. Poly-only — Solo ignores it.
    sustain: bool,
    /// Per-slot "key released while the pedal was down" flag.
    held_by_pedal: [bool; N_STACKS],
    /// Last assign mode applied via [`Self::set_mode`]. Tracked so a runtime
    /// Poly → Solo flip can gate the held chord off instead of leaving it
    /// sustaining under the monophonic allocator.
    assign_mode: AssignMode,
}

impl PolyAlloc {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            stacks: [Stack::default(); N_STACKS],
            seq: [IDLE_SEQ; N_STACKS],
            next_seq: 0,
            sample_rate,
            glides: [None; N_STACKS],
            bend_st: 0.0,
            held: [0; N_STACKS],
            held_len: 0,
            solo_active: false,
            solo_slot: None,
            solo_last_pitch: 0.0,
            solo_voiced: false,
            voiced: [false; N_STACKS],
            sustain: false,
            held_by_pedal: [false; N_STACKS],
            assign_mode: AssignMode::Poly,
        }
    }

    /// Reset every field to its post-[`Self::new`] state in place — no
    /// struct construction, no heap traffic. `Engine::reset` is a CLAP
    /// audio-thread method, so it must not allocate; this zeroes each field
    /// exactly as `new` would (all on-stack, fixed-size arrays) and preserves
    /// `sample_rate` (the one input `new` takes and `reset` keeps).
    ///
    /// Invariant: every field `new` sets is reset here. A field added to the
    /// struct without a line below is a reset-leak bug.
    pub fn clear(&mut self) {
        self.stacks = [Stack::default(); N_STACKS];
        self.seq = [IDLE_SEQ; N_STACKS];
        self.next_seq = 0;
        self.glides = [None; N_STACKS];
        self.bend_st = 0.0;
        self.held = [0; N_STACKS];
        self.held_len = 0;
        self.solo_active = false;
        self.solo_voiced = false;
        self.voiced = [false; N_STACKS];
        self.sustain = false;
        self.held_by_pedal = [false; N_STACKS];
        self.assign_mode = AssignMode::Poly;
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Allocation generation for slot `i` (`u64::MAX` when free). The engine
    /// compares generations across blocks to detect a fresh note-on in a
    /// reused slot — e.g. to snap its pitch smoother instead of gliding in
    /// from the previous voice's offset (ticket 0063).
    #[inline]
    pub(crate) fn slot_seq(&self, i: usize) -> u64 {
        self.seq[i]
    }

    pub fn note_on(
        &mut self,
        params: &AllocParams,
        stack_params: &StackParams,
        voice_params: &VoiceParams,
        note: u8,
        velocity: u8,
    ) {
        match params.assign_mode {
            AssignMode::Solo => {
                self.note_on_solo(params, stack_params, voice_params, note, velocity);
            }
            AssignMode::Poly => {
                let _ = self.note_on_poly(params, stack_params, voice_params, note, velocity);
            }
        }
    }

    /// Patch-driven note-on. After [ADR 0002] dropped Whole / Layer / Split
    /// voicing this is a single-path allocation that defers to the assign
    /// mode in `params`. Kept as a distinct entry point so the engine's
    /// note dispatch reads against the live `Patch` snapshot rather than
    /// re-shaping the stack + voice pair at every call site.
    pub fn note_on_patch(
        &mut self,
        params: &AllocParams,
        patch: &Patch,
        note: u8,
        velocity: u8,
    ) {
        self.note_on(params, &patch.stack, &patch.voice, note, velocity);
    }

    /// Patch-driven note-off. Mirrors [`Self::note_on_patch`]: defers to the
    /// assign-mode dispatch in [`Self::note_off`] so Solo gets its held-note
    /// fallback / legato re-pitch and Poly gates every stack holding `note`.
    /// (Pre-E006 this hardwired `note_off_poly`, leaving the entire solo
    /// note-off path unreachable from the engine — ticket 0064.)
    pub fn note_off_patch(&mut self, params: &AllocParams, patch: &Patch, note: u8) {
        self.note_off(params, &patch.stack, &patch.voice, note);
    }

    pub fn note_off(
        &mut self,
        params: &AllocParams,
        stack_params: &StackParams,
        voice_params: &VoiceParams,
        note: u8,
    ) {
        match params.assign_mode {
            AssignMode::Solo => self.note_off_solo(params, stack_params, voice_params, note),
            AssignMode::Poly => self.note_off_poly(note),
        }
    }

    /// Apply the live assign mode, reacting to a runtime change. The engine
    /// calls this once per block from the param snapshot, so a host/UI flip of
    /// the assign-mode param lands here before the block's note events.
    ///
    /// Poly → Solo gates every sounding voice off (release tails ring out) and
    /// clears the held-note bookkeeping, so a chord held across the switch does
    /// not keep sustaining under the now-monophonic allocator. Other
    /// transitions are no-ops: Solo → Poly leaves the single live voice
    /// playing, to be joined (not replaced) by the next note.
    pub fn set_mode(&mut self, mode: AssignMode) {
        if mode == self.assign_mode {
            return;
        }
        let prev = self.assign_mode;
        self.assign_mode = mode;
        if prev == AssignMode::Poly && mode == AssignMode::Solo {
            for i in 0..N_STACKS {
                if self.stacks[i].meta.gate {
                    self.stacks[i].note_off();
                }
                self.held_by_pedal[i] = false;
            }
            self.held_len = 0;
            self.solo_active = false;
            self.solo_slot = None;
        }
    }

    /// Release every gated stack and clear all hold state (sustain, pedal,
    /// solo). Used on transport stop to kill stuck notes.
    pub fn all_notes_off(&mut self) {
        for i in 0..N_STACKS {
            if self.stacks[i].meta.gate {
                self.stacks[i].note_off();
            }
            self.held_by_pedal[i] = false;
        }
        self.sustain = false;
        self.held_len = 0;
        self.solo_active = false;
        self.solo_slot = None;
    }

    /// Set channel-wide pitch bend in semitones; forwarded to every stack.
    pub fn set_bend(&mut self, semitones: f32) {
        self.bend_st = semitones;
        for s in &mut self.stacks {
            s.set_bend(semitones);
        }
    }

    pub fn bend(&self) -> f32 {
        self.bend_st
    }

    /// Advance per-block allocator state: glide ramps progress; stacks whose
    /// EGs have fully released are marked idle (seq reset).
    pub fn block_tick(&mut self, block_secs: f32) {
        for i in 0..N_STACKS {
            if let Some(g) = self.glides[i].as_mut() {
                g.time_remaining_s -= block_secs;
                if g.time_remaining_s <= 0.0 {
                    self.stacks[i].set_glide(0.0);
                    self.glides[i] = None;
                } else {
                    let t = g.time_remaining_s / g.time_total_s;
                    let from = g.from_st;
                    self.stacks[i].set_glide(from * t);
                }
            }
            // Free a slot once its voice has gone Idle. The `Stack` self-retires
            // (gate off, phase → Idle) in `eg_tick` once all its EGs decay —
            // a released note, a percussive sustain-0 patch, or a Declick voice
            // whose forced fast release reached 0.
            if self.stacks[i].is_idle() {
                self.free_slot(i);
            }
        }
    }

    // --- Poly internals -----------------------------------------------------

    fn note_on_poly(
        &mut self,
        params: &AllocParams,
        sp: &StackParams,
        vp: &VoiceParams,
        note: u8,
        velocity: u8,
    ) -> usize {
        // Pick the voice (if any) to retire for this note-on:
        //  - a re-press of a pedal-held note retires *that* voice (avoids
        //    doubling), regardless of the active-voice cap;
        //  - otherwise, only once the active (Held/Releasing) voices are at the
        //    cap, retire the quietest to make room.
        let pedal_dup = (0..N_STACKS).find(|&i| {
            self.held_by_pedal[i]
                && self.stacks[i].meta.note == note
                && self.stacks[i].meta.phase != VoicePhase::Declick
        });
        let victim = match pedal_dup {
            Some(dup) => Some(dup),
            None if self.active_count() >= N_ACTIVE => self.pick_victim(),
            None => None,
        };
        // The new note wants a clean idle stack. With N_DECLICK spare stacks
        // above the cap, one is essentially always free — even while a victim
        // declicks. `None` only under a pathological steal burst (every spare
        // mid-declick), in which case we fall back to in-place reuse.
        let idle = self.pick_idle(note);

        let slot = match (victim, idle) {
            (Some(v), Some(s)) if v != s => {
                // Declick the victim *in place* — it keeps its own filter/interp
                // state, so its tail rings out continuously (click-free) — and
                // start the new note fresh on the spare idle stack.
                self.stacks[v].start_declick();
                self.held_by_pedal[v] = false;
                s
            }
            (_, Some(s)) => s, // under cap → fresh onset on an idle stack
            (Some(v), None) => v, // burst fallback: hard-reuse the victim in place
            (None, None) => {
                // Unreachable: under the cap there is always an idle stack. Guard
                // defensively rather than panic on the audio thread.
                self.pick_victim().unwrap_or(0)
            }
        };

        // Always-glide (vxn-1 parity, ticket 0125): a reused or stolen voice
        // slides from its previous sounding pitch (note + any in-flight glide
        // offset) to the new note, independent of overlap or `legato`. A slot
        // that has never sounded snaps. Computed before `note_on` resets the
        // stack's pitch.
        let glide_from = if self.voiced[slot] && params.glide_time_ms > 0.0 {
            let cur_pitch = self.stacks[slot].meta.note as f32 + self.stacks[slot].meta.glide_st;
            cur_pitch - note as f32
        } else {
            0.0
        };
        let counter = self.bump_seq();
        // Capture the pedal-hold flag before `claim_slot` clears it: a stolen
        // pedal-held voice has its physical key already up, so the new note is
        // a fresh attack rather than a legato continuation.
        let was_pedal_held = self.held_by_pedal[slot];
        self.claim_slot(slot, counter);
        if self.stacks[slot].is_idle() {
            // Idle stack → fresh voice, onset from silence (click-free). This is
            // now the common steal path: the victim declicks elsewhere.
            self.stacks[slot].note_on(sp, vp, note, velocity, self.sample_rate, counter);
        } else {
            // Burst fallback only: no spare idle stack, so reuse the victim in
            // place — re-pitch without resetting oscillator phase / LFO2 / the
            // pitch/mod envelopes. Key still down → legato (continue the amp EG);
            // key up (releasing / pedal-held) → restart the amp EG from current
            // level. Both avoid the hard-retrigger click.
            let key_down_held = self.stacks[slot].meta.phase == VoicePhase::Held && !was_pedal_held;
            self.stacks[slot].retarget_pitch(sp, vp, note, velocity, self.sample_rate);
            if !key_down_held {
                self.stacks[slot].retrigger_eg();
            }
        }
        self.stacks[slot].set_bend(self.bend_st);
        self.voiced[slot] = true;
        if glide_from != 0.0 {
            self.start_glide(slot, glide_from, params.glide_time_ms / 1000.0);
        }
        slot
    }

    /// Count of *active* voices — those a player owns and hears (`Held` or
    /// `Releasing`). Excludes `Idle` (free) and `Declick` (being killed) stacks,
    /// so short declick tails living in the spare stacks do not count against
    /// the [`N_ACTIVE`] polyphony cap.
    fn active_count(&self) -> usize {
        self.stacks
            .iter()
            .filter(|s| matches!(s.meta.phase, VoicePhase::Held | VoicePhase::Releasing))
            .count()
    }

    fn note_off_poly(&mut self, note: u8) {
        for i in 0..N_STACKS {
            if self.stacks[i].meta.gate && self.stacks[i].meta.note == note {
                if self.sustain {
                    // Pedal held: defer the release to pedal-up.
                    self.held_by_pedal[i] = true;
                } else {
                    self.stacks[i].note_off();
                }
            }
        }
    }

    /// Sustain pedal (CC64). Poly-only. While held, [`Self::note_off_poly`]
    /// flags matching stacks `held_by_pedal` and keeps their gate high;
    /// releasing the pedal releases every flagged stack.
    pub fn set_sustain(&mut self, on: bool) {
        self.sustain = on;
        if !on {
            for i in 0..N_STACKS {
                if self.held_by_pedal[i] {
                    self.release_pedal_held(i);
                }
            }
        }
    }

    /// Pick a free (idle) stack for a new note. Among idle stacks, vxn-1's rule
    /// (ticket 0125): take the *voiced* one whose last sounding pitch is nearest
    /// the new note, so always-glide slides over a short, musical interval. If no
    /// idle stack has sounded yet, take the first unvoiced idle one (it snaps —
    /// nothing to glide from). `None` only when every stack is busy (≥`N_ACTIVE`
    /// sounding plus all `N_DECLICK` spares mid-declick) — a steal burst.
    fn pick_idle(&self, note: u8) -> Option<usize> {
        let mut nearest_voiced: Option<(usize, f32)> = None;
        let mut first_unvoiced_idle: Option<usize> = None;
        for i in 0..N_STACKS {
            if !self.stacks[i].is_idle() {
                continue;
            }
            if self.voiced[i] {
                let last_pitch = self.stacks[i].meta.note as f32 + self.stacks[i].meta.glide_st;
                let dist = (last_pitch - note as f32).abs();
                let better = match nearest_voiced {
                    Some((_, bd)) => dist < bd,
                    None => true,
                };
                if better {
                    nearest_voiced = Some((i, dist));
                }
            } else if first_unvoiced_idle.is_none() {
                first_unvoiced_idle = Some(i);
            }
        }
        nearest_voiced.map(|(i, _)| i).or(first_unvoiced_idle)
    }

    /// Pick the voice to retire when at the polyphony cap. The *quietest* active
    /// voice (lowest carrier amplitude, ties broken by oldest), preferring those
    /// whose physical key is already *up* — a pedal-sustained note
    /// (`held_by_pedal`) or a `Releasing` tail — over a key the player is still
    /// holding. So a steal under a held sustain pedal sheds a pedal-sustained
    /// note first; only if every active voice has its key down does it take the
    /// quietest of those. Quietest-first makes the declick the least audible (a
    /// near-silent voice's tail is already inaudible). Returns `None` only if no
    /// voice is at/past Sustain yet (every active voice still attacking).
    fn pick_victim(&self) -> Option<usize> {
        let mut best: Option<(usize, f32, u64)> = None;
        let mut best_keyup: Option<(usize, f32, u64)> = None;
        let quieter = |cand: (usize, f32, u64), b: Option<(usize, f32, u64)>| match b {
            Some(b) if b.1 < cand.1 || (b.1 == cand.1 && b.2 <= cand.2) => b,
            _ => cand,
        };
        for i in 0..N_STACKS {
            // Any *active* voice is a valid victim (declick fades it from its
            // current level regardless of EG stage). Idle/Declick stacks are not.
            // This matches `active_count` exactly, so at the cap a victim always
            // exists. Attacking voices have low carrier level, so the quietest
            // pick rarely lands on one anyway.
            let stealable = matches!(
                self.stacks[i].meta.phase,
                VoicePhase::Held | VoicePhase::Releasing
            );
            if stealable {
                let cand = (i, self.stacks[i].carrier_level(), self.seq[i]);
                best = Some(quieter(cand, best));
                if self.held_by_pedal[i] || self.stacks[i].meta.phase == VoicePhase::Releasing {
                    best_keyup = Some(quieter(cand, best_keyup));
                }
            }
        }
        best_keyup.or(best).map(|(i, _, _)| i)
    }

    // --- Solo internals -----------------------------------------------------

    fn note_on_solo(
        &mut self,
        params: &AllocParams,
        sp: &StackParams,
        vp: &VoiceParams,
        note: u8,
        velocity: u8,
    ) {
        // Portamento glides from the *current sounding pitch* (the previous
        // note plus any still-in-flight glide offset) to the new note —
        // regardless of whether the previous key is still held. Gated only by
        // glide-time and `solo_voiced` (so the very first note snaps). `legato`
        // governs EG retrigger, never the glide (ticket 0125).
        self.push_held(note);
        let counter = self.bump_seq();
        let glide_from = if self.solo_voiced && params.glide_time_ms > 0.0 {
            self.solo_pitch() - note as f32
        } else {
            0.0
        };

        // Legato + previous note still Held → re-pitch the same voice in place,
        // no retrigger, no kill. (A `Releasing` previous note is not held, so it
        // falls through to the crossfade.)
        if params.legato {
            if let Some(cur) = self.solo_slot {
                if self.stacks[cur].meta.phase == VoicePhase::Held {
                    self.stacks[cur].retarget_pitch(sp, vp, note, velocity, self.sample_rate);
                    // Legato continuation reuses the SAME sounding voice — do not
                    // bump `seq`. A bumped generation reads as a fresh onset in the
                    // engine (cook_stacks_block), which snaps the pitch smoother and
                    // *zeros the filter/HP z-state* (ADR 0004). On a continuous voice
                    // that mid-note reset is an audible click with the filter (or a
                    // raised HP) on. Phase/EG/LFO already carry over via
                    // `retarget_pitch`; pitch moves via the glide offset below.
                    self.apply_solo_glide(cur, glide_from, params, note);
                    self.solo_active = true;
                    self.solo_voiced = true;
                    return;
                }
            }
        }

        // Otherwise: round-robin to a FRESH voice (onset from silence is
        // click-free) and declick the previous note. `note_on` resets phase /
        // LFO2 / envelopes on the *new* slot only — the outgoing voice keeps
        // running while it fades, so no mid-phrase glitch.
        let prev = self.solo_slot;
        // Solo is monophonic — at most one active voice plus declick tails — so
        // an idle stack is always free; fall back defensively to a victim.
        let slot = self
            .pick_idle(note)
            .or_else(|| self.pick_victim())
            .unwrap_or(0);
        self.claim_slot(slot, counter);
        self.stacks[slot].note_on(sp, vp, note, velocity, self.sample_rate, counter);
        self.stacks[slot].set_bend(self.bend_st);
        self.voiced[slot] = true;
        self.apply_solo_glide(slot, glide_from, params, note);
        if let Some(p) = prev {
            if p != slot {
                self.stacks[p].start_declick();
            }
        }
        self.solo_slot = Some(slot);
        self.solo_active = true;
        self.solo_voiced = true;
    }

    /// Current solo sounding pitch (note + in-flight glide), falling back to the
    /// stashed last pitch once the slot has been freed/reused (detached glide).
    fn solo_pitch(&self) -> f32 {
        match self.solo_slot {
            Some(p) if !self.stacks[p].is_idle() => {
                self.stacks[p].meta.note as f32 + self.stacks[p].meta.glide_st
            }
            _ => self.solo_last_pitch,
        }
    }

    /// Shared glide + last-pitch bookkeeping for a solo note landing on `slot`.
    fn apply_solo_glide(&mut self, slot: usize, glide_from: f32, params: &AllocParams, note: u8) {
        if glide_from != 0.0 {
            self.start_glide(slot, glide_from, params.glide_time_ms / 1000.0);
        } else {
            self.clear_glide(slot);
        }
        self.solo_last_pitch = note as f32;
    }

    fn note_off_solo(
        &mut self,
        params: &AllocParams,
        sp: &StackParams,
        vp: &VoiceParams,
        note: u8,
    ) {
        self.remove_held(note);
        if !self.solo_active {
            return;
        }
        let Some(cur) = self.solo_slot else {
            return;
        };
        if self.stacks[cur].meta.note != note {
            return;
        }
        if let Some(prev) = self.most_recent_held() {
            // Fallback reuses the sounding stack's velocity (the note being
            // released) rather than the held note's original strike — `held`
            // stores notes only. Intentional: classic mono-synth behaviour,
            // continuous with the phrase being played (ticket 0064 Notes).
            let vel = self.stacks[cur].meta.velocity;
            let counter = self.bump_seq();
            let glide_from = if params.glide_time_ms > 0.0 {
                self.solo_pitch() - prev as f32
            } else {
                0.0
            };
            if params.legato && self.stacks[cur].meta.phase == VoicePhase::Held {
                // Held → re-pitch the same voice in place. Same as the note-on
                // legato path: a continuation, NOT a fresh onset, so don't bump
                // `seq` — that would zero the filter/HP z-state mid-note and click.
                self.stacks[cur].retarget_pitch(sp, vp, prev, vel, self.sample_rate);
                self.apply_solo_glide(cur, glide_from, params, prev);
            } else {
                // Crossfade the fallback onto a fresh voice; declick the released.
                let slot = self
                    .pick_idle(prev)
                    .or_else(|| self.pick_victim())
                    .unwrap_or(0);
                self.claim_slot(slot, counter);
                self.stacks[slot].note_on(sp, vp, prev, vel, self.sample_rate, counter);
                self.stacks[slot].set_bend(self.bend_st);
                self.voiced[slot] = true;
                self.apply_solo_glide(slot, glide_from, params, prev);
                if slot != cur {
                    self.stacks[cur].start_declick();
                }
                self.solo_slot = Some(slot);
            }
            self.solo_active = true;
            self.solo_voiced = true;
        } else {
            // No key left: release the current voice naturally. Keep `solo_slot`
            // pointing at it (a fast re-press declicks the tail); `free_slot`
            // clears it once the release completes.
            self.stacks[cur].note_off();
            self.solo_active = false;
        }
    }

    // --- helpers ------------------------------------------------------------

    fn bump_seq(&mut self) -> u64 {
        let s = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        s
    }

    fn start_glide(&mut self, slot: usize, from_st: f32, time_s: f32) {
        self.glides[slot] = Some(GlideState {
            from_st,
            time_remaining_s: time_s,
            time_total_s: time_s,
        });
        self.stacks[slot].set_glide(from_st);
    }

    /// Cancel an in-flight glide on `slot`: drop the ramp state and snap the
    /// stack's glide offset back to zero. Inverse of [`Self::start_glide`].
    fn clear_glide(&mut self, slot: usize) {
        self.glides[slot] = None;
        self.stacks[slot].set_glide(0.0);
    }

    /// Stamp a freshly picked slot's allocation metadata: assign its
    /// generation and clear any leftover glide / pedal-hold from a prior
    /// tenant. Owns only the side arrays — the caller drives the `Stack`
    /// note-on separately.
    fn claim_slot(&mut self, slot: usize, counter: u64) {
        self.glides[slot] = None;
        self.held_by_pedal[slot] = false;
        self.seq[slot] = counter;
    }

    /// Release a slot's allocation metadata back to idle: reset its
    /// generation, drop any in-flight glide, and clear the pedal-hold flag.
    /// Solo's "active" latch follows when `SOLO_SLOT` frees; `solo_voiced`
    /// deliberately survives (it tracks ever-voiced across note-offs). Does
    /// not touch the `Stack` — the EG-idle check that gated this already did.
    fn free_slot(&mut self, slot: usize) {
        self.seq[slot] = IDLE_SEQ;
        self.glides[slot] = None;
        self.held_by_pedal[slot] = false;
        if Some(slot) == self.solo_slot {
            self.solo_active = false;
            self.solo_slot = None;
        }
    }

    /// Fire a deferred (pedal-held) release: gate the stack off and clear its
    /// pedal-hold flag.
    fn release_pedal_held(&mut self, slot: usize) {
        self.stacks[slot].note_off();
        self.held_by_pedal[slot] = false;
    }

    fn push_held(&mut self, note: u8) {
        if let Some(pos) = (0..self.held_len).find(|&i| self.held[i] == note) {
            for j in pos..(self.held_len - 1) {
                self.held[j] = self.held[j + 1];
            }
            self.held[self.held_len - 1] = note;
        } else if self.held_len < self.held.len() {
            self.held[self.held_len] = note;
            self.held_len += 1;
        }
    }

    fn remove_held(&mut self, note: u8) {
        if let Some(pos) = (0..self.held_len).find(|&i| self.held[i] == note) {
            for j in pos..(self.held_len - 1) {
                self.held[j] = self.held[j + 1];
            }
            self.held_len -= 1;
        }
    }

    fn most_recent_held(&self) -> Option<u8> {
        if self.held_len == 0 {
            None
        } else {
            Some(self.held[self.held_len - 1])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vxn2_dsp::algo::N_OPS;
    use vxn2_dsp::eg::EgStage;
    use vxn2_dsp::op::OpParams;
    use vxn2_dsp::stack::{StackDistrib, stack_tick_mono, stack_tick_stereo};

    const SR: f32 = 48_000.0;
    const BLK: usize = 64;
    const BLK_DT: f32 = (BLK as f32) / SR;

    fn fast_patch() -> VoiceParams {
        let mut ops = [OpParams::default(); N_OPS];
        for op in &mut ops {
            op.eg.r[3] = 99;
        }
        VoiceParams {
            ops,
            algo: 32,
            ..VoiceParams::default()
        }
    }

    fn density1() -> StackParams {
        StackParams {
            density: 1,
            detune_cents_max: 0.0,
            spread: 0.0,
            phase: 0.0,
            distrib: StackDistrib::Linear,
        }
    }

    fn run_blocks(alloc: &mut PolyAlloc, blocks: usize) {
        for _ in 0..blocks {
            alloc.block_tick(BLK_DT);
            for s in &mut alloc.stacks {
                s.eg_tick(BLK_DT);
                for _ in 0..BLK {
                    let _ = stack_tick_mono(s);
                }
            }
        }
    }

    #[test]
    fn fresh_allocator_is_silent() {
        let alloc = PolyAlloc::new(SR);
        for s in &alloc.stacks {
            assert!(s.is_idle());
        }
    }

    #[test]
    fn clear_matches_fresh_and_next_note_identical() {
        let params = AllocParams::default();
        let sp = density1();
        let vp = fast_patch();

        // Perturb: gate several notes, advance, leave live state behind.
        let mut a = PolyAlloc::new(SR);
        a.set_sustain(true);
        for &n in &[48u8, 55, 60, 67] {
            a.note_on(&params, &sp, &vp, n, 100);
        }
        run_blocks(&mut a, 4);
        a.set_bend(1.5);
        assert!(a.stacks.iter().any(|s| !s.is_idle()), "precondition: voices live");

        a.clear();

        // Observable allocator state equals a fresh instance's.
        let fresh = PolyAlloc::new(SR);
        assert_eq!(a.sample_rate, fresh.sample_rate, "sample_rate preserved");
        assert_eq!(a.seq, fresh.seq);
        assert_eq!(a.next_seq, fresh.next_seq);
        assert_eq!(a.bend_st, fresh.bend_st);
        assert_eq!(a.held, fresh.held);
        assert_eq!(a.held_len, fresh.held_len);
        assert_eq!(a.solo_active, fresh.solo_active);
        assert_eq!(a.sustain, fresh.sustain);
        assert_eq!(a.held_by_pedal, fresh.held_by_pedal);
        assert!(a.glides.iter().all(|g| g.is_none()));
        for (s, f) in a.stacks.iter().zip(fresh.stacks.iter()) {
            assert!(s.is_idle());
            assert_eq!(s.meta.gate, f.meta.gate);
            assert_eq!(s.meta.note, f.meta.note);
        }

        // Next note-on after clear behaves identically to a fresh instance's.
        let mut b = PolyAlloc::new(SR);
        a.note_on(&params, &sp, &vp, 64, 100);
        b.note_on(&params, &sp, &vp, 64, 100);
        assert_eq!(a.seq, b.seq, "slot pick / seq after clear matches fresh");
        for (s, f) in a.stacks.iter().zip(b.stacks.iter()) {
            assert_eq!(s.meta.gate, f.meta.gate);
            assert_eq!(s.meta.note, f.meta.note);
        }
    }

    #[test]
    fn poly_note_on_picks_idle_slot() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams::default();
        let sp = density1();
        let vp = fast_patch();
        alloc.note_on(&params, &sp, &vp, 60, 100);
        assert!(alloc.stacks[0].meta.gate);
        assert_eq!(alloc.stacks[0].meta.note, 60);
        for s in &alloc.stacks[1..] {
            assert!(s.is_idle());
        }
    }

    #[test]
    fn poly_distinct_notes_use_distinct_slots() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams::default();
        let sp = density1();
        let vp = fast_patch();
        for n in 60..70 {
            alloc.note_on(&params, &sp, &vp, n, 100);
        }
        let active = alloc.stacks.iter().filter(|s| s.meta.gate).count();
        assert_eq!(active, 10);
    }

    #[test]
    fn poly_note_off_gates_matching_stack() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams::default();
        let sp = density1();
        let vp = fast_patch();
        alloc.note_on(&params, &sp, &vp, 60, 100);
        alloc.note_off(&params, &sp, &vp, 60);
        assert!(!alloc.stacks[0].meta.gate);
    }

    #[test]
    fn sustain_pedal_defers_poly_release() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams::default();
        let sp = density1();
        let vp = fast_patch();
        alloc.note_on(&params, &sp, &vp, 60, 100);
        alloc.set_sustain(true);
        alloc.note_off(&params, &sp, &vp, 60);
        // Pedal held: gate stays high, the stack keeps ringing.
        assert!(alloc.stacks[0].meta.gate);
        run_blocks(&mut alloc, (SR as usize) / 10 / BLK);
        assert!(!alloc.stacks[0].is_idle(), "held by pedal, must not free");
        // Pedal up: the deferred release fires and the tail frees the stack.
        alloc.set_sustain(false);
        assert!(!alloc.stacks[0].meta.gate);
        run_blocks(&mut alloc, (SR as usize) / 10 / BLK);
        assert!(alloc.stacks[0].is_idle());
    }

    #[test]
    fn sustain_pedal_off_with_key_still_down_keeps_note() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams::default();
        let sp = density1();
        let vp = fast_patch();
        alloc.note_on(&params, &sp, &vp, 60, 100);
        alloc.set_sustain(true);
        // Key never released; pedal-up must not gate a held key off.
        alloc.set_sustain(false);
        assert!(alloc.stacks[0].meta.gate);
    }

    #[test]
    fn idle_stacks_freed_after_release_tail() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams::default();
        let sp = density1();
        let vp = fast_patch();
        alloc.note_on(&params, &sp, &vp, 60, 100);
        alloc.note_off(&params, &sp, &vp, 60);
        run_blocks(&mut alloc, (SR as usize) / 10 / BLK);
        assert!(alloc.stacks[0].is_idle());
    }

    /// A note past the active cap holds polyphony at `N_ACTIVE`: the new note
    /// sounds, one voice is shed (declicked), and the count of owned voices is
    /// unchanged. Which voice is shed depends on the quietest-first rule + KS, so
    /// this asserts the invariant, not the slot.
    #[test]
    fn steal_caps_active_voices() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams::default();
        let sp = density1();
        let vp = fast_patch();
        for n in 60u8..(60 + N_ACTIVE as u8) {
            alloc.note_on(&params, &sp, &vp, n, 100);
        }
        run_blocks(&mut alloc, (SR as usize) / 20 / BLK);
        assert_eq!(alloc.active_count(), N_ACTIVE);
        alloc.note_on(&params, &sp, &vp, 90, 100);
        assert_eq!(alloc.active_count(), N_ACTIVE, "steal holds the cap");
        assert!(
            alloc.stacks.iter().any(|s| s.meta.gate && s.meta.note == 90),
            "new note sounding"
        );
    }

    #[test]
    fn solo_steal_uses_fresh_slot_and_declicks_previous() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams {
            assign_mode: AssignMode::Solo,
            legato: false,
            glide_time_ms: 0.0,
        };
        let sp = density1();
        let vp = fast_patch();
        alloc.note_on(&params, &sp, &vp, 60, 100);
        let first = alloc.solo_slot.expect("first note allocates a slot");
        alloc.note_on(&params, &sp, &vp, 64, 100);
        let cur = alloc.solo_slot.expect("second note allocates a slot");
        assert_ne!(cur, first, "a solo steal must round-robin to a fresh slot");
        assert!(alloc.stacks[cur].meta.gate);
        assert_eq!(alloc.stacks[cur].meta.note, 64);
        // The previous note is killed via the declick lifecycle (faded engine-side).
        assert_eq!(alloc.stacks[first].meta.phase, VoicePhase::Declick);
        assert_eq!(alloc.stacks[first].meta.note, 60);
    }

    #[test]
    fn solo_note_off_falls_back_to_held_note() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams {
            assign_mode: AssignMode::Solo,
            legato: false,
            glide_time_ms: 0.0,
        };
        let sp = density1();
        let vp = fast_patch();
        alloc.note_on(&params, &sp, &vp, 60, 100);
        alloc.note_on(&params, &sp, &vp, 64, 100);
        alloc.note_off(&params, &sp, &vp, 64);
        // Fallback to the still-held 60 lands on a fresh live (non-declicking) slot.
        let slot = alloc
            .stacks
            .iter()
            .position(|s| s.meta.gate && s.meta.note == 60 && s.meta.phase != VoicePhase::Declick)
            .expect("fallback note 60 is sounding");
        assert_eq!(alloc.solo_slot, Some(slot));
    }

    #[test]
    fn solo_legato_does_not_retrigger_eg() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams {
            assign_mode: AssignMode::Solo,
            legato: true,
            glide_time_ms: 0.0,
        };
        let sp = density1();
        let mut vp = fast_patch();
        for op in &mut vp.ops {
            op.eg.r = [99, 99, 99, 99];
        }
        alloc.note_on(&params, &sp, &vp, 60, 100);
        run_blocks(&mut alloc, (SR as usize) / BLK);
        assert_eq!(alloc.stacks[0].core.ops[0].eg.stage, EgStage::Sustain);
        alloc.note_on(&params, &sp, &vp, 64, 100);
        assert_eq!(alloc.stacks[0].core.ops[0].eg.stage, EgStage::Sustain);
        assert_eq!(alloc.stacks[0].meta.note, 64);
    }

    #[test]
    fn solo_non_legato_steal_is_fresh_voice_in_attack() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams {
            assign_mode: AssignMode::Solo,
            legato: false,
            glide_time_ms: 0.0,
        };
        let sp = density1();
        let vp = fast_patch();
        alloc.note_on(&params, &sp, &vp, 60, 100);
        let first = alloc.solo_slot.unwrap();
        run_blocks(&mut alloc, (SR as usize) / 5 / BLK);
        alloc.note_on(&params, &sp, &vp, 64, 100);
        let cur = alloc.solo_slot.unwrap();
        assert_ne!(cur, first);
        // The new note is a fresh voice (onset from silence → attack).
        assert_eq!(alloc.stacks[cur].core.ops[0].eg.stage, EgStage::Attack);
        // The previous note is declicking, not retriggered in place.
        assert_eq!(alloc.stacks[first].meta.phase, VoicePhase::Declick);
    }

    #[test]
    fn solo_glide_ramps_pitch_down() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams {
            assign_mode: AssignMode::Solo,
            legato: true,
            glide_time_ms: 200.0,
        };
        let sp = density1();
        let vp = fast_patch();
        alloc.note_on(&params, &sp, &vp, 72, 100);
        run_blocks(&mut alloc, (SR as usize) / 50 / BLK);
        alloc.note_on(&params, &sp, &vp, 60, 100);
        assert!(
            (alloc.stacks[0].meta.glide_st - 12.0).abs() < 1e-3,
            "glide_st should start at +12 st, got {}",
            alloc.stacks[0].meta.glide_st
        );
        run_blocks(&mut alloc, (100 * SR as usize) / 1000 / BLK);
        assert!(
            (alloc.stacks[0].meta.glide_st - 6.0).abs() < 1.0,
            "mid-glide should be ~+6, got {}",
            alloc.stacks[0].meta.glide_st
        );
        run_blocks(&mut alloc, (150 * SR as usize) / 1000 / BLK);
        assert!(
            alloc.stacks[0].meta.glide_st.abs() < 1e-3,
            "post-glide should be 0, got {}",
            alloc.stacks[0].meta.glide_st
        );
    }

    /// vxn-1 portamento parity (ticket 0125): in Solo mode glide fires on a
    /// *detached* note — previous key fully released and the slot idled before
    /// the next strike — not only on overlapping/slurred notes. Glide is
    /// governed by glide-time alone, independent of `legato`.
    #[test]
    fn solo_glide_on_detached_notes() {
        for legato in [false, true] {
            let mut alloc = PolyAlloc::new(SR);
            let params = AllocParams {
                assign_mode: AssignMode::Solo,
                legato,
                glide_time_ms: 200.0,
            };
            let sp = density1();
            let vp = fast_patch();
            // First note: snaps (no previous pitch to glide from).
            alloc.note_on(&params, &sp, &vp, 72, 100);
            run_blocks(&mut alloc, (SR as usize) / 50 / BLK);
            assert!(alloc.stacks[0].meta.glide_st.abs() < 1e-3, "first note must snap");
            // Release it and let the voice idle (slot frees, solo_active clears).
            alloc.note_off(&params, &sp, &vp, 72);
            run_blocks(&mut alloc, SR as usize / BLK);
            assert!(!alloc.solo_active, "fixture: detached gap must idle the slot");
            // Next note (no overlap) must still glide from the previous pitch.
            alloc.note_on(&params, &sp, &vp, 60, 100);
            assert!(
                (alloc.stacks[0].meta.glide_st - 12.0).abs() < 1e-3,
                "detached note (legato={legato}) must glide: glide_st={}",
                alloc.stacks[0].meta.glide_st
            );
        }
    }

    /// Poly always-glide is per-voice (vxn-1 parity, ticket 0125): a *reused*
    /// voice slides from its own previous pitch, regardless of `legato`. Play a
    /// note, release it so the slot idles, then play another — the nearest-pitch
    /// picker reuses the same slot and it glides from the old note.
    #[test]
    fn poly_reused_voice_glides() {
        for legato in [false, true] {
            let mut alloc = PolyAlloc::new(SR);
            let params = AllocParams {
                assign_mode: AssignMode::Poly,
                legato,
                glide_time_ms: 100.0,
            };
            let sp = density1();
            let vp = fast_patch();
            alloc.note_on(&params, &sp, &vp, 60, 100);
            run_blocks(&mut alloc, (SR as usize) / 50 / BLK);
            assert!(alloc.stacks[0].meta.glide_st.abs() < 1e-3, "first note must snap");
            alloc.note_off(&params, &sp, &vp, 60);
            run_blocks(&mut alloc, SR as usize / BLK);
            alloc.note_on(&params, &sp, &vp, 72, 100);
            let slot = alloc
                .stacks
                .iter()
                .position(|s| s.meta.gate && s.meta.note == 72)
                .expect("new note not allocated");
            assert_eq!(slot, 0, "nearest-pitch pick must reuse the freed voice");
            assert!(
                (alloc.stacks[slot].meta.glide_st - (-12.0)).abs() < 1e-3,
                "reused voice (legato={legato}) must glide: glide_st={}",
                alloc.stacks[slot].meta.glide_st
            );
        }
    }

    /// Poly steal at the active-voice cap: the victim is declicked *in place*
    /// (keeps its slot/filter, fades over `DECLICK_SECS`) and the new note takes
    /// a spare idle stack with a *fresh* attack from silence. Active voices stay
    /// capped at `N_ACTIVE`.
    #[test]
    fn poly_steal_declicks_victim_and_attacks_fresh() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams {
            assign_mode: AssignMode::Poly,
            legato: false,
            glide_time_ms: 0.0,
        };
        let sp = density1();
        let vp = fast_patch();
        for n in 60u8..(60 + N_ACTIVE as u8) {
            alloc.note_on(&params, &sp, &vp, n, 100);
        }
        run_blocks(&mut alloc, (SR as usize) / BLK); // all reach Sustain (Held)
        assert_eq!(alloc.active_count(), N_ACTIVE);
        // One more note → declick a victim, fresh-attack the new note elsewhere.
        alloc.note_on(&params, &sp, &vp, 90, 100);
        let new = alloc
            .stacks
            .iter()
            .position(|s| s.meta.gate && s.meta.note == 90)
            .expect("new note sounds 90");
        assert_eq!(alloc.stacks[new].meta.phase, VoicePhase::Held);
        assert_eq!(
            alloc.stacks[new].core.ops[0].eg.stage,
            EgStage::Attack,
            "new note is a fresh onset, not a reused voice"
        );
        assert!(
            alloc.stacks[new].core.ops[0].eg.level < 1.0e-3,
            "fresh attack starts from silence"
        );
        assert!(
            alloc.stacks.iter().any(|s| s.meta.phase == VoicePhase::Declick),
            "the stolen victim is declicking in place"
        );
        // Still exactly N_ACTIVE owned voices — the steal did not grow polyphony.
        assert_eq!(alloc.active_count(), N_ACTIVE);
    }

    /// A steal of a *Releasing* voice declicks the (quietest) tail and fresh-
    /// attacks the new note on a spare stack — no in-place reuse.
    #[test]
    fn poly_steal_of_releasing_declicks_and_attacks_fresh() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams {
            assign_mode: AssignMode::Poly,
            legato: false,
            glide_time_ms: 0.0,
        };
        let sp = density1();
        let vp = fast_patch();
        for n in 60u8..(60 + N_ACTIVE as u8) {
            alloc.note_on(&params, &sp, &vp, n, 100);
        }
        run_blocks(&mut alloc, (SR as usize) / BLK);
        for n in 60u8..(60 + N_ACTIVE as u8) {
            alloc.note_off(&params, &sp, &vp, n); // all → Releasing
        }
        assert_eq!(alloc.active_count(), N_ACTIVE, "releasing voices count as active");
        alloc.note_on(&params, &sp, &vp, 90, 100);
        let new = alloc
            .stacks
            .iter()
            .position(|s| s.meta.gate && s.meta.note == 90)
            .expect("new note sounds 90");
        assert_eq!(alloc.stacks[new].core.ops[0].eg.stage, EgStage::Attack);
        assert!(alloc.stacks[new].core.ops[0].eg.level < 1.0e-3, "fresh from silence");
        assert!(
            alloc.stacks.iter().any(|s| s.meta.phase == VoicePhase::Declick),
            "a releasing tail was declicked to make room"
        );
    }

    /// At the cap under a held sustain pedal, the victim is a pedal-sustained
    /// note (key already up) before a key the player is still holding. The
    /// pedal-held voice is declicked (its pedal flag cleared so a later pedal-up
    /// does not re-release it) and the new note attacks fresh; the held key
    /// survives.
    #[test]
    fn poly_steal_prefers_pedal_held_over_active_key() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams {
            assign_mode: AssignMode::Poly,
            legato: false,
            glide_time_ms: 0.0,
        };
        let sp = density1();
        let vp = fast_patch();
        // Fill the active cap; note 60 is the oldest.
        for n in 60u8..(60 + N_ACTIVE as u8) {
            alloc.note_on(&params, &sp, &vp, n, 100);
        }
        run_blocks(&mut alloc, (SR as usize) / BLK); // all reach Sustain (Held)
        // Pedal down, lift one *middle* key (70) — held by the pedal now, gate
        // high, physical key up. (Still N_ACTIVE active: pedal-held counts.)
        alloc.set_sustain(true);
        alloc.note_off(&params, &sp, &vp, 70);
        let pedal_slot = alloc
            .stacks
            .iter()
            .position(|s| s.meta.note == 70)
            .expect("note 70 sounding");
        assert!(alloc.held_by_pedal[pedal_slot], "70 deferred by pedal");
        assert_eq!(alloc.active_count(), N_ACTIVE);
        // At the cap → the victim must be the pedal-held 70, not the held 60.
        alloc.note_on(&params, &sp, &vp, 90, 100);
        assert_eq!(
            alloc.stacks[pedal_slot].meta.phase,
            VoicePhase::Declick,
            "the pedal-held voice is the victim"
        );
        assert!(
            !alloc.held_by_pedal[pedal_slot],
            "declicked victim dropped its pedal-hold flag"
        );
        // 60 (held key) survives; 90 sounds fresh.
        let new = alloc
            .stacks
            .iter()
            .position(|s| s.meta.gate && s.meta.note == 90)
            .expect("new note sounding");
        assert_eq!(alloc.stacks[new].core.ops[0].eg.stage, EgStage::Attack);
        assert!(
            alloc.stacks.iter().any(|s| s.meta.gate && s.meta.note == 60),
            "oldest held key must survive the steal"
        );
        // Pedal up: nothing flagged points at the declicked victim.
        alloc.set_sustain(false);
        assert!(!alloc.held_by_pedal[pedal_slot]);
    }

    /// At the cap, the victim is the *quietest* active voice (lowest carrier
    /// amplitude), so the declick is least audible. A note struck at near-zero
    /// velocity (high `vel_sens`, KS off) is declicked over louder, older notes.
    #[test]
    fn poly_steal_picks_quietest_voice() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams {
            assign_mode: AssignMode::Poly,
            legato: false,
            glide_time_ms: 0.0,
        };
        let sp = density1();
        let mut vp = fast_patch();
        // Velocity is the only loudness lever here: max `vel_sens`, KS off (so
        // pitch does not also scale the carrier level and confound the pick).
        for op in &mut vp.ops {
            op.vel_sens = 7;
            op.ks_l_depth = 0;
            op.ks_r_depth = 0;
        }
        // Fill the cap. Note 68 struck softly → quietest carrier; the rest hard.
        let quiet_note = 68u8;
        for n in 60u8..(60 + N_ACTIVE as u8) {
            let vel = if n == quiet_note { 1 } else { 127 };
            alloc.note_on(&params, &sp, &vp, n, vel);
        }
        run_blocks(&mut alloc, (SR as usize) / BLK); // all reach Sustain
        let quiet_slot = alloc
            .stacks
            .iter()
            .position(|s| s.meta.note == quiet_note)
            .expect("soft note sounding");
        let oldest_slot = alloc
            .stacks
            .iter()
            .position(|s| s.meta.note == 60)
            .expect("oldest note sounding");
        assert!(
            alloc.stacks[quiet_slot].carrier_level() < alloc.stacks[oldest_slot].carrier_level(),
            "fixture: soft note must be the quieter carrier"
        );
        // At the cap → declick the quietest (68), not the louder oldest (60).
        alloc.note_on(&params, &sp, &vp, 90, 100);
        assert_eq!(
            alloc.stacks[quiet_slot].meta.phase,
            VoicePhase::Declick,
            "quietest voice is the victim"
        );
        assert_eq!(
            alloc.stacks[oldest_slot].meta.note, 60,
            "louder oldest voice survives"
        );
        assert!(
            alloc.stacks[oldest_slot].meta.gate,
            "survivor still held"
        );
        assert!(
            alloc.stacks.iter().any(|s| s.meta.gate && s.meta.note == 90),
            "new note sounds fresh on a spare stack"
        );
    }

    /// Re-pressing a note already held by the sustain pedal retires the old
    /// voice (Declick) and starts the new note from a clean retrigger on a
    /// fresh slot. The new key, on later release, enters the sustain buffer
    /// in its own right.
    #[test]
    fn poly_repress_pedal_held_note_declicks_old_and_attacks_fresh() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams::default();
        let sp = density1();
        let vp = fast_patch();
        alloc.note_on(&params, &sp, &vp, 60, 100);
        let first_slot = alloc
            .stacks
            .iter()
            .position(|s| s.meta.gate && s.meta.note == 60)
            .expect("first 60 voice");
        run_blocks(&mut alloc, (SR as usize) / BLK); // reach Sustain (Held)
        alloc.set_sustain(true);
        alloc.note_off(&params, &sp, &vp, 60); // pedal-held now
        assert!(alloc.held_by_pedal[first_slot]);
        assert!(alloc.stacks[first_slot].meta.gate);

        alloc.note_on(&params, &sp, &vp, 60, 100);

        // Old voice declicking, no longer pedal-flagged.
        assert_eq!(
            alloc.stacks[first_slot].meta.phase,
            VoicePhase::Declick,
            "pedal-held dup retired into Declick"
        );
        assert!(
            !alloc.held_by_pedal[first_slot],
            "declicked slot cleared from sustain buffer"
        );
        // New voice on a different slot, fresh Attack from level 0.
        let new_slot = alloc
            .stacks
            .iter()
            .position(|s| s.meta.gate && s.meta.note == 60 && s.meta.phase == VoicePhase::Held)
            .expect("fresh 60 voice");
        assert_ne!(new_slot, first_slot, "new note took a fresh slot");
        assert_eq!(alloc.stacks[new_slot].core.ops[0].eg.stage, EgStage::Attack);
        assert!(
            alloc.stacks[new_slot].core.ops[0].eg.level < 1.0e-3,
            "fresh attack starts from 0, not legato-continued"
        );

        // Release the new key: still under pedal, so it joins the sustain buffer.
        alloc.note_off(&params, &sp, &vp, 60);
        assert!(
            alloc.held_by_pedal[new_slot],
            "new voice now in sustain buffer"
        );
        assert!(alloc.stacks[new_slot].meta.gate, "still held by pedal");

        // Pedal up gates the new voice off.
        alloc.set_sustain(false);
        assert!(!alloc.held_by_pedal[new_slot]);
    }

    /// Re-press of a pedal-held note retires that voice (declick) and starts the
    /// new note fresh on a spare stack — no doubling. With declick headroom a
    /// spare is always free, so the dup is declicked rather than reused in place.
    #[test]
    fn poly_repress_pedal_held_declicks_dup_attacks_fresh() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams::default();
        let sp = density1();
        let vp = fast_patch();
        for n in 60u8..(60 + N_ACTIVE as u8) {
            alloc.note_on(&params, &sp, &vp, n, 100);
        }
        run_blocks(&mut alloc, (SR as usize) / BLK);
        alloc.set_sustain(true);
        for n in 60u8..(60 + N_ACTIVE as u8) {
            alloc.note_off(&params, &sp, &vp, n); // all pedal-held
        }
        let dup_note = 65u8;
        let dup_slot = alloc
            .stacks
            .iter()
            .position(|s| s.meta.note == dup_note)
            .expect("dup note sounding");
        assert!(alloc.held_by_pedal[dup_slot]);

        alloc.note_on(&params, &sp, &vp, dup_note, 100);

        // The pedal-held dup is declicked (flag cleared); the new note attacks
        // fresh on a different stack. Exactly one *Held* voice sounds the note.
        assert_eq!(alloc.stacks[dup_slot].meta.phase, VoicePhase::Declick);
        assert!(!alloc.held_by_pedal[dup_slot], "declicked dup cleared its flag");
        let held_dup = alloc
            .stacks
            .iter()
            .filter(|s| s.meta.phase == VoicePhase::Held && s.meta.note == dup_note)
            .count();
        assert_eq!(held_dup, 1, "no doubling — one fresh voice for the re-press");
        let fresh = alloc
            .stacks
            .iter()
            .position(|s| s.meta.phase == VoicePhase::Held && s.meta.note == dup_note)
            .unwrap();
        assert_ne!(fresh, dup_slot, "fresh note took a spare stack");
        assert_eq!(alloc.stacks[fresh].core.ops[0].eg.stage, EgStage::Attack);
    }

    /// Steal burst exhausting the declick headroom: once every spare is mid-
    /// declick, a further steal falls back to in-place reuse of the victim. No
    /// panic, polyphony stays at the cap, output stays finite.
    #[test]
    fn poly_steal_burst_falls_back_in_place() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams::default();
        let sp = density1();
        let vp = fast_patch();
        for n in 60u8..(60 + N_ACTIVE as u8) {
            alloc.note_on(&params, &sp, &vp, n, 100);
        }
        run_blocks(&mut alloc, (SR as usize) / BLK);
        // Fire more steals than the headroom with NO block_tick between, so the
        // declick lanes never free — the last few hit the in-place fallback.
        for n in 90u8..(90 + N_DECLICK as u8 + 3) {
            alloc.note_on(&params, &sp, &vp, n, 100);
            assert!(alloc.active_count() <= N_ACTIVE, "cap never exceeded");
        }
        // Render: no panic, finite output.
        let mut peak = 0.0_f32;
        for _ in 0..64 {
            alloc.block_tick(BLK_DT);
            for s in &mut alloc.stacks {
                s.eg_tick(BLK_DT);
                for _ in 0..BLK {
                    let (l, r) = stack_tick_stereo(s);
                    assert!(l.is_finite() && r.is_finite());
                    peak = peak.max(l.abs()).max(r.abs());
                }
            }
        }
        assert!(peak < 40.0, "peak too high: {peak}");
    }

    /// Overlapping distinct notes (a chord) take *fresh* voices and must not
    /// glide between each other — only same-voice reuse glides (ticket 0125).
    #[test]
    fn poly_overlapping_chord_notes_do_not_glide() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams {
            assign_mode: AssignMode::Poly,
            legato: true,
            glide_time_ms: 100.0,
        };
        let sp = density1();
        let vp = fast_patch();
        alloc.note_on(&params, &sp, &vp, 60, 100);
        run_blocks(&mut alloc, (SR as usize) / 50 / BLK);
        alloc.note_on(&params, &sp, &vp, 72, 100);
        let new_slot = alloc
            .stacks
            .iter()
            .position(|s| s.meta.gate && s.meta.note == 72)
            .expect("new note not allocated");
        assert_ne!(new_slot, 0, "overlapping note must take a fresh voice");
        assert!(
            alloc.stacks[new_slot].meta.glide_st.abs() < 1e-3,
            "fresh voice must not glide from another voice: glide_st={}",
            alloc.stacks[new_slot].meta.glide_st
        );
    }

    /// Glide-time 0 disables glide entirely, even on a reused voice.
    #[test]
    fn poly_zero_glide_time_never_glides() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams {
            assign_mode: AssignMode::Poly,
            legato: false,
            glide_time_ms: 0.0,
        };
        let sp = density1();
        let vp = fast_patch();
        alloc.note_on(&params, &sp, &vp, 60, 100);
        alloc.note_off(&params, &sp, &vp, 60);
        run_blocks(&mut alloc, SR as usize / BLK);
        alloc.note_on(&params, &sp, &vp, 72, 100);
        for s in &alloc.stacks {
            assert_eq!(s.meta.glide_st, 0.0);
        }
    }

    #[test]
    fn poly_to_solo_switch_releases_held_gates() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams::default();
        let sp = density1();
        let vp = fast_patch();
        // Hold a chord in Poly.
        for n in 60u8..64 {
            alloc.note_on(&params, &sp, &vp, n, 100);
        }
        run_blocks(&mut alloc, (SR as usize) / 50 / BLK);
        assert_eq!(alloc.stacks.iter().filter(|s| s.meta.gate).count(), 4);
        // Flip to Solo: every held gate releases.
        alloc.set_mode(AssignMode::Solo);
        assert!(alloc.stacks.iter().all(|s| !s.meta.gate), "all gates released");
        assert_eq!(alloc.held_len, 0, "held-note stack cleared");
        assert!(!alloc.solo_active);
        assert_eq!(alloc.solo_slot, None);
    }

    #[test]
    fn poly_to_solo_switch_releases_pedal_held_gates() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams::default();
        let sp = density1();
        let vp = fast_patch();
        alloc.note_on(&params, &sp, &vp, 60, 100);
        alloc.set_sustain(true);
        alloc.note_off(&params, &sp, &vp, 60); // deferred by pedal, gate stays high
        assert!(alloc.stacks[0].meta.gate);
        alloc.set_mode(AssignMode::Solo);
        assert!(!alloc.stacks[0].meta.gate, "pedal-held gate released on solo switch");
        assert!(!alloc.held_by_pedal[0]);
    }

    #[test]
    fn solo_to_poly_switch_leaves_voice_sounding() {
        let mut alloc = PolyAlloc::new(SR);
        let solo = AllocParams {
            assign_mode: AssignMode::Solo,
            legato: false,
            glide_time_ms: 0.0,
        };
        let sp = density1();
        let vp = fast_patch();
        alloc.set_mode(AssignMode::Solo);
        alloc.note_on(&solo, &sp, &vp, 60, 100);
        let slot = alloc.solo_slot.expect("solo voice live");
        assert!(alloc.stacks[slot].meta.gate);
        // Solo → Poly must not kill the live voice.
        alloc.set_mode(AssignMode::Poly);
        assert!(alloc.stacks[slot].meta.gate, "solo→poly keeps the live voice");
    }

    #[test]
    fn bend_forwards_to_every_stack() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams::default();
        let sp = density1();
        let vp = fast_patch();
        for n in 60u8..68 {
            alloc.note_on(&params, &sp, &vp, n, 100);
        }
        alloc.set_bend(2.0);
        for s in &alloc.stacks {
            assert_eq!(s.meta.bend_st, 2.0);
        }
    }

    #[test]
    fn bend_persists_through_subsequent_note_on() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams::default();
        let sp = density1();
        let vp = fast_patch();
        alloc.set_bend(1.0);
        alloc.note_on(&params, &sp, &vp, 60, 100);
        assert_eq!(alloc.stacks[0].meta.bend_st, 1.0);
    }

    #[test]
    fn sixteen_held_plus_one_no_panic_finite_output() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams::default();
        let sp = density1();
        let vp = fast_patch();
        for n in 60u8..76 {
            alloc.note_on(&params, &sp, &vp, n, 100);
        }
        run_blocks(&mut alloc, (SR as usize) / 50 / BLK);
        alloc.note_on(&params, &sp, &vp, 90, 100);
        let mut peak = 0.0_f32;
        for _ in 0..200 {
            alloc.block_tick(BLK_DT);
            for s in &mut alloc.stacks {
                s.eg_tick(BLK_DT);
                for _ in 0..BLK {
                    let (l, r) = stack_tick_stereo(s);
                    assert!(l.is_finite() && r.is_finite());
                    let m = l.abs().max(r.abs());
                    if m > peak {
                        peak = m;
                    }
                }
            }
        }
        assert!(peak < 40.0, "peak too high: {peak}");
    }

    #[test]
    fn density_8_stack_uses_one_slot() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams::default();
        let sp = StackParams {
            density: 8,
            detune_cents_max: 8.0,
            spread: 0.6,
            phase: 0.5,
            distrib: StackDistrib::Linear,
        };
        let vp = fast_patch();
        alloc.note_on(&params, &sp, &vp, 60, 100);
        // One stack carrying 8 lanes — only slot 0 is gated.
        let active = alloc.stacks.iter().filter(|s| s.meta.gate).count();
        assert_eq!(active, 1);
        assert_eq!(alloc.stacks[0].meta.density, 8);
    }

    #[test]
    fn note_off_releases_all_lanes_in_stack() {
        // 0005 acceptance: at note_off, all stacked instances gate to release
        // together. Since one Stack holds all lanes, gating it gates them all.
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams::default();
        let sp = StackParams {
            density: 8,
            ..StackParams::default()
        };
        let vp = fast_patch();
        alloc.note_on(&params, &sp, &vp, 60, 100);
        alloc.note_off(&params, &sp, &vp, 60);
        assert!(!alloc.stacks[0].meta.gate);
        for op in &alloc.stacks[0].core.ops {
            assert_eq!(op.eg.stage, EgStage::Release);
        }
    }
}
