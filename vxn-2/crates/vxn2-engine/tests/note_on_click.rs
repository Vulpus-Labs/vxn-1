//! Note-on (onset) click regression. The per-op EG level is applied as a
//! block constant in `stack_tick`; block-to-block steps are smoothed by the
//! 0077 per-sample ramp, but the note's *first* block used to snap — the
//! engine ran `eg_tick` (advancing the level one attack step past 0) and then
//! rendered that step as a block constant with no ramp. On a near-zero attack
//! (rate 99) the voice amplitude jumped 0 → full at sample 0: an onset click.
//! The fresh-note path now seeds the level at silence and ramps the onset
//! across the first block, so a fast attack fades in (~one block) instead of
//! stepping.

use vxn2_engine::alloc::AssignMode;
use vxn2_engine::engine::Engine;

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
        let worst = (on_t..buf.len() - 2)
            .map(|i| {
                (buf[i + 2] - 4.0 * buf[i + 1] + 6.0 * buf[i] - 4.0 * buf[i - 1] + buf[i - 2])
                    .abs() as f64
            })
            .fold(0.0, f64::max);
        assert!(
            worst < 5e-3,
            "note {note}: onset |d4| {worst:.2e} — fast-attack onset click is back \
             (pre-fix the first block stepped 0 → full)"
        );
    }
}

/// Solo-mode steal (legato off): stealing a *sounding* note must be click-free.
/// A retrigger steal resets the oscillator phase, and the EG continues its level
/// across `note_on`. The earlier onset fix masked the phase-reset glitch by
/// forcing the level to 0 through it (`op_level_mod = -eg`) — trading the click
/// for a level dip. Carrying the level through instead (the onset fix's intent)
/// then *exposed* the phase glitch as a loud click. The resolution keeps both
/// the level and the oscillator waveform continuous on a steal (the EG still
/// retriggers), so neither artefact is present. We measure the steal transient
/// with the same 4th-difference probe as the note-off test and require it to
/// stay near the click-free (legato) floor.
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

    let d4 = |i: usize| {
        (buf[i + 2] - 4.0 * buf[i + 1] + 6.0 * buf[i] - 4.0 * buf[i - 1] + buf[i - 2]).abs() as f64
    };
    let steal_worst = (steal_t..buf.len() - 2).map(d4).fold(0.0, f64::max);
    let baseline = (4..steal_t - 2).map(d4).fold(0.0, f64::max);

    // Click-free reference (legato, phase+level continuous) measures ~0.016 on
    // this patch; the phase-reset click (level carried through, phase reset)
    // measures ~0.6. Sit the gate well between the two.
    assert!(
        steal_worst < 5e-2,
        "solo steal transient |d4| {steal_worst:.2e} (steady baseline {baseline:.2e}) — \
         a steal-of-sounding-note click is back (phase reset no longer masked / continued)"
    );
}
