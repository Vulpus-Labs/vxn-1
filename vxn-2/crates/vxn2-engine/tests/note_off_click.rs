//! Note-off click regression. The op feedback loop must stay in its stable
//! region at every feedback setting so a releasing EG can't sweep the loop gain
//! through a stability boundary and collapse the oscillation mode into a loud
//! broadband transient about 1 ms after note-off. The release must be
//! transient-free.
//!
//! The feedback-scale table keeps the loop gain inside that stable region.

mod common;

use vxn2_engine::engine::Engine;

const SR: f32 = 48_000.0;
const BLK: usize = 32;

#[test]
fn note_off_release_is_click_free() {
    for &note in &[48u8, 60, 67] {
        let mut e = Engine::new(SR, BLK);
        e.params.delay.on = false;
        e.params.delay.mix = 0.0;
        e.params.reverb.on = false;
        e.params.reverb.mix = 0.0;
        e.note_on(note, 100);
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        let off_block = SR as usize / 4 / BLK;
        let total = SR as usize / 2 / BLK;
        let mut buf = Vec::with_capacity(total * BLK);
        for b in 0..total {
            if b == off_block {
                e.note_off(note);
            }
            e.process_block(&mut l, &mut r);
            buf.extend_from_slice(&l);
        }
        let off_t = off_block * BLK;
        let worst = common::worst_d4(&buf, off_t..buf.len() - 2);
        assert!(
            worst < 5e-3,
            "note {note}: post-off |d4| {worst:.2e} — release transient is back (pre-fix ≈ 0.7)"
        );
    }
}
