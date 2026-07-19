//! E028 ticket 0147 — dynamics block wired into the FX bus.
//!
//! The DSP-level checks (fade-in/out, detector reset, soft-knee gain reduction,
//! tanh flattening) live in `vxn2-dsp::dynamics::tests`. Here we prove the
//! *wired* feature:
//!
//! - **Bypass bit-identity** — with `dyn-on = 0` the per-sample loop early-exits
//!   inside `DynamicsBlock::process` (gate at
//!   `vxn-2/crates/vxn2-dsp/src/dynamics.rs`), so the bus output is
//!   sample-for-sample independent of every other `dyn-*` param. That's the
//!   "bit-identical to pre-epic" guarantee on the engine path.
//! - **Gain reduction reaches the bus** — with `dyn-on = 1` and a known
//!   threshold/ratio on a hot signal, the post-FX peak drops measurably below
//!   the same render with `dyn-on = 0` (i.e. the block isn't just attached but
//!   actually shaping the bus).

use vxn2_engine::engine::Engine;
use vxn2_engine::params::id_of;
use vxn2_engine::shared::SharedParams;

mod common;
use common::worst_d4;

const SR: f32 = 48_000.0;
const BLK: usize = 64;

/// Deterministic render: fresh engine, hold a chord at high velocity (so the
/// dynamics block has something to chew on), collect interleaved L/R.
fn render_capture(s: &SharedParams, blocks: usize) -> Vec<f32> {
    let mut e = Engine::new(SR, BLK);
    e.snapshot_params(s);
    for &n in &[40u8, 47, 52, 59] {
        e.note_on(n, 120);
    }
    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    let mut out = Vec::with_capacity(blocks * BLK * 2);
    for _ in 0..blocks {
        e.process_block(&mut l, &mut r);
        for i in 0..BLK {
            out.push(l[i]);
            out.push(r[i]);
        }
    }
    out
}

/// With `dyn-on = 0` the per-sample loop's `DynamicsBlock::process` early-exits
/// to a bit-exact passthrough — every other `dyn-*` param is dead air. Slamming
/// them to extremes while the gate stays off must leave the bus output
/// byte-for-byte identical to the dyn-defaulted render.
#[test]
fn bypass_render_is_bit_identical_across_dynamics_params() {
    let on = id_of("dyn-on").unwrap();
    let thresh = id_of("dyn-threshold").unwrap();
    let ratio = id_of("dyn-ratio").unwrap();
    let attack = id_of("dyn-attack").unwrap();
    let release = id_of("dyn-release").unwrap();
    let makeup = id_of("dyn-makeup").unwrap();
    let drive = id_of("dyn-drive").unwrap();
    let mix = id_of("dyn-mix").unwrap();

    let s = SharedParams::new();
    s.set(on, 0.0);
    let reference = render_capture(&s, 24);

    // Same patch, dyn still off, but every other dyn knob slammed to a
    // different extreme. The bypass gate must ignore all of it.
    s.set(on, 0.0);
    s.set(thresh, -60.0);
    s.set(ratio, 20.0);
    s.set(attack, 0.1);
    s.set(release, 5.0);
    s.set(makeup, 24.0);
    s.set(drive, 36.0);
    s.set(mix, 1.0);
    let varied = render_capture(&s, 24);

    assert_eq!(reference.len(), varied.len(), "render length changed");
    for (i, (a, b)) in reference.iter().zip(&varied).enumerate() {
        assert_eq!(
            a.to_bits(),
            b.to_bits(),
            "bypass not bit-identical at interleaved sample {i} ({a} vs {b}) \
             — a dyn-* param leaked into the OFF path",
        );
    }
}

/// `dyn-on = 1` with an aggressive threshold/ratio on a hot signal must drop
/// the post-FX peak below the same render with `dyn-on = 0`. Proves the block
/// isn't just constructed and called — its gain reduction is actually shaping
/// the bus.
#[test]
fn dyn_on_reduces_post_fx_peak() {
    let on = id_of("dyn-on").unwrap();
    let thresh = id_of("dyn-threshold").unwrap();
    let ratio = id_of("dyn-ratio").unwrap();
    let attack = id_of("dyn-attack").unwrap();
    let release = id_of("dyn-release").unwrap();
    let mix = id_of("dyn-mix").unwrap();
    let makeup = id_of("dyn-makeup").unwrap();
    let drive = id_of("dyn-drive").unwrap();

    // Common dynamics params; render once with the gate off, once on.
    let configure = |s: &SharedParams| {
        s.set(thresh, -36.0);
        s.set(ratio, 20.0);
        s.set(attack, 1.0);
        s.set(release, 80.0);
        s.set(mix, 1.0);
        s.set(makeup, 0.0);
        s.set(drive, 0.0);
    };

    let s_off = SharedParams::new();
    configure(&s_off);
    s_off.set(on, 0.0);
    let off_buf = render_capture(&s_off, 64);

    let s_on = SharedParams::new();
    configure(&s_on);
    s_on.set(on, 1.0);
    let on_buf = render_capture(&s_on, 64);

    let peak = |buf: &[f32]| buf.iter().fold(0.0_f32, |m, &x| m.max(x.abs()));
    let peak_off = peak(&off_buf);
    let peak_on = peak(&on_buf);

    assert!(peak_off > 0.05, "off peak {peak_off} too quiet — test signal isn't reaching the bus");
    // 20:1 above −36 dB should pull peaks down significantly. Conservative
    // bound: at least 10% reduction.
    assert!(
        peak_on < peak_off * 0.9,
        "dyn-on did not reduce post-FX peak: off={peak_off}, on={peak_on}",
    );
}

/// Regression: with the filter OFF, turning the dynamics section off must not
/// click. Disengaging the dynamics is the only thing keeping the 4× span alive,
/// so the oversampling stops and its ~L-sample resampler group delay drops from
/// the path. That step lands *late* — when the 30 ms wet fade finally reaches 0
/// and the block goes inactive — and clicks unless the span-settle crossfade
/// bridges it. (With the filter on the span stays up, so this never fires.)
///
/// Hold a chord with dynamics engaged (filter off), switch `dyn-on` off, and
/// render well past the wet fade. The `d4` energy in the window where the span
/// actually drops (past the fade) must stay within a small factor of the tone's
/// own steady slew — a bare latency step spikes it by orders of magnitude.
#[test]
fn dynamics_off_with_filter_off_is_click_free() {
    let dyn_on = id_of("dyn-on").unwrap();

    let s = SharedParams::new();
    // Filter off (default). Dynamics engaged, doing real work so the span is
    // clearly active before we drop it.
    s.set(dyn_on, 1.0);
    s.set(id_of("dyn-threshold").unwrap(), -24.0);
    s.set(id_of("dyn-ratio").unwrap(), 6.0);
    s.set(id_of("dyn-drive").unwrap(), 18.0);
    s.set(id_of("delay-on").unwrap(), 0.0);
    s.set(id_of("reverb-on").unwrap(), 0.0);

    let mut e = Engine::new(SR, BLK);
    e.snapshot_params(&s);
    for &n in &[40u8, 47, 52, 59] {
        e.note_on(n, 110);
    }

    // Blocks: [0, OFF_B) dynamics on; disengage at OFF_B; render to END. The wet
    // fade doesn't reach 0 (and the span doesn't drop) until the mix smoother
    // decays to its snap-to-zero floor — ~14·τ at the 30 ms smoother running at
    // the 4× rate, i.e. ~400 ms ≈ 306 blocks after OFF_B, not one fade-time. The
    // pre-fix click was measured there (block ~346, d4 ~1.8e-1 ≈ 7.5× steady);
    // the window straddles it with margin either side.
    const OFF_B: usize = 40;
    // Local baseline just *before* the drop — same faded-out (dynamics bypassed)
    // regime as the drop itself, so amplitude/EG decay can't bias the ratio.
    const BASE_LO: usize = OFF_B + 230;
    const BASE_HI: usize = OFF_B + 270;
    const DROP_LO: usize = OFF_B + 275;
    const DROP_HI: usize = OFF_B + 335;
    const END: usize = OFF_B + 345;
    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    let mut buf = Vec::with_capacity(END * BLK);
    for b in 0..END {
        if b == OFF_B {
            s.set(dyn_on, 0.0);
            e.snapshot_params(&s);
        }
        e.process_block(&mut l, &mut r);
        buf.extend_from_slice(&l);
    }

    let baseline = worst_d4(&buf, BASE_LO * BLK..BASE_HI * BLK);
    let drop = worst_d4(&buf, DROP_LO * BLK..DROP_HI * BLK);

    // The raw group-delay step spikes d4 to ~1.8e-1 — orders of magnitude over
    // the surrounding slew; the bridged version sits at the baseline. 10× cleanly
    // separates them without false-failing on the fade's own gentle motion.
    assert!(
        drop <= 10.0 * baseline,
        "dyn-off (filter off) span drop |d4| {drop:.2e} ≫ local baseline {baseline:.2e}: \
         group-delay step clicked (span-settle didn't bridge it)",
    );
}

/// Regression: with the filter OFF, turning the dynamics section *on* must not
/// pop. Engaging routes the signal through the 4× span (interp→decim). The engine
/// carries a constant group delay (`SpanDelay` holds the bypass path at the same
/// latency), so there is *no* latency step to click — what remains is only the
/// resampler's fill transient (its interp/decim start from empty on the engage
/// edge), which the `SpanFade` crossfade smears down to a small tick.
///
/// Identity dynamics (ratio 1, drive 0) isolate that fill from the compressor's
/// own attack: engaging changes nothing audible, so any d4 at the edge is the
/// fill alone. (A real comp/sat attack legitimately reshapes the sound on engage
/// — that is not a click and is not what this guards.)
#[test]
fn dynamics_on_with_filter_off_is_click_free() {
    let dyn_on = id_of("dyn-on").unwrap();

    let s = SharedParams::new();
    // Filter off (default). Dynamics enabled but an identity (no comp, no sat),
    // so engaging inserts the span latency without reshaping the signal.
    s.set(dyn_on, 0.0);
    s.set(id_of("dyn-ratio").unwrap(), 1.0);
    s.set(id_of("dyn-drive").unwrap(), 0.0);
    s.set(id_of("delay-on").unwrap(), 0.0);
    s.set(id_of("reverb-on").unwrap(), 0.0);

    let mut e = Engine::new(SR, BLK);
    e.snapshot_params(&s);
    for &n in &[40u8, 47, 52, 59] {
        e.note_on(n, 110);
    }

    // Hold with the dynamics off (base rate), engage at ON_B, render on. The
    // engage bridge is an ~8 ms window (~6 blocks); the latency step, if
    // unbridged, lands in the block right at ON_B.
    const ON_B: usize = 40;
    const END: usize = ON_B + 20;
    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    let mut buf = Vec::with_capacity(END * BLK);
    for b in 0..END {
        if b == ON_B {
            s.set(dyn_on, 1.0);
            e.snapshot_params(&s);
        }
        e.process_block(&mut l, &mut r);
        buf.extend_from_slice(&l);
    }

    // Straddle the engage edge, covering the bridge window.
    let engage = worst_d4(&buf, (ON_B - 1) * BLK..(ON_B + 9) * BLK);
    // The signal peak, so the bound is a fraction of the actual level.
    let peak = buf.iter().fold(0.0_f32, |m, &x| m.max(x.abs())) as f64;

    // Unbridged, engaging steps the signal path as a one-sample discontinuity:
    // d4 ≈ 1.8 (a full-scale pop, ~0.86× the signal peak). Constant latency +
    // the `SpanFade` crossfade cut it to ~0.013 (~0.6 % of peak) — the residual
    // resampler-fill tick. Bound at 5 % of peak: clears the residual with ~8×
    // margin, and a regression back to the pop blows clean through it.
    assert!(
        engage <= 0.05 * peak,
        "dyn-on (filter off) engage |d4| {engage:.2e} ≫ 5% of peak {peak:.2e}: \
         engage popped (constant-latency + SpanFade should leave only the fill)",
    );
}

