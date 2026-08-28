//! Note-on (onset) click regression. The per-op EG level is applied as a block
//! constant in `stack_tick`, with per-sample ramps smoothing block-to-block
//! steps. The fresh-note path seeds the level at silence and ramps the onset
//! across the first block, so even a near-zero attack (rate 99) fades in (~one
//! block) rather than stepping 0 → full at sample 0.
//!
//! ## What these guards do and don't cover
//!
//! The probe is [`common::worst_d4`] — a *slope-discontinuity* detector. The
//! discontinuity worth guarding is the silence → note boundary, so the windows
//! below are one [`CONTROL_BLOCK`] wide, starting at the note-on.
//!
//! They deliberately stop short of the attack apex. A DX7-faithful attack
//! marches linear-in-dB (`eg::march_attack_log`), so amplitude *accelerates*
//! into the L1 corner and the sharpest slope change on a fast-attack patch sits
//! at the top of the attack, two to four blocks in — not at the onset. That
//! corner is the percussive transient the instrument exists to produce; a
//! window wide enough to include it cannot distinguish it from a click, and a
//! threshold loose enough to admit it would no longer catch a real one. Before
//! the attack curve was corrected the distinction did not arise, because no
//! attack was fast enough to place a corner anywhere near the onset.
//!
//! [`onset_probe_detects_a_hard_gate`] pins the narrowed window's sensitivity:
//! an ungated 0 → full step still measures ~200× the threshold inside it.

mod common;

use vxn2_engine::alloc::AssignMode;
use vxn2_engine::engine::Engine;
use vxn2_engine::preset::from_toml_str;
use vxn2_engine::shared::{ParamModel, SharedParams};
use vxn_core_dsp::control::CONTROL_BLOCK;

const SR: f32 = 48_000.0;
/// Render cadence — the shipping one. Both host shells slice buffers to
/// `CONTROL_BLOCK` before `Engine::process_block`, so this is the granularity
/// at which an onset ramp is actually emitted.
const BLK: usize = CONTROL_BLOCK;
/// Onset-window transient ceiling, shared by the note-on and steal guards.
const ONSET_D4_MAX: f64 = 5e-3;

#[test]
fn note_on_onset_is_click_free_on_fast_attack() {
    for &note in &[48u8, 60, 67] {
        let mut e = Engine::new(SR, BLK);
        e.params.delay.on = false;
        e.params.delay.mix = 0.0;
        e.params.reverb.on = false;
        e.params.reverb.mix = 0.0;
        // Fastest possible attack on every operator — the worst case for an
        // onset step.
        for op in &mut e.params.patch.voice.ops {
            op.eg.r[0] = 99;
        }
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        // Render a few quiet blocks first so the buffer has a settled-silence
        // pre-roll, then trigger.
        let mut buf = Vec::new();
        for _ in 0..2 {
            e.process_block(&mut l, &mut r);
            buf.extend_from_slice(&l);
        }
        let on_t = buf.len();
        e.note_on(note, 100);
        for _ in 0..(SR as usize / 8 / BLK) {
            e.process_block(&mut l, &mut r);
            buf.extend_from_slice(&l);
        }
        // 4th-difference transient detector across the silence → note boundary
        // (same discontinuity probe as the note-off test). One control block:
        // the span over which the fresh-note path ramps the seeded-silent level
        // up. See the module docs on why it stops there.
        let worst = common::worst_d4(&buf, on_t..on_t + BLK);
        assert!(
            worst < ONSET_D4_MAX,
            "note {note}: onset |d4| {worst:.2e} — fast-attack onset click is back"
        );
    }
}

/// Sensitivity pin for the narrowed onset window: splice a *fully attacked*
/// note onto silence at a block boundary — the exact defect the guard exists to
/// catch, an onset that steps to full instead of ramping — and confirm the
/// one-block window still sees it, by a wide margin. Without this, narrowing the
/// window could quietly turn the guard into a no-op.
#[test]
fn onset_probe_detects_a_hard_gate() {
    let mut e = Engine::new(SR, BLK);
    e.params.delay.on = false;
    e.params.delay.mix = 0.0;
    e.params.reverb.on = false;
    e.params.reverb.mix = 0.0;
    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    e.note_on(60, 100);
    let mut sounding = Vec::new();
    for _ in 0..(SR as usize / 8 / BLK) {
        e.process_block(&mut l, &mut r);
        sounding.extend_from_slice(&l);
    }
    // Discard the first 100 ms so the splice point is at full, steady level.
    let mut gated = vec![0.0_f32; 2 * BLK];
    let on_t = gated.len();
    gated.extend_from_slice(&sounding[(SR as usize / 10)..]);

    let worst = common::worst_d4(&gated, on_t..on_t + BLK);
    assert!(
        worst > ONSET_D4_MAX * 100.0,
        "hard-gated onset |d4| {worst:.2e} — the one-block window has lost its \
         teeth; a real onset step would now slip past the guard"
    );
}

/// Solo-mode steal (legato off): stealing a *sounding* note must be click-free.
/// A solo note change round-robins to a fresh voice (onset from silence is
/// click-free) and declicks the previous note — a ~5 ms fade to silence that
/// overlaps the new onset. Measure the steal transient with the same
/// 4th-difference probe as the note-off test and require it near the floor.
///
/// A steal *contains* a fresh onset, so it is scored against one: the reference
/// is this run's own first note-on, measured over the same one-block window.
/// The absolute ceiling is the onset guard's. (The reference used to be
/// `4..steal_t`, which spans the first note-on — it was measuring that onset,
/// not the steady state it was named for, so the two sides of the comparison
/// were the same quantity.)
#[test]
fn solo_steal_is_click_free() {
    let mut e = Engine::new(SR, BLK);
    e.params.alloc.assign_mode = AssignMode::Solo;
    e.params.alloc.legato = false; // retrigger the EG on the stolen note
    e.params.delay.on = false;
    e.params.delay.mix = 0.0;
    e.params.reverb.on = false;
    e.params.reverb.mix = 0.0;
    for op in &mut e.params.patch.voice.ops {
        op.eg.r[0] = 99; // fast attack — worst case for a steal transient
    }

    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    let mut buf = Vec::new();

    // Settled-silence pre-roll, so the first note-on has a measurable boundary
    // to serve as the onset reference.
    for _ in 0..2 {
        e.process_block(&mut l, &mut r);
        buf.extend_from_slice(&l);
    }
    let on_t = buf.len();
    // First note, settle to a steady sounding level.
    e.note_on(60, 100);
    for _ in 0..(SR as usize / 10 / BLK) {
        e.process_block(&mut l, &mut r);
        buf.extend_from_slice(&l);
    }
    let steal_t = buf.len();
    // Steal it. The slot is still sounding → waveform + level continuity.
    e.note_on(67, 100);
    for _ in 0..(SR as usize / 20 / BLK) {
        e.process_block(&mut l, &mut r);
        buf.extend_from_slice(&l);
    }

    let steal_worst = common::worst_d4(&buf, steal_t..steal_t + BLK);
    let onset_ref = common::worst_d4(&buf, on_t..on_t + BLK);

    // Crossfade + ~5 ms declick measures ~0.001 here; a phase-reset steal click
    // measures ~1. Gate well between the two, on both counts.
    assert!(
        steal_worst < ONSET_D4_MAX,
        "solo steal transient |d4| {steal_worst:.2e} — a steal-of-sounding-note \
         click is back"
    );
    assert!(
        steal_worst <= 2.0 * onset_ref,
        "solo steal transient |d4| {steal_worst:.2e} exceeds twice a plain note-on \
         ({onset_ref:.2e}) — the steal is clickier than the onset it replaces"
    );
}

/// Worst 4th-difference transient at the note boundaries of a FLUTE 2 solo
/// 16th-note line at 100 BPM, for the given stacking density and stack phase.
fn flute2_solo_sixteenths_boundary_d4(density: u8, stack_phase: f32) -> f64 {
    // Fixture, not a factory preset: this test needs a patch whose modulators
    // decay to sustain 0 (an unmasked FM transient on retrigger), and it must
    // keep testing that regardless of what the shipping bank contains.
    const FLUTE2: &str = include_str!("fixtures/flute2.toml");
    let (_meta, blob, _warn) = from_toml_str(FLUTE2).expect("FLUTE 2 parses");
    let shared = SharedParams::new();
    shared.load_bytes(&blob).expect("FLUTE 2 loads");

    let mut e = Engine::new(SR, BLK);
    e.snapshot_params(&shared);
    e.params.alloc.assign_mode = AssignMode::Solo;
    e.params.alloc.legato = false;
    e.params.delay.on = false;
    e.params.delay.mix = 0.0;
    e.params.reverb.on = false;
    e.params.reverb.mix = 0.0;
    e.params.patch.stack.density = density;
    e.params.patch.stack.phase = stack_phase;
    e.apply_block_params();

    // 16th notes at 100 BPM = 0.15 s = 7200 samples per note.
    let note_blocks = ((SR * 60.0 / 100.0 / 4.0) / BLK as f32).round() as usize;
    let pattern = [72u8, 74, 76, 77, 79, 77, 76, 74];
    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    let mut buf = Vec::new();
    let mut boundaries = Vec::new();
    for (i, &n) in pattern.iter().cycle().take(24).enumerate() {
        e.note_on(n, 100);
        if i > 0 {
            boundaries.push(buf.len());
        }
        for _ in 0..note_blocks {
            e.process_block(&mut l, &mut r);
            buf.extend_from_slice(&l);
        }
    }

    let mut worst = 0.0;
    for &b in &boundaries {
        let lo = b.saturating_sub(96).max(4);
        let hi = (b + 96).min(buf.len() - 2);
        worst = f64::max(worst, common::worst_d4(&buf, lo..hi));
    }
    worst
}

/// The FLUTE 2 fixture patch played as a solo 16th-note line at 100 BPM must
/// be click-free. The patch's modulators decay to sustain 0, so retriggering
/// them mid-phrase would be an unmasked FM transient; solo round-robins to a
/// fresh voice per note and declicks the previous one instead.
#[test]
fn flute2_solo_sixteenths_are_click_free() {
    let worst = flute2_solo_sixteenths_boundary_d4(1, 0.0);
    assert!(
        worst < 1.5e-2,
        "FLUTE 2 solo 16ths: note-boundary |d4| {worst:.2e} — per-note click is back"
    );
}

/// FLUTE 2 solo 16ths with voice stacking (density 4) and stack phase 0.5
/// (maximal per-lane decorrelation). Any in-place voice reuse would discontinue
/// the decorrelated lane phases; only the fresh-voice + declick crossfade is
/// clean here.
#[test]
fn flute2_solo_sixteenths_stacked_phase_half_are_click_free() {
    let worst = flute2_solo_sixteenths_boundary_d4(4, 0.5);
    assert!(
        worst < 1.5e-2,
        "FLUTE 2 solo 16ths (density 4, phase 0.5): note-boundary |d4| {worst:.2e} — \
         stacked-steal click is back"
    );
}

/// A killed (declicked) solo voice fades out and frees its slot: after a steal,
/// the previous voice reaches Idle within the declick window + a block.
#[test]
fn solo_declick_completes_to_idle() {
    let mut e = Engine::new(SR, BLK);
    e.params.alloc.assign_mode = AssignMode::Solo;
    e.params.alloc.legato = false;
    e.params.delay.on = false;
    e.params.delay.mix = 0.0;
    e.params.reverb.on = false;
    e.params.reverb.mix = 0.0;
    e.apply_block_params();

    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    e.note_on(60, 100);
    e.process_block(&mut l, &mut r);
    e.note_on(67, 100); // steal: 60 starts declicking
    // Declick is ~5 ms; render well past it.
    for _ in 0..(SR as usize / 20 / BLK) {
        e.process_block(&mut l, &mut r);
    }
    let live = e
        .alloc
        .stacks
        .iter()
        .filter(|s| s.meta.gate && !s.is_idle())
        .count();
    assert_eq!(live, 1, "exactly one live voice after the declicked note frees");
}
