//! Declick regression tests (E035 / 0192).
//!
//! Two guarantees for the toggle-declick work (0190 FX bypass crossfade, 0191
//! oversampling-change crossfade):
//!
//! 1. **No hard step at a toggle edge or an OS change.** A 4th-difference click
//!    detector (`worst_d4`, the probe vxn-2's declick harness uses) suppresses
//!    the tone's own smooth slew by f^4 while preserving the full amplitude of a
//!    slope discontinuity. The metric that isolates the *switch* — as opposed to
//!    an effect's own DSP once it is running — is the `d4` straddling the edge
//!    sample itself (the "join"). A hard switch lands a full `|wet − dry|` step
//!    there and blows the join by ~3 orders of magnitude (measured: a hard
//!    phaser switch gives join ≈ 2.3e-1 vs the crossfade's ≈ 1.6e-4). The join
//!    must stay within a small factor of the steady-state tone either side.
//!
//!    Note what this does *not* claim: an LFO-modulated effect starting cold has
//!    its own onset transient a few ms *after* the switch (a chorus/delay line
//!    filling, a limiter grabbing). That is the effect doing its job, not a
//!    toggle click; the crossfade attenuates it (it lands under low wet weight)
//!    but cannot remove it without keeping the effect warm while bypassed —
//!    which the CPU-gated design deliberately avoids. So we assert on the join.
//!
//! 2. **Bit-exact when idle.** With every FX flag off and never toggled, the
//!    output is byte-for-byte independent of the FX knobs — the zero-cost
//!    passthrough is the unchanged fast path, untouched by the fade machinery
//!    (the sample-exact-vs-absent contract the fades must not break).

use vxn_engine::{GlobalParam, Synth};

const SR: f32 = 48_000.0;
const BLK: usize = 64;

// Timeline (blocks): [0,ON_B) off, engage at ON_B, [ON_B,OFF_B) on, disengage
// at OFF_B, [OFF_B,END) off. The ~10 ms FX fade spans ~8 blocks of 64, so each
// segment leaves a settled tail for the steady-state baseline.
const ON_B: usize = 16;
const OFF_B: usize = 48;
const END: usize = 84;
// The ~10 ms fade spans ~8 blocks; a 10-block plateau guard keeps the settled
// baseline windows clear of the fade tail.
const FADE_GUARD: usize = 10;

/// 4th-difference click detector: max `|b[i+2] − 4b[i+1] + 6b[i] − 4b[i−1] +
/// b[i−2]|` over `range` (caller ensures 2 ≤ range.start, range.end + 2 ≤ len).
fn worst_d4(buf: &[f32], range: std::ops::Range<usize>) -> f64 {
    range
        .map(|i| {
            (buf[i + 2] - 4.0 * buf[i + 1] + 6.0 * buf[i] - 4.0 * buf[i - 1] + buf[i - 2]).abs()
                as f64
        })
        .fold(0.0, f64::max)
}

/// Worst `d4` over the whole settled OFF and settled ON plateaus (not a short
/// window). vxn-1's FX are LFO-modulated (phaser/chorus sweep, limiter pumps,
/// reverb wet is dense), so their `d4` fluctuates across the modulation cycle; a
/// short window undersamples it. The max over the full plateau is the honest
/// floor — the tone's own worst-case slew either side of the edges.
fn steady_floor(buf: &[f32]) -> f64 {
    let off = worst_d4(buf, 2 * BLK..(ON_B - 1) * BLK);
    let on = worst_d4(buf, (ON_B + FADE_GUARD) * BLK..(OFF_B - 1) * BLK);
    off.max(on)
}

/// `d4` straddling one edge sample — the discontinuity a hard switch introduces
/// and the crossfade removes. Six samples centred on the join cover the kernel's
/// ±2 reach either side of the edge.
fn join_d4(buf: &[f32], edge_block: usize) -> f64 {
    worst_d4(buf, edge_block * BLK - 3..edge_block * BLK + 3)
}

/// Boot with every master FX off (so the fades prime to off), apply `cfg`, hold
/// a chord, and render the toggle timeline for `flag`. Returns the mono-L buffer.
fn render_toggle(flag: GlobalParam, cfg: &[(GlobalParam, f32)]) -> Vec<f32> {
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
    let mut buf = Vec::with_capacity(END * BLK);
    for b in 0..END {
        if b == ON_B {
            synth.params_mut().global_mut().set(flag, 1.0);
        } else if b == OFF_B {
            synth.params_mut().global_mut().set(flag, 0.0);
        }
        synth.process(&mut l, &mut r);
        buf.extend_from_slice(&l);
    }
    buf
}

/// The crossfade is click-free when both toggle joins stay within `k×` the
/// steady tone — i.e. no hard `|wet − dry|` step at either switch sample.
fn assert_join_clean(name: &str, buf: &[f32], k: f64) {
    let steady = steady_floor(buf);
    let join_on = join_d4(buf, ON_B);
    let join_off = join_d4(buf, OFF_B);
    eprintln!("{name}: steady={steady:.3e} join_on={join_on:.3e} join_off={join_off:.3e}");
    assert!(
        join_on <= k * steady,
        "{name}: on-edge join |d4| {join_on:.3e} ≫ steady {steady:.3e}: hard switch clicks",
    );
    assert!(
        join_off <= k * steady,
        "{name}: off-edge join |d4| {join_off:.3e} ≫ steady {steady:.3e}: hard switch clicks",
    );
}

#[test]
fn phaser_toggle_is_click_free() {
    let buf = render_toggle(
        GlobalParam::PhaserOn,
        &[(GlobalParam::PhaserMix, 1.0), (GlobalParam::PhaserDepth, 0.8)],
    );
    assert_join_clean("phaser", &buf, 4.0);
}

#[test]
fn chorus_toggle_is_click_free() {
    let buf = render_toggle(
        GlobalParam::ChorusOn,
        &[(GlobalParam::ChorusMix, 0.8), (GlobalParam::ChorusDepth, 0.8)],
    );
    assert_join_clean("chorus", &buf, 4.0);
}

#[test]
fn delay_toggle_is_click_free() {
    let buf = render_toggle(
        GlobalParam::DelayOn,
        &[
            (GlobalParam::DelayMix, 0.6),
            (GlobalParam::DelayFeedback, 0.4),
        ],
    );
    assert_join_clean("delay", &buf, 4.0);
}

#[test]
fn reverb_toggle_is_click_free() {
    let buf = render_toggle(GlobalParam::ReverbOn, &[(GlobalParam::ReverbMix, 0.7)]);
    assert_join_clean("reverb", &buf, 4.0);
}

#[test]
fn limiter_toggle_is_click_free() {
    // Drive the bus hard so the limiter actually acts on engage.
    let buf = render_toggle(GlobalParam::LimiterOn, &[(GlobalParam::MasterVolume, 1.0)]);
    assert_join_clean("limiter", &buf, 4.0);
}

#[test]
fn oversampling_change_is_declicked() {
    // Boot at 1× (Oversample index 0), change to 4× (index 2) mid-render. The
    // decimator FIR state is rate-specific and must reset, which makes the new
    // decimator emit near-zero for its first sample — a hard step down from the
    // pre-switch level. A fade-in from zero *cannot* hide that step (it is the
    // step); 0191 instead crossfades from the frozen pre-switch level into the
    // rebuilt output. That removes the level step; a residual first-sample slope
    // kink (the held level is flat, the pre-switch waveform was sloping) remains
    // but is ~2 orders of magnitude below the raw hard-reset click.
    //
    // Measured: raw decimator reset (no crossfade) gives join d4 ≈ 1.2; the
    // crossfade brings it to ≈ 1.5e-2. We assert the strong improvement holds —
    // an absolute bound comfortably below the raw click and above the residual —
    // and defer final audibility to the Reaper listen (0192).
    const CHG: usize = 24;
    const RAW_RESET_CLICK: f64 = 1.2; // documented reference: reset with no crossfade
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
        g.set(GlobalParam::Oversample, 0.0); // 1×
    }
    for &n in &[48u8, 55, 60] {
        synth.note_on(n, 0.9);
    }
    let mut l = [0.0f32; BLK];
    let mut r = [0.0f32; BLK];
    let mut buf = Vec::with_capacity(END * BLK);
    for b in 0..END {
        if b == CHG {
            synth.params_mut().global_mut().set(GlobalParam::Oversample, 2.0); // 4×
        }
        synth.process(&mut l, &mut r);
        buf.extend_from_slice(&l);
    }
    let steady = worst_d4(&buf, 2 * BLK..(CHG - 1) * BLK)
        .max(worst_d4(&buf, (CHG + FADE_GUARD) * BLK..(END - 2) * BLK));
    let change = join_d4(&buf, CHG);
    eprintln!("os-change: steady={steady:.3e} change={change:.3e}");
    // At least an order of magnitude below the raw hard-reset click.
    assert!(
        change < RAW_RESET_CLICK / 10.0,
        "os-change join |d4| {change:.3e} not ≪ raw reset click {RAW_RESET_CLICK:.1e}: \
         crossfade regressed",
    );
    // And a modest absolute ceiling (tuned to the measured ~1.5e-2 residual).
    assert!(
        change < 5.0e-2,
        "os-change join |d4| {change:.3e} exceeds the 5e-2 declick ceiling",
    );
}

#[test]
fn all_fx_off_is_bit_exact_across_fx_params() {
    // With every FX flag off and never toggled, the render is the zero-cost
    // passthrough and must be byte-for-byte independent of the FX knobs — the
    // fade machinery must not perturb the fast path. Oversample is held equal
    // (it is the synth path, not a master FX).
    let off = |g: &mut vxn_engine::GlobalValues| {
        for f in [
            GlobalParam::PhaserOn,
            GlobalParam::ChorusOn,
            GlobalParam::DelayOn,
            GlobalParam::ReverbOn,
            GlobalParam::LimiterOn,
        ] {
            g.set(f, 0.0);
        }
    };

    let render = |cfg: &[(GlobalParam, f32)]| -> Vec<f32> {
        let mut synth = Synth::new(SR);
        {
            let g = synth.params_mut().global_mut();
            off(g);
            for &(p, v) in cfg {
                g.set(p, v);
            }
        }
        for &n in &[48u8, 55, 60] {
            synth.note_on(n, 0.9);
        }
        let mut l = [0.0f32; BLK];
        let mut r = [0.0f32; BLK];
        let mut buf = Vec::with_capacity(48 * BLK * 2);
        for _ in 0..48 {
            synth.process(&mut l, &mut r);
            for i in 0..BLK {
                buf.push(l[i]);
                buf.push(r[i]);
            }
        }
        buf
    };

    let reference = render(&[]);
    let varied = render(&[
        (GlobalParam::PhaserMix, 1.0),
        (GlobalParam::PhaserDepth, 1.0),
        (GlobalParam::ChorusMix, 1.0),
        (GlobalParam::ChorusDepth, 1.0),
        (GlobalParam::DelayMix, 1.0),
        (GlobalParam::DelayFeedback, 0.7),
        (GlobalParam::ReverbMix, 1.0),
        (GlobalParam::ReverbSize, 1.0),
    ]);

    assert_eq!(reference.len(), varied.len());
    for (i, (a, b)) in reference.iter().zip(&varied).enumerate() {
        assert_eq!(
            a.to_bits(),
            b.to_bits(),
            "FX-off passthrough not bit-identical at sample {i} ({a} vs {b}) — \
             an FX param leaked into the off path",
        );
    }
}
