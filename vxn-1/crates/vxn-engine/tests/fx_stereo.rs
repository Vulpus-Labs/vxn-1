//! FX stereo controls reach the render (ticket 0278).
//!
//! Two globals added over the shared kernels' new arguments (0277):
//! `PhaserStereo` (the L/R LFO sweep offset) and `DelayPingPong` (feedback
//! crossfeed). Both are pure *stereo* controls — they move where energy sits
//! across the two channels without changing the mono sum much — so each test
//! measures the L−R difference signal rather than either channel alone.
//!
//! Both also default to the behaviour the kernels were pinned to before they
//! were controls, which is what keeps `baseline.rs`'s render golden and VXN1b's
//! parity oracle green; these tests pin the *other* end of each range.

use vxn_engine::{GlobalParam, Synth};

const SR: f32 = 48_000.0;
const BLK: usize = 64;
const BLOCKS: usize = 200;

/// Hold a chord through the FX bus configured by `cfg`, and return the L−R
/// difference signal — the channel decorrelation the stereo controls own.
fn render_side(cfg: &[(GlobalParam, f32)]) -> Vec<f32> {
    let mut synth = Synth::new(SR);
    {
        let g = synth.params_mut().global_mut();
        for f in [
            GlobalParam::PhaserOn,
            GlobalParam::ChorusOn,
            GlobalParam::DelayOn,
            GlobalParam::ReverbOn,
            GlobalParam::LimiterOn,
        ] {
            g.set(f, 0.0);
        }
        for &(p, v) in cfg {
            g.set(p, v);
        }
    }
    for &n in &[48u8, 55, 60] {
        synth.note_on(n, 0.9);
    }
    let mut l = [0.0f32; BLK];
    let mut r = [0.0f32; BLK];
    let mut side = Vec::with_capacity(BLOCKS * BLK);
    for _ in 0..BLOCKS {
        synth.process(&mut l, &mut r);
        for i in 0..BLK {
            side.push(l[i] - r[i]);
        }
    }
    side
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

#[test]
fn phaser_stereo_widens_the_image() {
    // The voice bus already carries stereo content of its own (two detuned
    // oscillators, per-voice drift), so 0° is not silent on the side channel —
    // it is the bus's own width passed through a phaser sweeping both cascades
    // in lockstep. The 180° default sweeps them anti-phase and adds its own
    // decorrelation on top, which is the ratio measured here.
    let cfg = |deg: f32| {
        [
            (GlobalParam::PhaserOn, 1.0),
            (GlobalParam::PhaserMix, 1.0),
            (GlobalParam::PhaserDepth, 0.9),
            (GlobalParam::PhaserStereo, deg),
        ]
    };
    let narrow = rms(&render_side(&cfg(0.0)));
    let wide = rms(&render_side(&cfg(180.0)));
    assert!(
        wide > 1.5 * narrow.max(1e-9),
        "Phaser Stereo should widen: side RMS {narrow:.3e} at 0° vs {wide:.3e} at 180°"
    );
}

#[test]
fn delay_pingpong_changes_the_stereo_image() {
    // Crossfeed only diverges from straight feedback on a stereo input, so the
    // chorus runs ahead of the delay to decorrelate the bus first (it sits
    // upstream in the chain: phaser → chorus → delay → reverb).
    let cfg = |pingpong: f32| {
        [
            (GlobalParam::ChorusOn, 1.0),
            (GlobalParam::ChorusMix, 0.8),
            (GlobalParam::ChorusDepth, 0.8),
            (GlobalParam::DelayOn, 1.0),
            (GlobalParam::DelayMix, 0.8),
            (GlobalParam::DelayFeedback, 0.7),
            (GlobalParam::DelayTime, 0.12),
            (GlobalParam::DelayPingPong, pingpong),
        ]
    };
    let straight = render_side(&cfg(0.0));
    let ping = render_side(&cfg(1.0));
    let diff = rms(
        &straight
            .iter()
            .zip(&ping)
            .map(|(a, b)| a - b)
            .collect::<Vec<f32>>(),
    );
    assert!(
        diff > 0.1 * rms(&straight).max(1e-9),
        "Ping-Pong should re-route the repeats: side-signal difference RMS {diff:.3e} \
         vs straight side RMS {:.3e}",
        rms(&straight)
    );
}
