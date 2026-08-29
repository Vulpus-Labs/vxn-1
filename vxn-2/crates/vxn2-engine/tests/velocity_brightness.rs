//! Velocity must reach an operator's *brightness*, not just its loudness.
//!
//! On the reference, velocity is a signed level offset added to the operator's
//! level accumulator (ADR 0010), so a hard strike lifts a high-sensitivity
//! operator **above** its nominal level rather than merely failing to attenuate
//! it. On a modulator, level is modulation index — so the touch response of an
//! FM patch lives here, and a velocity curve that can only attenuate flattens
//! it out.
//!
//! `Electric Boogaloo` is the case that found the bug: a 14:1 tine on `op2`,
//! the bank's clearest high-sensitivity modulator, whose ting had gone missing.
//! Measured on the 15th harmonic — `op1` is a ratio-1 carrier and `op2` a
//! ratio-14 modulator, so the first upper sideband lands on `1 + 14`.
//!
//! The assertion is on the *span* between a soft and a hard strike rather than
//! an absolute level: it is the velocity dependence that regressed, and a span
//! survives re-voicing of the preset in a way an absolute figure would not.

mod common;

use vxn2_engine::engine::Engine;
use vxn2_engine::factory::factory;
use vxn2_engine::preset::from_toml_str;
use vxn2_engine::shared::{ParamModel, SharedParams};
use vxn_core_dsp::control::CONTROL_BLOCK;

const SR: f32 = 48_000.0;
const BLK: usize = CONTROL_BLOCK;
const KEY: u8 = 60;

/// Level of harmonic `h` of `KEY`, in dB relative to the fundamental, measured
/// over the first 100 ms of a note struck at `velocity`.
fn harmonic_db(velocity: u8, h: u32) -> f64 {
    let fp = factory()
        .into_iter()
        .find(|p| p.name == "Electric Boogaloo")
        .expect("Electric Boogaloo is in the factory bank");
    let (_meta, blob, _warn) = from_toml_str(fp.contents).expect("preset parses");
    let shared = SharedParams::new();
    shared.load_bytes(&blob).expect("preset loads");

    let mut e = Engine::new(SR, BLK);
    e.snapshot_params(&shared);
    e.params_mut().master.limiter_on = false;
    // The reverb tail would smear the harmonic readout; the operators are what
    // is under test.
    e.params_mut().reverb.on = false;
    e.params_mut().reverb.mix = 0.0;
    e.apply_block_params();

    let (mut l, mut r) = ([0.0_f32; BLK], [0.0_f32; BLK]);
    for _ in 0..40 {
        e.process_block(&mut l, &mut r);
    }
    e.note_on(KEY, velocity);

    let n = (SR * 0.1) as usize;
    let mut x = Vec::with_capacity(n + BLK);
    while x.len() < n {
        e.process_block(&mut l, &mut r);
        for i in 0..BLK {
            x.push(0.5 * (l[i] + r[i]));
        }
    }

    let f0 = 440.0 * 2_f32.powf((KEY as f32 - 69.0) / 12.0);
    let mag = |freq: f32| -> f64 {
        let om = 2.0 * std::f64::consts::PI * freq as f64 / SR as f64;
        let (mut re, mut im) = (0.0_f64, 0.0_f64);
        for (i, &s) in x.iter().enumerate() {
            // Hann window: the harmonics are close enough together that
            // rectangular leakage would swamp a -50 dB sideband.
            let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / x.len() as f64).cos();
            let p = om * i as f64;
            re += s as f64 * w * p.cos();
            im -= s as f64 * w * p.sin();
        }
        (re * re + im * im).sqrt()
    };
    20.0 * (mag(f0 * h as f32) / mag(f0).max(1e-12)).log10()
}

/// The tine's velocity span. Before velocity became a level offset this
/// measured ~14 dB, the curve being unable to exceed nominal; it now measures
/// ~29 dB. The bound sits between the two so the regression cannot creep back.
#[test]
fn tine_brightness_tracks_velocity() {
    let soft = harmonic_db(40, 15);
    let hard = harmonic_db(110, 15);
    let span = hard - soft;
    assert!(
        span > 22.0,
        "15th-harmonic tine spans only {span:.1} dB from vel 40 ({soft:.1}) to \
         vel 110 ({hard:.1}); velocity is not reaching modulation index"
    );
}

// Monotonicity is deliberately *not* asserted here. `Electric Boogaloo` is
// algo 5 — three modulator/carrier pairs — and op4 and op6 are `vel-sens 6`,
// so their chains also put energy near the 15th harmonic and interfere with
// op2's sideband. The measured ratio consequently dips at some velocities (vel
// 60 reads below vel 40) without the underlying law being non-monotone. The
// law is asserted where it holds exactly, on the offset itself:
// `vxn2_dsp::level::tests::vel_level_offset_is_monotone_in_velocity`.
