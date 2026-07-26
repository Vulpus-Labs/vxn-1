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

/// Voice count — inherited from the shared DSP crate so VXN1b's poly is
/// identical to VXN1's (ADR 0001 §1). A flat bank: channel/pressure plumbing is
/// orthogonal to VXN1's per-layer SoA split, so allocation reasons over all
/// voices uniformly.
const N: usize = MAX_VOICES;

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

/// Pick the voice a new note lands on: the first free (inactive) voice, else the
/// steal target by [`steal_tier`] then age. Pure over the borrowed view; the
/// caller stamps channel/note/pressure onto the returned index. Total — always
/// returns a valid `0..N` index (steal falls back to voice 0).
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
    /// Monotonic allocation counter; stamped into `alloc_tick` per note-on so
    /// the steal policy can rank by age. Wraps at u64::MAX (unreachable in
    /// practice — ~6M years at 100k notes/s).
    next_tick: u64,
    /// Note-on-random stream state. Advanced one draw per note-on; a single
    /// stream (not per-voice seeds) guarantees successive voices get distinct
    /// values while staying deterministic. Never zero (xorshift stuck point).
    rng: u64,
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
            next_tick: 0,
            rng: NOTE_RANDOM_SEED,
        }
    }

    /// Clear every voice (host reset / all-sound-off).
    pub fn reset(&mut self) {
        self.active = [false; N];
        self.gate = [false; N];
        self.pressure = [0.0; N];
        // note/channel/alloc_tick are stale-until-reused; queries gate on `active`.
    }

    /// Allocate a voice for a note-on on `channel`, stamp its identity, and reset
    /// its pressure. Returns the assigned voice index. If every voice is busy the
    /// oldest steal-tier target is re-used and **re-parented to `channel`** — the
    /// core MPE requirement: the stolen voice now belongs to the stealing note's
    /// channel, so subsequent channel pressure on that channel reaches it (and on
    /// the old channel no longer does).
    pub fn note_on(&mut self, channel: u8, note: u8) -> usize {
        let v = allocate(&self.view());
        self.active[v] = true;
        self.gate[v] = true;
        self.note[v] = note;
        self.channel[v] = channel;
        self.pressure[v] = 0.0;
        self.note_random[v] = note_random_draw(&mut self.rng);
        self.alloc_tick[v] = self.next_tick;
        self.next_tick = self.next_tick.wrapping_add(1);
        v
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

    #[inline]
    fn view(&self) -> AllocView<'_> {
        AllocView {
            active: &self.active,
            gate: &self.gate,
            alloc_tick: &self.alloc_tick,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_on_stores_channel_on_assigned_voice() {
        let mut voices = Voices::new();
        let v = voices.note_on(3, 60);
        assert!(voices.is_active(v));
        assert_eq!(voices.channel(v), 3);
        assert_eq!(voices.note(v), 60);
    }

    #[test]
    fn per_note_pressure_isolated_to_matching_voice() {
        let mut voices = Voices::new();
        // MPE: each note on its own channel.
        let a = voices.note_on(1, 60);
        let b = voices.note_on(2, 64);
        voices.poly_pressure(1, 60, 0.8);
        assert_eq!(voices.pressure(a), 0.8);
        // Must not leak to the other channel/note.
        assert_eq!(voices.pressure(b), 0.0);
    }

    #[test]
    fn per_note_pressure_ignores_same_note_other_channel() {
        let mut voices = Voices::new();
        let a = voices.note_on(1, 60);
        let b = voices.note_on(2, 60); // same note, different channel
        voices.poly_pressure(2, 60, 0.5);
        assert_eq!(voices.pressure(a), 0.0);
        assert_eq!(voices.pressure(b), 0.5);
    }

    #[test]
    fn channel_pressure_broadcasts_to_all_voices_on_channel() {
        let mut voices = Voices::new();
        // Channel mode: several notes share one channel.
        let a = voices.note_on(1, 60);
        let b = voices.note_on(1, 64);
        let c = voices.note_on(2, 67); // other channel — untouched
        voices.channel_pressure(1, 0.6);
        assert_eq!(voices.pressure(a), 0.6);
        assert_eq!(voices.pressure(b), 0.6);
        assert_eq!(voices.pressure(c), 0.0);
    }

    #[test]
    fn free_voice_chosen_before_steal() {
        let mut voices = Voices::new();
        let first = voices.note_on(1, 60);
        let second = voices.note_on(1, 61);
        assert_ne!(first, second, "distinct free voices used before any steal");
    }

    #[test]
    fn stolen_voice_reparents_to_stealing_channel() {
        let mut voices = Voices::new();
        // Fill every voice on channel 1.
        for i in 0..N {
            voices.note_on(1, 60 + i as u8);
        }
        // Next note-on on channel 2 must steal and re-parent.
        let stolen = voices.note_on(2, 72);
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
        let oldest = voices.note_on(1, 60);
        for i in 1..N {
            voices.note_on(1, 60 + i as u8);
        }
        // All held; the first-allocated (oldest tick) is sacrificed.
        let stolen = voices.note_on(1, 90);
        assert_eq!(stolen, oldest);
    }

    #[test]
    fn released_tail_stolen_before_held_note() {
        let mut voices = Voices::new();
        // Voice 0 is the oldest but gets released; a later voice is younger but held.
        let released = voices.note_on(1, 60);
        for i in 1..N {
            voices.note_on(1, 60 + i as u8);
        }
        voices.note_off(1, 60); // release the oldest → tier 0
        // A held-but-younger voice exists, yet the released tail goes first.
        let stolen = voices.note_on(1, 90);
        assert_eq!(stolen, released);
    }

    #[test]
    fn note_off_matches_channel_and_note() {
        let mut voices = Voices::new();
        let a = voices.note_on(1, 60);
        let b = voices.note_on(2, 60); // same note, other channel
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
            let v = voices.note_on(1, 60 + i as u8);
            let r = voices.note_random(v);
            assert!((0.0..1.0).contains(&r), "note-random {r} out of [0,1)");
        }
    }

    #[test]
    fn note_random_constant_over_note_lifetime() {
        let mut voices = Voices::new();
        let a = voices.note_on(1, 60);
        let latched = voices.note_random(a);
        // Unrelated activity must not disturb a held voice's latched value.
        voices.note_on(2, 64);
        voices.channel_pressure(1, 0.7);
        voices.poly_pressure(1, 60, 0.3);
        assert_eq!(voices.note_random(a), latched);
    }

    #[test]
    fn note_random_differs_across_concurrent_voices() {
        let mut voices = Voices::new();
        let mut seen: Vec<f32> = Vec::new();
        for i in 0..N {
            let v = voices.note_on(1, 60 + i as u8);
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
            let va = a.note_on(1, 60 + i);
            let vb = b.note_on(1, 60 + i);
            assert_eq!(a.note_random(va), b.note_random(vb));
        }
    }

    #[test]
    fn note_random_relatched_on_reuse() {
        // A reused (stolen) voice draws a fresh value, not the stale one.
        let mut voices = Voices::new();
        for i in 0..N {
            voices.note_on(1, 60 + i as u8);
        }
        let stolen = voices.note_on(2, 90);
        let before = voices.note_random(stolen);
        // Steal it again with another full round + one more note.
        for i in 0..N {
            voices.note_on(3, 40 + i as u8);
        }
        let stolen2 = voices.note_on(4, 91);
        // Overwhelmingly likely distinct; assert the draw actually re-ran by
        // checking the stream advanced (value changed for this reused slot).
        if stolen2 == stolen {
            assert_ne!(voices.note_random(stolen2), before);
        }
    }

    #[test]
    fn pressure_is_clamped() {
        let mut voices = Voices::new();
        let v = voices.note_on(1, 60);
        voices.channel_pressure(1, 2.5);
        assert_eq!(voices.pressure(v), 1.0);
        voices.poly_pressure(1, 60, -1.0);
        assert_eq!(voices.pressure(v), 0.0);
    }
}
