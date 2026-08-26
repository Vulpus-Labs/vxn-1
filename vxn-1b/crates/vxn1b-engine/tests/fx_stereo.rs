//! FX stereo controls reach the render (ticket 0279).
//!
//! `PhaserStereo` (L/R LFO sweep offset) and `DelayPingPong` (feedback
//! crossfeed) are globals over the shared kernels' 0277 arguments. Both are
//! stereo controls, so each test measures the L−R difference signal rather
//! than one channel: they move energy across the pair without much changing
//! the mono sum.
//!
//! Both default to the kernels' historical pinned behaviour, which is what
//! keeps the VXN1 parity oracle green; these tests pin the other end.

use vxn1b_engine::{Engine, Layer, ParamId, clap_id_of};

const SR: f32 = 48_000.0;
const FRAMES: usize = 4096;
const BLOCKS: usize = 4;

/// Hold a note through the global FX chain configured by `cfg`, returning the
/// L−R difference signal.
fn render_side(cfg: &[(ParamId, f32)]) -> Vec<f32> {
    let mut e = Engine::new(SR);
    // FX are global — the layer argument is ignored for these ids.
    for &(p, v) in cfg {
        e.set_param(clap_id_of(Layer::L1, p), v);
    }
    e.note_on(0, 60, 1.0);
    let mut l = vec![0.0f32; FRAMES];
    let mut r = vec![0.0f32; FRAMES];
    let mut side = Vec::with_capacity(BLOCKS * FRAMES);
    for _ in 0..BLOCKS {
        e.process_block(&mut l, &mut r);
        side.extend(l.iter().zip(&r).map(|(a, b)| a - b));
    }
    side
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

#[test]
fn phaser_stereo_widens_the_image() {
    let cfg = |deg: f32| {
        [
            (ParamId::PhaserOn, 1.0),
            (ParamId::PhaserMix, 1.0),
            (ParamId::PhaserDepth, 0.9),
            (ParamId::PhaserStereo, deg),
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
    // chorus decorrelates the bus upstream of the delay (chain order is
    // dynamics → chorus → phaser → delay → reverb).
    let cfg = |pingpong: f32| {
        [
            (ParamId::ChorusOn, 1.0),
            (ParamId::ChorusMix, 0.8),
            (ParamId::ChorusDepth, 0.8),
            (ParamId::DelayOn, 1.0),
            (ParamId::DelayMix, 0.8),
            (ParamId::DelayFeedback, 0.7),
            (ParamId::DelayTime, 0.12),
            (ParamId::DelayPingPong, pingpong),
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
        "Ping-Pong should re-route the repeats: difference RMS {diff:.3e} vs straight {:.3e}",
        rms(&straight)
    );
}
