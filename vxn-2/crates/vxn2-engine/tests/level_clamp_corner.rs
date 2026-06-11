//! Clamp-corner regression for level-mod routes (ticket 0076). A
//! full-depth LFO on an op level gates the op silent each cycle; the
//! per-sample `clamp(eg + mod, 0, 1)` used to corner the amplitude
//! envelope at full LFO slope there — an isolated broadband tick, twice
//! per LFO cycle. The engine now clamps + one-poles the ramp targets at
//! block rate, which must keep the rendered corner below the patch's own
//! waveform sharpness.
//!
//! Detector: max |4th difference| over late render (EG settled, t=2..3 s).
//! The 4th difference suppresses smooth carriers by f^4 while a slope
//! discontinuity keeps its full size. Pre-0076 the routed render measured
//! 6.5e-4 against a 3.65e-4 static floor (1.8x); the bound allows 1.2x.

use vxn2_engine::MatrixRowRaw;
use vxn2_engine::engine::Engine;

const SR: f32 = 48_000.0;
const BLK: usize = 32;

fn max_d4(route: bool) -> f64 {
    let mut e = Engine::new(SR, BLK);
    e.params.delay.on = false;
    e.params.delay.mix = 0.0;
    e.params.reverb.on = false;
    e.params.reverb.mix = 0.0;
    e.params.mod_params.lfo1.rate_hz = 5.0;
    if route {
        e.params.matrix_rows[0] = MatrixRowRaw {
            source: 1, // Lfo1
            dest: 2,   // Op1Level
            curve: 0,
            active: true,
            depth: 1.0,
        };
        e.params.mtx_depths[0] = 1.0;
    }
    e.apply_block_params();
    e.note_on(60, 100);

    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    for _ in 0..(2 * SR as usize / BLK) {
        e.process_block(&mut l, &mut r);
    }
    let nblocks = SR as usize / BLK;
    let mut buf = Vec::with_capacity(nblocks * BLK);
    for _ in 0..nblocks {
        e.process_block(&mut l, &mut r);
        buf.extend_from_slice(&l);
    }
    // Amplitude-normalized: |d4| over the local signal RMS (±256 samples).
    // Raw |d4| scales with how loud the waveform currently is — a full-depth
    // route pins the level at 1.0 and is louder than any static reference —
    // while an embedded discontinuity is a local outlier at any loudness.
    let mut prefix = vec![0.0_f64; buf.len() + 1];
    for (i, &x) in buf.iter().enumerate() {
        prefix[i + 1] = prefix[i] + (x as f64) * (x as f64);
    }
    let local_rms = |i: usize| {
        let a = i.saturating_sub(256);
        let b = (i + 256).min(buf.len());
        ((prefix[b] - prefix[a]) / (b - a) as f64).sqrt()
    };
    (2..buf.len() - 2)
        .map(|i| {
            let d4 = (buf[i + 2] - 4.0 * buf[i + 1] + 6.0 * buf[i] - 4.0 * buf[i - 1]
                + buf[i - 2])
                .abs() as f64;
            d4 / local_rms(i).max(1e-9)
        })
        .fold(0.0, f64::max)
}

#[test]
fn gating_level_route_stays_below_waveform_floor() {
    let floor = max_d4(false);
    let routed = max_d4(true);
    assert!(
        routed < floor * 1.2,
        "gating level route d4 max {routed:.2e} pokes above static floor {floor:.2e} — clamp corner is back"
    );
}
