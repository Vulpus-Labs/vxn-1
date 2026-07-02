//! Note-off click regression (ticket 0079). The op feedback loop used to
//! run in its chaotic zone on the default patch (fb_scale 2.0): a
//! releasing EG swept the loop gain through the stability boundary and
//! the oscillation mode collapsed within a couple of samples — a loud
//! broadband transient (|d4| ≈ 0.7) about 1 ms after every note-off.
//! With the DX7-calibrated FB_SCALE_TABLE the loop stays in its stable
//! region at every feedback setting; the release must be transient-free.

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
            "note {note}: post-off |d4| {worst:.2e} — release transient is back (pre-0079 ≈ 0.7)"
        );
    }
}
