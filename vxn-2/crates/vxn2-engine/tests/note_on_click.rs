//! Note-on (onset) click regression. The per-op EG level is applied as a
//! block constant in `stack_tick`; block-to-block steps are smoothed by the
//! 0077 per-sample ramp, but the note's *first* block used to snap — the
//! engine ran `eg_tick` (advancing the level one attack step past 0) and then
//! rendered that step as a block constant with no ramp. On a near-zero attack
//! (rate 99) the voice amplitude jumped 0 → full at sample 0: an onset click.
//! The fresh-note path now seeds the level at silence and ramps the onset
//! across the first block, so a fast attack fades in (~one block) instead of
//! stepping.

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
