//! Zipper / discontinuity regression gate (ticket 0208).
//!
//! A stepped modulation source (here a **square LFO** at full rate) routed into
//! a continuous dest must not land a hard step at control-block edges. Each test
//! drives one route to its worst case and measures block-edge discontinuity in
//! the output; the smoothing added in 0208 keeps it bounded.
//!
//! One detector, on a pure-sine carrier: the peak block-edge **second
//! difference** relative to the carrier's own mean curvature. A hard step at a
//! control-block edge — whether an amplitude step (Amp: `ΔA·sin` injected into
//! d²) or a frequency step (Pitch/Cutoff: a slope break) — spikes it; a smooth
//! glide leaves the edge indistinguishable from the sine's interior curvature.
//!
//! Cutoff is included precisely to show it needs *no* new smoothing: the OTA
//! ladder ramps its own coefficients, so it already passes the same gate.

use vxn1b_engine::{DestId, Engine, Layer, MatrixSlot, ParamId, SourceId};
use vxn1b_engine::matrix::Curve;

const SR: f32 = 48_000.0;
const CB: usize = 32; // CONTROL_BLOCK
const FRAMES: usize = 8192;

/// Build an engine with a single square-LFO→`dest` route at `depth`, a steady
/// held sine note, and LFO onset defeated (full swing from sample 0).
fn engine_with_route(dest: DestId, depth: f32) -> Engine {
    let mut e = Engine::new(SR, FRAMES);
    // Pure sine carrier, osc1 only — a clean slope for the frequency detectors.
    e.set_param(ParamId::Osc1Wave as usize, 0.0); // Sine
    e.set_param(ParamId::Osc1Level as usize, 0.9);
    e.set_param(ParamId::Osc2Level as usize, 0.0);
    e.set_param(ParamId::SubLevel as usize, 0.0);
    e.set_param(ParamId::NoiseLevel as usize, 0.0);
    // Steady VCA: near-instant attack, full sustain, so amplitude is flat once
    // the note settles (except when Amp itself is the route under test).
    e.set_param(ParamId::Env2Attack as usize, 0.0005);
    e.set_param(ParamId::Env2Decay as usize, 0.001);
    e.set_param(ParamId::Env2Sustain as usize, 1.0);
    // Square LFO at full rate, no delay/fade, free-running: a hard ± flip.
    e.set_param(ParamId::Lfo1Shape as usize, 4.0); // Square
    e.set_param(ParamId::Lfo1Rate as usize, 40.0);
    e.set_param(ParamId::Lfo1DelayTime as usize, 0.0);
    e.set_param(ParamId::Lfo1Fade as usize, 0.0);
    e.set_param(ParamId::Lfo1FreeRun as usize, 1.0);

    // Slot 2 is the default LFO1→Pitch vibrato; zero it unless Pitch is the
    // route under test, so the carrier frequency is otherwise steady.
    if dest != DestId::Pitch {
        e.matrix_mut(Layer::L1).slots[2].depth = 0.0;
    }
    // Install the route in a spare slot (3); slot 0's Env2→Amp VCA stays so the
    // note sounds.
    e.matrix_mut(Layer::L1).slots[3] = MatrixSlot {
        source: SourceId::Lfo1,
        dest,
        depth,
        curve: Curve::Lin,
        scale_src: SourceId::None,
    };
    e.note_on(0, 60, 1.0);
    e
}

fn render(e: &mut Engine) -> Vec<f32> {
    let mut l = vec![0.0f32; FRAMES];
    let mut r = vec![0.0f32; FRAMES];
    e.process_block(&mut l, &mut r);
    l
}

/// Peak block-edge second difference relative to the mean interior second
/// difference. A frequency step on the sine carrier spikes the edge; a smooth
/// glide leaves edge ≈ interior curvature.
fn peak_edge_d2_ratio(x: &[f32], skip: usize) -> f32 {
    let mut peak_edge = 0.0f64;
    let mut sum_int = 0.0f64;
    let mut n_int = 0u64;
    for i in (skip + 1)..(x.len() - 1) {
        let d2 = (x[i - 1] as f64 - 2.0 * x[i] as f64 + x[i + 1] as f64).abs();
        if i % CB == 0 {
            peak_edge = peak_edge.max(d2);
        } else {
            sum_int += d2;
            n_int += 1;
        }
    }
    let mean_int = (sum_int / n_int.max(1) as f64).max(1e-9);
    (peak_edge / mean_int) as f32
}

#[test]
fn square_lfo_to_amp_is_declicked() {
    // LFO→Amp at a big depth: without the block-rate one-pole the VCA gain would
    // step by the full swing at each flip — a ΔA·sin spike in d². The guard
    // glides it, so the edge stays near the sine's own curvature.
    let mut e = engine_with_route(DestId::Amp, 0.8);
    let x = render(&mut e);
    let ratio = peak_edge_d2_ratio(&x, 256);
    eprintln!("amp: peak edge/interior d² ratio = {ratio:.2}");
    // Baseline: with the smoother removed this route measured ~73× — the guard
    // brings it to ~2.6×, on par with the sine's own curvature.
    assert!(
        ratio < 6.0,
        "square LFO→Amp spikes block-edge d² by {ratio}× — smoothing not engaged"
    );
}

#[test]
fn square_lfo_to_pitch_is_declicked() {
    // ±12 st square flip on a sine carrier. Without the cascade the frequency
    // jumps at the flip → a slope break (d² spike) at that block edge.
    let mut e = engine_with_route(DestId::Pitch, 1.0);
    let x = render(&mut e);
    let ratio = peak_edge_d2_ratio(&x, 256);
    eprintln!("pitch: peak edge/interior d² ratio = {ratio:.2}");
    assert!(
        ratio < 6.0,
        "square LFO→Pitch spikes block-edge d² by {ratio}× — cascade not engaged"
    );
}

#[test]
fn square_lfo_to_cutoff_stays_clean_without_added_smoothing() {
    // Cutoff gets no 0208 smoothing — the OTA ladder ramps its own coeffs. The
    // same detector must still pass, proving we correctly left it alone.
    let mut e = engine_with_route(DestId::Cutoff, 1.0);
    let x = render(&mut e);
    let ratio = peak_edge_d2_ratio(&x, 256);
    eprintln!("cutoff: peak edge/interior d² ratio = {ratio:.2}");
    assert!(
        ratio < 8.0,
        "square LFO→Cutoff spikes block-edge d² by {ratio}× — ladder ramp regressed"
    );
}

#[test]
fn output_stays_finite_under_worst_case_flips() {
    for dest in [DestId::Amp, DestId::Pitch, DestId::XModSweep, DestId::Pwm, DestId::Cutoff] {
        let mut e = engine_with_route(dest, 1.0);
        let x = render(&mut e);
        assert!(x.iter().all(|s| s.is_finite()), "non-finite output for {dest:?}");
    }
}
