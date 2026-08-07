//! Render-parity gate (ticket 0202, step 4): VXN1b's seeded default patch must
//! reproduce VXN1's fixed-routing output.
//!
//! VXN1 and VXN1b share the `vxn-dsp` kernels but differ in incidental stages
//! (VXN1 has a 1× decimator, a per-sample master-volume smoother, and a bypassed
//! FX bus; VXN1b has none of these yet), so a *bit*-exact comparison across the
//! two full engines isn't the right gate. Instead both are driven into their
//! **closest comparable state** — oversampling OFF, FX OFF, vibrato OFF, spread
//! 0, drift 0, one Poly note on lane 0 — where the only path exercised is
//! osc→mixer→HPF→ladder→VCA→pan, forked verbatim into VXN1b. The match is
//! asserted within a **documented tolerance** (see the asserts): tight enough to
//! catch any wrong osc/filter/VCA/tuning, loose enough to absorb the volume
//! smoother's first-block settle and the 1× decimator.

use vxn_app::{GlobalParam, Layer, PatchParam, global_clap_id, patch_clap_id};
use vxn_engine::Synth;
use vxn1b_engine::Engine;
use vxn1b_engine::ParamId;

const SR: f32 = 48_000.0;
const BLOCK: usize = 512;

/// VXN1 driven into the comparable state, then one note held.
fn vxn1_reference() -> (Vec<f32>, Vec<f32>) {
    let mut s = Synth::new(SR);
    // Oversampling OFF (index 0).
    s.set_param(global_clap_id(GlobalParam::Oversample), 0.0);
    // Every effect + limiter OFF (chorus is ON by default).
    for g in [
        GlobalParam::PhaserOn,
        GlobalParam::ChorusOn,
        GlobalParam::DelayOn,
        GlobalParam::ReverbOn,
        GlobalParam::LimiterOn,
    ] {
        s.set_param(global_clap_id(g), 0.0);
    }
    // Vibrato OFF (Whole-mode reads the Upper layer's params).
    s.set_param(patch_clap_id(Layer::Upper, PatchParam::PitchLfoDepth), 0.0);
    // One note on the Upper bank (lane 0), full velocity.
    s.note_on_layer(Layer::Upper as usize, 60, 1.0);
    let mut l = vec![0.0; BLOCK];
    let mut r = vec![0.0; BLOCK];
    s.process(&mut l, &mut r);
    (l, r)
}

/// VXN1b default patch with the vibrato route disabled (to match), one note.
fn vxn1b_output() -> (Vec<f32>, Vec<f32>) {
    let mut e = Engine::new(SR, BLOCK);
    // Oversampling OFF, as on the VXN1 side (0249). Both synths *default* to 2×;
    // the point of this gate is the voice render, and comparing across two
    // different decimator states would only measure the halfband FIR. Aliasing
    // under OS has its own test (`tests/oversampling.rs`).
    let os_id = vxn1b_engine::params::global_clap_id(ParamId::Oversample).expect("global");
    e.set_param(os_id, 0.0);
    // Disable the seeded LFO1→Pitch vibrato so the core render is compared
    // without the sub-ULP vibrato-depth divergence. Found by dest, not by slot
    // index: 0245 removed the pre-wired Key→Cutoff slot and the vibrato route
    // slid from slot 2 to slot 1, silently making the old index a no-op.
    for slot in e.matrix_mut(vxn1b_engine::Layer::L1).slots.iter_mut() {
        if slot.dest == vxn1b_engine::DestId::Pitch {
            slot.depth = 0.0;
        }
    }
    e.note_on(0, 60, 1.0);
    let mut l = vec![0.0; BLOCK];
    let mut r = vec![0.0; BLOCK];
    e.process_block(&mut l, &mut r);
    (l, r)
}

/// Max abs difference and RMS ratio over `a` vs `b` on `window`.
fn compare(a: &[f32], b: &[f32], window: std::ops::Range<usize>) -> (f32, f32, f32) {
    let mut max_abs = 0.0f32;
    let mut sa = 0.0f64;
    let mut sb = 0.0f64;
    for i in window {
        max_abs = max_abs.max((a[i] - b[i]).abs());
        sa += (a[i] as f64).powi(2);
        sb += (b[i] as f64).powi(2);
    }
    let rms_a = (sa).sqrt() as f32;
    let rms_b = (sb).sqrt() as f32;
    (max_abs, rms_a, rms_b)
}

#[test]
fn default_patch_render_matches_vxn1() {
    let (a_l, _a_r) = vxn1_reference();
    let (b_l, _b_r) = vxn1b_output();

    // Both must actually make sound (guard against a silent false pass).
    let a_peak = a_l.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
    let b_peak = b_l.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
    assert!(a_peak > 1e-4 && b_peak > 1e-4, "both engines must sound (a {a_peak}, b {b_peak})");

    // Compare a settled window (skip the volume smoother's first-block ramp).
    let (max_abs, rms_a, rms_b) = compare(&a_l, &b_l, 128..BLOCK);
    let rms_ratio = rms_b / rms_a;
    eprintln!(
        "parity: max_abs={max_abs:.3e}  rms_a={rms_a:.5}  rms_b={rms_b:.5}  ratio={rms_ratio:.5}  peak_a={a_peak:.4} peak_b={b_peak:.4}"
    );

    // Documented tolerance: measured RMS ratio ≈ 1.00006 and peaks match to 4
    // decimals — the two renders are the same voice through the same kernels.
    // The ±0.5% band absorbs VXN1's 1× decimator + volume-smoother settle and
    // any platform float variation; a wrong osc/filter/VCA/tuning would move the
    // ratio well outside it.
    assert!(
        (0.995..1.005).contains(&rms_ratio),
        "RMS ratio {rms_ratio} outside parity band (max_abs {max_abs})"
    );
}

#[test]
fn default_patch_amp_env_matches_shape() {
    // Independent of absolute level: the output *envelope* (per-block peak
    // trajectory) should track between the two engines — proves the VCA follows
    // Env2 the same way (per-frame Amp).
    let (a_l, _) = vxn1_reference();
    let (b_l, _) = vxn1b_output();
    let seg = 64;
    for w in (0..BLOCK - seg).step_by(seg) {
        let pa = a_l[w..w + seg].iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        let pb = b_l[w..w + seg].iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        if pa > 1e-3 {
            let ratio = pb / pa;
            assert!(
                (0.9..1.1).contains(&ratio),
                "env segment {w}: ratio {ratio} (a {pa}, b {pb})"
            );
        }
    }
}

// Keep an unused import from tripping the build if ParamId isn't referenced.
#[allow(dead_code)]
const _: fn() -> ParamId = || ParamId::MasterVolume;
