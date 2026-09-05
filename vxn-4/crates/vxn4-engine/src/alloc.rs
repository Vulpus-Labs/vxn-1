//! Voice allocation: 16 slots of explicit polyphony plus 4 spares that only
//! ever hold declick tails.
//!
//! The behaviour is vxn-2's ([`vxn2_engine::alloc`], ADR §3), reimplemented
//! against vxn-4's voice model rather than ported — vxn-2 allocates *stacks* of
//! lane-packed operator instances with glide, solo mode, a sustain pedal and
//! pitch bend, none of which vxn-4 has yet. What carries over is the part that
//! matters for how it feels to play:
//!
//! - **16 active voices.** A voice counts as active while `Held` or
//!   `Releasing`. Declicking voices do not count, so a burst of steals cannot
//!   eat the polyphony budget with tails.
//! - **4 spare slots.** A stolen voice is declicked *in place*, keeping its own
//!   state so its tail rings out continuously, while the new note starts clean
//!   on a spare. This is the whole reason for the spares: hard-reusing a
//!   sounding slot clicks, and fading it out before reusing it costs latency.
//! - **Quietest-voice stealing, key-up first.** A voice whose key is already up
//!   (`Releasing`) is shed before one the player is still holding; within each
//!   group the quietest goes, ties broken by age. Stealing the quietest makes
//!   the declick least audible, since a near-silent tail is already inaudible.
//!
//! Deliberately not carried over: glide, solo/legato, sustain pedal, pitch
//! bend. Those are vxn-2 features with their own semantics to get right, and
//! the brief asked for note selection and trimming, not the rest of the
//! keyboard model. [`Voice::pitch`] is a `f32` in semitones so glide and bend
//! have somewhere to land later without a signature change.
//!
//! No allocation and no panics on the audio thread.

use crate::eg::{Eg, EgParams};
use vxn4_dsp::ops::NOPS;

/// Explicit polyphony. A 17th simultaneous note declicks the quietest voice.
pub const N_ACTIVE: usize = 16;

/// Spare slots above the cap, holding only declick tails.
pub const N_DECLICK: usize = 4;

/// Physical slot count — what the engine actually renders.
pub const N_SLOTS: usize = N_ACTIVE + N_DECLICK;

/// Declick fade time. Long enough not to click, short enough that a spare is
/// back in the pool before a fast player needs it.
pub const DECLICK_SECS: f32 = 0.005;

const IDLE_SEQ: u64 = u64::MAX;

/// Voice lifecycle. The authoritative liveness state.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Phase {
    /// Free. Not rendered, available immediately.
    #[default]
    Idle,
    /// Key down.
    Held,
    /// Key up, envelopes in their release segment.
    Releasing,
    /// Stolen. Fading to silence over [`DECLICK_SECS`], then Idle.
    Declick,
}

/// One allocated voice. The DSP state lives in the engine's operator banks;
/// this is the bookkeeping that decides which lane gets which note.
#[derive(Clone, Copy, Debug)]
pub struct Voice {
    pub phase: Phase,
    pub note: u8,
    /// Sounding pitch in semitones. Equal to `note` today; the place glide,
    /// bend and detune will land.
    pub pitch: f32,
    pub velocity: u8,
    /// One envelope per operator.
    pub eg: [Eg; NOPS],
    /// Cached loudest sum-bus-weighted level, refreshed each control tick.
    /// The steal decision reads this rather than the DSP banks, so picking a
    /// victim touches no audio state.
    pub amp: f32,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            note: 0,
            pitch: 0.0,
            velocity: 0,
            eg: [Eg::default(); NOPS],
            amp: 0.0,
        }
    }
}

impl Voice {
    pub fn is_idle(&self) -> bool {
        self.phase == Phase::Idle
    }

    /// Active = a voice the player owns and hears. Excludes idle slots and
    /// declick tails, which is what keeps the [`N_ACTIVE`] cap honest.
    pub fn is_active(&self) -> bool {
        matches!(self.phase, Phase::Held | Phase::Releasing)
    }
}

/// What the engine must do to the DSP banks as a result of an allocation
/// decision. Returned rather than performed, so the allocator stays free of
/// audio state and is testable on its own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Fresh onset on `slot`: reset phases and history, then cook the pitch.
    Start { slot: usize },
    /// Re-pitch `slot` without resetting phase — the burst fallback, where no
    /// spare was free and the victim had to be reused in place.
    Reuse { slot: usize },
}

pub struct Alloc {
    pub voices: [Voice; N_SLOTS],
    /// Monotonic note-on counter per slot; [`IDLE_SEQ`] when free.
    seq: [u64; N_SLOTS],
    next_seq: u64,
    /// Whether a slot has ever sounded. Unvoiced slots are preferred for fresh
    /// notes, since there is nothing to glide from.
    voiced: [bool; N_SLOTS],
}

impl Default for Alloc {
    fn default() -> Self {
        Self::new()
    }
}

impl Alloc {
    pub fn new() -> Self {
        Self {
            voices: [Voice::default(); N_SLOTS],
            seq: [IDLE_SEQ; N_SLOTS],
            next_seq: 0,
            voiced: [false; N_SLOTS],
        }
    }

    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// Count of active (Held or Releasing) voices.
    pub fn active_count(&self) -> usize {
        self.voices.iter().filter(|v| v.is_active()).count()
    }

    pub fn sounding_count(&self) -> usize {
        self.voices.iter().filter(|v| !v.is_idle()).count()
    }

    /// Allocate a note. Returns the action the engine must apply to the banks.
    ///
    /// The two decisions are independent and both are made before anything is
    /// mutated: *who dies* (a victim, only if at the cap) and *who plays* (a
    /// spare, almost always available). Only when both land on the same slot,
    /// or no spare exists at all, does the new note reuse a sounding lane.
    pub fn note_on(&mut self, eg_params: &[EgParams; NOPS], note: u8, velocity: u8) -> Action {
        let victim = if self.active_count() >= N_ACTIVE {
            self.pick_victim()
        } else {
            None
        };
        let idle = self.pick_idle();

        let (slot, fresh) = match (victim, idle) {
            // The common steal: fade the victim where it stands, start clean on
            // a spare. The victim keeps its own state, so its tail is continuous.
            (Some(v), Some(s)) if v != s => {
                self.start_declick(v);
                (s, true)
            }
            (_, Some(s)) => (s, true),
            // Burst fallback: every spare is mid-declick. Reuse the victim in
            // place and retrigger its envelopes from their current level rather
            // than from zero, which is click-free even though it is not clean.
            (Some(v), None) => (v, false),
            (None, None) => (self.pick_victim().unwrap_or(0), false),
        };

        let counter = self.next_seq;
        self.next_seq += 1;
        self.seq[slot] = counter;
        self.voiced[slot] = true;

        let peak = velocity_to_peak(velocity);
        let v = &mut self.voices[slot];
        v.phase = Phase::Held;
        v.note = note;
        v.pitch = note as f32;
        v.velocity = velocity;
        for (eg, p) in v.eg.iter_mut().zip(eg_params.iter()) {
            if fresh {
                eg.clear();
            }
            eg.cook(p, peak);
            eg.note_on();
        }

        if fresh {
            Action::Start { slot }
        } else {
            Action::Reuse { slot }
        }
    }

    /// Release every held voice matching `note`.
    pub fn note_off(&mut self, note: u8) {
        for v in self.voices.iter_mut() {
            if v.phase == Phase::Held && v.note == note {
                v.phase = Phase::Releasing;
                for eg in v.eg.iter_mut() {
                    eg.note_off();
                }
            }
        }
    }

    pub fn all_notes_off(&mut self) {
        for i in 0..N_SLOTS {
            if self.voices[i].is_active() {
                self.voices[i].phase = Phase::Releasing;
                for eg in self.voices[i].eg.iter_mut() {
                    eg.note_off();
                }
            }
        }
    }

    /// Begin a declick fade on `slot`.
    ///
    /// Every operator gets the same wall-clock deadline, so the voice collapses
    /// evenly rather than shedding operators one at a time — a staggered
    /// collapse reads as a timbre sweep, not a fade.
    fn start_declick(&mut self, slot: usize) {
        self.voices[slot].phase = Phase::Declick;
        for eg in self.voices[slot].eg.iter_mut() {
            eg.kill(DECLICK_SECS);
        }
    }

    /// Advance every voice's envelopes by one control tick, refresh the cached
    /// amplitudes, and retire voices whose envelopes have all gone idle.
    ///
    /// Returns a bitmask of slots that became idle this tick, so the engine can
    /// silence their lanes rather than leaving a stale tail in the history ring.
    pub fn control_tick(&mut self, dt: f32, bus_weight: &[f32; NOPS]) -> u32 {
        let mut retired = 0u32;
        for i in 0..N_SLOTS {
            let v = &mut self.voices[i];
            if v.phase == Phase::Idle {
                continue;
            }
            let mut amp = 0.0f32;
            let mut all_idle = true;
            for (eg, w) in v.eg.iter_mut().zip(bus_weight.iter()) {
                let level = eg.tick(dt);
                amp = amp.max(level * *w);
                all_idle &= eg.is_idle();
            }
            v.amp = amp;
            if all_idle {
                v.phase = Phase::Idle;
                v.amp = 0.0;
                self.seq[i] = IDLE_SEQ;
                retired |= 1 << i;
            }
        }
        retired
    }

    /// Pick a free slot for a new note.
    ///
    /// Unvoiced slots first, so a fresh instrument spreads across lanes before
    /// reusing any — which keeps successive notes from sharing a lane's phase
    /// history. After that, the least recently used, so a reused lane is the
    /// one whose tail is longest gone.
    fn pick_idle(&self) -> Option<usize> {
        let mut oldest: Option<(usize, u64)> = None;
        for i in 0..N_SLOTS {
            if !self.voices[i].is_idle() {
                continue;
            }
            if !self.voiced[i] {
                return Some(i);
            }
            let age = self.seq[i];
            if oldest.is_none_or(|(_, best)| age < best) {
                oldest = Some((i, age));
            }
        }
        oldest.map(|(i, _)| i)
    }

    /// Pick the voice to retire at the cap: the quietest active one, preferring
    /// those whose key is already up, ties broken by age.
    fn pick_victim(&self) -> Option<usize> {
        let mut best: Option<(usize, f32, u64)> = None;
        let mut best_keyup: Option<(usize, f32, u64)> = None;
        let quieter = |cand: (usize, f32, u64), b: Option<(usize, f32, u64)>| match b {
            Some(b) if b.1 < cand.1 || (b.1 == cand.1 && b.2 <= cand.2) => b,
            _ => cand,
        };
        for i in 0..N_SLOTS {
            if !self.voices[i].is_active() {
                continue;
            }
            let cand = (i, self.voices[i].amp, self.seq[i]);
            best = Some(quieter(cand, best));
            if self.voices[i].phase == Phase::Releasing {
                best_keyup = Some(quieter(cand, best_keyup));
            }
        }
        best_keyup.or(best).map(|(i, _, _)| i)
    }
}

/// Velocity curve. Squared, which is the usual perceptual fit and matches what
/// vxn-2 does at the top of its level ladder.
#[inline]
pub fn velocity_to_peak(velocity: u8) -> f32 {
    let v = (velocity as f32 / 127.0).clamp(0.0, 1.0);
    v * v
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 64.0 / 48_000.0;

    fn params() -> [EgParams; NOPS] {
        [EgParams::adsr(0.001, 0.05, 0.8, 0.05); NOPS]
    }

    fn weights() -> [f32; NOPS] {
        [1.0; NOPS]
    }

    fn settle(a: &mut Alloc, secs: f32) {
        for _ in 0..((secs / DT) as usize) {
            a.control_tick(DT, &weights());
        }
    }

    #[test]
    fn fresh_allocator_is_silent() {
        let a = Alloc::new();
        assert_eq!(a.active_count(), 0);
        assert_eq!(a.sounding_count(), 0);
        assert!(a.voices.iter().all(|v| v.is_idle()));
    }

    #[test]
    fn distinct_notes_take_distinct_slots() {
        let mut a = Alloc::new();
        let mut seen = Vec::new();
        for n in 60..72u8 {
            match a.note_on(&params(), n, 100) {
                Action::Start { slot } => seen.push(slot),
                other => panic!("expected a fresh start, got {other:?}"),
            }
        }
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 12, "slots were reused while free ones existed");
    }

    #[test]
    fn sixteen_notes_fit_without_stealing() {
        let mut a = Alloc::new();
        for n in 0..N_ACTIVE {
            a.note_on(&params(), 48 + n as u8, 100);
            a.control_tick(DT, &weights());
        }
        assert_eq!(a.active_count(), N_ACTIVE);
        assert!(
            a.voices.iter().all(|v| v.phase != Phase::Declick),
            "nothing should have been stolen at exactly the cap"
        );
    }

    /// The 17th note steals, and the stolen voice must be *declicking* rather
    /// than snapped to idle — that is what makes the steal inaudible.
    #[test]
    fn seventeenth_note_steals_into_a_declick() {
        let mut a = Alloc::new();
        for n in 0..N_ACTIVE {
            a.note_on(&params(), 48 + n as u8, 100);
            a.control_tick(DT, &weights());
        }
        settle(&mut a, 0.2);
        let action = a.note_on(&params(), 90, 100);
        assert!(matches!(action, Action::Start { .. }), "{action:?}");
        assert_eq!(
            a.voices.iter().filter(|v| v.phase == Phase::Declick).count(),
            1
        );
        assert_eq!(a.active_count(), N_ACTIVE, "cap must hold after the steal");
    }

    /// The declick tail must not occupy the polyphony budget.
    #[test]
    fn declick_tails_do_not_count_against_the_cap() {
        let mut a = Alloc::new();
        for n in 0..N_ACTIVE {
            a.note_on(&params(), 48 + n as u8, 100);
        }
        settle(&mut a, 0.2);
        for n in 0..4u8 {
            a.note_on(&params(), 90 + n, 100);
        }
        assert_eq!(a.active_count(), N_ACTIVE);
        assert!(a.sounding_count() > N_ACTIVE, "tails should still be sounding");
        assert!(a.sounding_count() <= N_SLOTS);
    }

    /// The steal must prefer a released voice over one the player is holding.
    #[test]
    fn stealing_prefers_a_key_that_is_already_up() {
        let mut a = Alloc::new();
        for n in 0..N_ACTIVE {
            a.note_on(&params(), 48 + n as u8, 100);
        }
        settle(&mut a, 0.02);
        // Release one mid-register note; it stays active (Releasing) but its key
        // is up, so it should be the victim even though louder notes exist.
        a.note_off(52);
        settle(&mut a, 0.001);
        let released = a
            .voices
            .iter()
            .position(|v| v.note == 52 && v.phase == Phase::Releasing)
            .expect("note 52 should be releasing");

        a.note_on(&params(), 90, 100);
        assert_eq!(
            a.voices[released].phase,
            Phase::Declick,
            "the released voice should have been stolen first"
        );
    }

    /// Among equals, the quietest goes — that is what makes the fade least
    /// audible.
    #[test]
    fn stealing_takes_the_quietest_held_voice() {
        let mut a = Alloc::new();
        // Fill the cap, with one note markedly quieter than the rest.
        for n in 0..N_ACTIVE {
            let vel = if n == 5 { 12 } else { 127 };
            a.note_on(&params(), 48 + n as u8, vel);
        }
        settle(&mut a, 0.2);
        let quiet = a.voices.iter().position(|v| v.note == 53).unwrap();
        a.note_on(&params(), 90, 100);
        assert_eq!(
            a.voices[quiet].phase,
            Phase::Declick,
            "expected the quietest voice to be stolen"
        );
    }

    /// Exhausting every spare must degrade to in-place reuse, not to a panic or
    /// a dropped note.
    #[test]
    fn a_steal_burst_falls_back_to_reuse_rather_than_dropping_notes() {
        let mut a = Alloc::new();
        for n in 0..N_ACTIVE {
            a.note_on(&params(), 48 + n as u8, 100);
        }
        settle(&mut a, 0.2);
        // No control ticks between these, so no declick can finish and no spare
        // can come back.
        let mut reused = 0;
        for n in 0..12u8 {
            match a.note_on(&params(), 90 + n, 100) {
                Action::Start { .. } => {}
                Action::Reuse { .. } => reused += 1,
            }
        }
        assert!(reused > 0, "expected the spares to run out");
        assert_eq!(a.active_count(), N_ACTIVE, "cap held through the burst");
    }

    #[test]
    fn voices_retire_after_release_and_free_their_slots() {
        let mut a = Alloc::new();
        a.note_on(&params(), 60, 100);
        settle(&mut a, 0.2);
        assert_eq!(a.active_count(), 1);
        a.note_off(60);
        settle(&mut a, 0.5);
        assert_eq!(a.active_count(), 0);
        assert_eq!(a.sounding_count(), 0);
    }

    #[test]
    fn control_tick_reports_retired_slots() {
        let mut a = Alloc::new();
        let slot = match a.note_on(&params(), 60, 100) {
            Action::Start { slot } => slot,
            other => panic!("{other:?}"),
        };
        settle(&mut a, 0.2);
        a.note_off(60);
        let mut mask = 0u32;
        for _ in 0..((0.5 / DT) as usize) {
            mask |= a.control_tick(DT, &weights());
        }
        assert_eq!(mask & (1 << slot), 1 << slot, "slot {slot} never reported");
    }

    #[test]
    fn note_off_for_an_unheld_note_is_harmless() {
        let mut a = Alloc::new();
        a.note_on(&params(), 60, 100);
        a.note_off(61);
        assert_eq!(a.active_count(), 1);
        assert_eq!(a.voices.iter().filter(|v| v.phase == Phase::Held).count(), 1);
    }

    /// A declick must actually finish and hand the slot back, or the spares
    /// leak and every steal degrades to in-place reuse.
    #[test]
    fn declicked_slots_return_to_the_pool() {
        let mut a = Alloc::new();
        for n in 0..N_ACTIVE {
            a.note_on(&params(), 48 + n as u8, 100);
        }
        settle(&mut a, 0.2);
        a.note_on(&params(), 90, 100);
        assert_eq!(a.voices.iter().filter(|v| v.phase == Phase::Declick).count(), 1);
        settle(&mut a, DECLICK_SECS * 4.0);
        assert_eq!(
            a.voices.iter().filter(|v| v.phase == Phase::Declick).count(),
            0,
            "declick never completed"
        );
    }

    #[test]
    fn velocity_curve_is_monotonic_and_bounded() {
        assert_eq!(velocity_to_peak(0), 0.0);
        assert!((velocity_to_peak(127) - 1.0).abs() < 1e-6);
        let mut prev = -1.0;
        for v in 0..=127u8 {
            let p = velocity_to_peak(v);
            assert!(p >= prev, "not monotonic at {v}");
            assert!((0.0..=1.0).contains(&p));
            prev = p;
        }
    }
}
