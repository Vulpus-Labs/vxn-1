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
