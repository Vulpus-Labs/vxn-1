//! Note-on (onset) click regression. The per-op EG level is applied as a block
//! constant in `stack_tick`, with per-sample ramps smoothing block-to-block
//! steps. The fresh-note path seeds the level at silence and ramps the onset
//! across the first block, so even a near-zero attack (rate 99) fades in (~one
//! block) rather than stepping 0 → full at sample 0.

mod common;

use vxn2_engine::alloc::AssignMode;
use vxn2_engine::engine::Engine;
use vxn2_engine::factory::factory;
use vxn2_engine::preset::from_toml_str;
use vxn2_engine::shared::{ParamModel, SharedParams};

const SR: f32 = 48_000.0;
const BLK: usize = 32;

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
        // 4th-difference transient detector over the onset window (same
        // discontinuity probe as the note-off test).
        let worst = common::worst_d4(&buf, on_t..buf.len() - 2);
        assert!(
            worst < 5e-3,
            "note {note}: onset |d4| {worst:.2e} — fast-attack onset click is back"
        );
    }
}

/// Solo-mode steal (legato off): stealing a *sounding* note must be click-free.
/// A solo note change round-robins to a fresh voice (onset from silence is
/// click-free) and declicks the previous note — a ~5 ms fade to silence that
/// overlaps the new onset. Measure the steal transient with the same
/// 4th-difference probe as the note-off test and require it near the floor.
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

    let steal_worst = common::worst_d4(&buf, steal_t..buf.len() - 2);
    let baseline = common::worst_d4(&buf, 4..steal_t - 2);

    // Crossfade + ~5 ms declick measures ~0.006 here; a phase-reset steal
    // click measures ~0.6. Gate well between the two.
    assert!(
        steal_worst < 1.5e-2,
        "solo steal transient |d4| {steal_worst:.2e} (steady baseline {baseline:.2e}) — \
         a steal-of-sounding-note click is back"
    );
}

/// Worst 4th-difference transient at the note boundaries of a FLUTE 2 solo
/// 16th-note line at 100 BPM, for the given stacking density and stack phase.
fn flute2_solo_sixteenths_boundary_d4(density: u8, stack_phase: f32) -> f64 {
    let fp = factory()
        .into_iter()
        .find(|p| p.name == "Flute 2")
        .expect("Flute 2 factory preset present");
    let (_meta, blob, _warn) = from_toml_str(fp.contents).expect("FLUTE 2 parses");
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

/// The FLUTE 2 factory preset played as a solo 16th-note line at 100 BPM must
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
