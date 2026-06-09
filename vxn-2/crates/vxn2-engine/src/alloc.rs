//! Polyphony allocator: 16 fixed `Stack` slots with oldest-note stealing,
//! Poly / Solo assignment modes, glide (portamento), and channel-wide pitch
//! bend forwarding.
//!
//! A "voice" here is a *stack* of up to 8 lane-packed op-instances (ticket
//! 0005). The allocator's poly cap is 16 stacks, giving up to 16 × 8 = 128
//! op-voice instances in flight when `stack_density = 8`. Stealing operates
//! on whole stacks — picking off the oldest reclaims all of its lanes
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

use vxn2_dsp::eg::EgStage;
use vxn2_dsp::stack::{Stack, StackParams};
use vxn2_dsp::voice::VoiceParams;

use crate::shared::Patch;

/// Fixed polyphony cap. ADR §3 sets this at 16 stacks for v1.
pub const N_STACKS: usize = 16;

const SOLO_SLOT: usize = 0;

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
    /// Solo: skip EG retrigger on note change. Poly: enables overlap-glide.
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
    /// True when `SOLO_SLOT` is currently playing a held note.
    solo_active: bool,
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
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
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

    /// Patch-driven note-off — gates every stack holding `note`.
    pub fn note_off_patch(&mut self, _patch: &Patch, note: u8) {
        self.note_off_poly(note);
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
            if self.stacks[i].is_idle() {
                self.seq[i] = IDLE_SEQ;
                if i == SOLO_SLOT {
                    self.solo_active = false;
                }
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
        let glide_from = if params.legato && params.glide_time_ms > 0.0 {
            self.most_recent_gated_pitch(note)
        } else {
            0.0
        };
        let slot = self.pick_slot();
        self.glides[slot] = None;
        let counter = self.bump_seq();
        self.stacks[slot].note_on(sp, vp, note, velocity, self.sample_rate, counter);
        self.stacks[slot].set_bend(self.bend_st);
        self.seq[slot] = counter;
        if glide_from != 0.0 {
            self.start_glide(slot, glide_from, params.glide_time_ms / 1000.0);
        }
        slot
    }

    fn note_off_poly(&mut self, note: u8) {
        for i in 0..N_STACKS {
            if self.stacks[i].gate && self.stacks[i].note == note {
                self.stacks[i].note_off();
            }
        }
    }

    /// Pick the destination slot. Priority: idle slot → oldest at/past
    /// Sustain (ties broken by lowest note) → globally oldest.
    fn pick_slot(&self) -> usize {
        for i in 0..N_STACKS {
            if self.stacks[i].is_idle() {
                return i;
            }
        }
        let mut best: Option<(usize, u64, u8)> = None;
        for i in 0..N_STACKS {
            let stealable = self.stacks[i]
                .ops
                .iter()
                .any(|o| matches!(o.eg.stage, EgStage::Sustain | EgStage::Release));
            if stealable {
                let cand = (i, self.seq[i], self.stacks[i].note);
                best = Some(match best {
                    None => cand,
                    Some(b) if cand.1 < b.1 || (cand.1 == b.1 && cand.2 < b.2) => cand,
                    Some(b) => b,
                });
            }
        }
        if let Some((i, _, _)) = best {
            return i;
        }
        let mut min_seq = IDLE_SEQ;
        let mut min_i = 0;
        for i in 0..N_STACKS {
            if self.seq[i] < min_seq {
                min_seq = self.seq[i];
                min_i = i;
            }
        }
        min_i
    }

    fn most_recent_gated_pitch(&self, new_note: u8) -> f32 {
        let mut best: Option<(u64, u8)> = None;
        for i in 0..N_STACKS {
            if self.stacks[i].gate {
                let cand = (self.seq[i], self.stacks[i].note);
                best = Some(match best {
                    None => cand,
                    Some(b) if cand.0 > b.0 => cand,
                    Some(b) => b,
                });
            }
        }
        match best {
            Some((_, n)) => n as f32 - new_note as f32,
            None => 0.0,
        }
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
        let glide_from = if self.solo_active && params.glide_time_ms > 0.0 {
            self.stacks[SOLO_SLOT].note as f32 - note as f32
        } else {
            0.0
        };
        self.push_held(note);
        let counter = self.bump_seq();
        if self.solo_active && params.legato {
            self.stacks[SOLO_SLOT].retarget_pitch(sp, vp, note, velocity, self.sample_rate);
        } else {
            self.stacks[SOLO_SLOT].note_on(sp, vp, note, velocity, self.sample_rate, counter);
            self.stacks[SOLO_SLOT].set_bend(self.bend_st);
        }
        self.solo_active = true;
        self.seq[SOLO_SLOT] = counter;
        if glide_from != 0.0 {
            self.start_glide(SOLO_SLOT, glide_from, params.glide_time_ms / 1000.0);
        } else {
            self.glides[SOLO_SLOT] = None;
            self.stacks[SOLO_SLOT].set_glide(0.0);
        }
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
        if self.stacks[SOLO_SLOT].note != note {
            return;
        }
        if let Some(prev) = self.most_recent_held() {
            let cur = self.stacks[SOLO_SLOT].note as f32;
            let glide_from = if params.glide_time_ms > 0.0 {
                cur - prev as f32
            } else {
                0.0
            };
            let vel = self.stacks[SOLO_SLOT].velocity;
            let counter = self.bump_seq();
            if params.legato {
                self.stacks[SOLO_SLOT].retarget_pitch(sp, vp, prev, vel, self.sample_rate);
            } else {
                self.stacks[SOLO_SLOT].note_on(sp, vp, prev, vel, self.sample_rate, counter);
                self.stacks[SOLO_SLOT].set_bend(self.bend_st);
            }
            self.seq[SOLO_SLOT] = counter;
            if glide_from != 0.0 {
                self.start_glide(SOLO_SLOT, glide_from, params.glide_time_ms / 1000.0);
            }
        } else {
            self.stacks[SOLO_SLOT].note_off();
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
    fn poly_note_on_picks_idle_slot() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams::default();
        let sp = density1();
        let vp = fast_patch();
        alloc.note_on(&params, &sp, &vp, 60, 100);
        assert!(alloc.stacks[0].gate);
        assert_eq!(alloc.stacks[0].note, 60);
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
        let active = alloc.stacks.iter().filter(|s| s.gate).count();
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
        assert!(!alloc.stacks[0].gate);
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

    #[test]
    fn steal_oldest_when_polyphony_full() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams::default();
        let sp = density1();
        let vp = fast_patch();
        for n in 60u8..76 {
            alloc.note_on(&params, &sp, &vp, n, 100);
        }
        run_blocks(&mut alloc, (SR as usize) / 20 / BLK);
        alloc.note_on(&params, &sp, &vp, 90, 100);
        let active: Vec<u8> = alloc.stacks.iter().filter(|s| s.gate).map(|s| s.note).collect();
        assert_eq!(active.len(), 16);
        assert!(active.contains(&90));
        assert!(!active.contains(&60));
    }

    #[test]
    fn solo_reuses_slot_zero() {
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
        assert!(alloc.stacks[0].gate);
        assert_eq!(alloc.stacks[0].note, 64);
        for s in &alloc.stacks[1..] {
            assert!(s.is_idle());
        }
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
        assert!(alloc.stacks[0].gate);
        assert_eq!(alloc.stacks[0].note, 60);
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
        assert_eq!(alloc.stacks[0].ops[0].eg.stage, EgStage::Sustain);
        alloc.note_on(&params, &sp, &vp, 64, 100);
        assert_eq!(alloc.stacks[0].ops[0].eg.stage, EgStage::Sustain);
        assert_eq!(alloc.stacks[0].note, 64);
    }

    #[test]
    fn solo_non_legato_retriggers_eg() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams {
            assign_mode: AssignMode::Solo,
            legato: false,
            glide_time_ms: 0.0,
        };
        let sp = density1();
        let vp = fast_patch();
        alloc.note_on(&params, &sp, &vp, 60, 100);
        run_blocks(&mut alloc, (SR as usize) / 5 / BLK);
        alloc.note_on(&params, &sp, &vp, 64, 100);
        assert_eq!(alloc.stacks[0].ops[0].eg.stage, EgStage::Attack);
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
            (alloc.stacks[0].glide_st - 12.0).abs() < 1e-3,
            "glide_st should start at +12 st, got {}",
            alloc.stacks[0].glide_st
        );
        run_blocks(&mut alloc, (100 * SR as usize) / 1000 / BLK);
        assert!(
            (alloc.stacks[0].glide_st - 6.0).abs() < 1.0,
            "mid-glide should be ~+6, got {}",
            alloc.stacks[0].glide_st
        );
        run_blocks(&mut alloc, (150 * SR as usize) / 1000 / BLK);
        assert!(
            alloc.stacks[0].glide_st.abs() < 1e-3,
            "post-glide should be 0, got {}",
            alloc.stacks[0].glide_st
        );
    }

    #[test]
    fn poly_legato_overlap_glides_new_stack() {
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
            .position(|s| s.gate && s.note == 72)
            .expect("new note not allocated");
        assert!(
            (alloc.stacks[new_slot].glide_st - (-12.0)).abs() < 1e-3,
            "expected glide_st=-12, got {}",
            alloc.stacks[new_slot].glide_st
        );
    }

    #[test]
    fn poly_no_legato_no_glide() {
        let mut alloc = PolyAlloc::new(SR);
        let params = AllocParams {
            assign_mode: AssignMode::Poly,
            legato: false,
            glide_time_ms: 100.0,
        };
        let sp = density1();
        let vp = fast_patch();
        alloc.note_on(&params, &sp, &vp, 60, 100);
        alloc.note_on(&params, &sp, &vp, 72, 100);
        for s in &alloc.stacks {
            assert_eq!(s.glide_st, 0.0);
        }
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
            assert_eq!(s.bend_st, 2.0);
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
        assert_eq!(alloc.stacks[0].bend_st, 1.0);
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
        let active = alloc.stacks.iter().filter(|s| s.gate).count();
        assert_eq!(active, 1);
        assert_eq!(alloc.stacks[0].density, 8);
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
        assert!(!alloc.stacks[0].gate);
        for op in &alloc.stacks[0].ops {
            assert_eq!(op.eg.stage, EgStage::Release);
        }
    }
}
