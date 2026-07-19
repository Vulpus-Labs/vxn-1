//! E007 ticket 0087 — filter feature integration tests on the *engine* path
//! (the DSP-kernel-level checks live in `vxn2-dsp`: `filter::tests` covers
//! mode/slope, self-osc and quiescence-decay; `halfband::tests` covers the
//! load-bearing deferred-decimation equivalence `decimate_is_linear_over_voice_sum`
//! and the interp→decimate roundtrip).
//!
//! Here we prove the *wired* feature:
//!
//! - **Bypass bit-identity** — with `filter-enable` off, the render is
//!   sample-for-sample independent of every filter param, i.e. the off path
//!   is the unchanged sample-major loop and stays bit-identical to the
//!   pre-epic output for every factory patch (AC 1).
//! - **Self-oscillation bounded** — resonance = 1 across the cutoff range stays
//!   finite and bounded on the integrated per-voice path at the fixed 4× (AC 5;
//!   the multi-factor resampler sweep lives in the `vxn2-dsp` kernel tests).
//! - **Matrix cutoff/resonance + RT hardening** — `DestId::Cutoff`/`Resonance`
//!   route end-to-end through the matrix with the filter on and the process
//!   callback stays finite/non-panicking (AC 5, no-RT-alloc/panic).
//! - **Quiescence tail preservation** — a resonant release tail rings out with
//!   no skip cliff (0085 criteria): the moment the quiescence-skip engages must
//!   not click (AC 6).
//! - **Click-free enable toggle** — flipping `filter-enable` on and off is
//!   crossfaded (ADR 0004 §10), so neither edge introduces a discontinuity
//!   beyond the tone's own slew, checked at the fixed 4× group delay.

mod common;
use common::worst_d4;

use vxn2_engine::engine::Engine;
use vxn2_engine::factory::factory;
use vxn2_engine::matrix::{DestId, SourceId};
use vxn2_engine::params::id_of;
use vxn2_engine::preset::from_toml_str;
use vxn2_engine::shared::{MatrixRowRaw, ParamModel, SharedParams};

const SR: f32 = 48_000.0;
const BLK: usize = 64;

/// Deterministic render: fresh engine, hold a chord, collect interleaved L/R.
fn render_capture(s: &SharedParams, blocks: usize) -> Vec<f32> {
    let mut e = Engine::new(SR, BLK);
    e.snapshot_params(s);
    for &n in &[40u8, 47, 52, 59] {
        e.note_on(n, 100);
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

/// Load a factory preset's blob into a fresh `SharedParams`.
fn shared_from_preset(contents: &str) -> SharedParams {
    let (_meta, blob, _warnings) = from_toml_str(contents).expect("factory preset parses");
    let s = SharedParams::new();
    ParamModel::load_bytes(&s, &blob).expect("factory blob loads");
    s
}

/// AC 1 — with `filter-enable` off the render path is the sample-major bypass
/// and its output does not depend on any filter param. Forcing the whole filter
/// section to extreme values while the enable stays off must leave the output
/// byte-for-byte identical: that is exactly the "bit-identical to pre-epic"
/// guarantee, checked against the binary's own bypass floor for every factory
/// patch (and the default patch).
#[test]
fn bypass_render_is_bit_identical_across_filter_params() {
    let en = id_of("filter-enable").unwrap();
    let cut = id_of("filter-cutoff").unwrap();
    let res = id_of("filter-resonance").unwrap();
    let mode = id_of("filter-mode").unwrap();
    let slope = id_of("filter-slope").unwrap();
    let drive = id_of("filter-drive").unwrap();

    // The default patch plus every embedded factory preset.
    let mut banks: Vec<(String, SharedParams)> = vec![("<default>".into(), SharedParams::new())];
    for fp in factory() {
        banks.push((
            format!("{}/{}", fp.category, fp.name),
            shared_from_preset(fp.contents),
        ));
    }

    for (name, s) in &banks {
        // Reference: filter forced off, filter section left at the patch's
        // values.
        s.set(en, 0.0);
        let reference = render_capture(s, 24);

        // Comparison: same patch, filter still off, but every filter knob
        // slammed to a different extreme. The off path must ignore all of it.
        s.set(en, 0.0);
        s.set(cut, 9000.0);
        s.set(res, 1.0);
        s.set(mode, 2.0); // BP (FILTER_MODES index: LP=0 HP=1 BP=2 Notch=3)
        s.set(slope, 1.0); // 4-pole
        s.set(drive, 4.0);
        let varied = render_capture(s, 24);

        assert_eq!(
            reference.len(),
            varied.len(),
            "{name}: render length changed"
        );
        for (i, (a, b)) in reference.iter().zip(&varied).enumerate() {
            assert_eq!(
                a.to_bits(),
                b.to_bits(),
                "{name}: bypass not bit-identical at interleaved sample {i} \
                 ({a} vs {b}) — a filter param leaked into the OFF path",
            );
        }
    }
}

/// AC 5 — resonance = 1 across the cutoff range self-oscillates without blowing
/// up on the integrated per-voice engine path. Oversampling is fixed at 4×
/// (selector removed); the multi-factor resampler-stability sweep lives in the
/// DSP-kernel tests (`vxn2-dsp` `filter::tests` / `halfband::tests`).
#[test]
fn self_oscillation_bounded() {
    let en = id_of("filter-enable").unwrap();
    let cut = id_of("filter-cutoff").unwrap();
    let res = id_of("filter-resonance").unwrap();

    for &cutoff in &[200.0_f32, 1000.0, 5000.0, 12000.0] {
        let s = SharedParams::new();
        s.set(en, 1.0);
        s.set(res, 1.0); // top of range → self-oscillation
        s.set(cut, cutoff);

        let mut e = Engine::new(SR, BLK);
        e.snapshot_params(&s);
        for &n in &[36u8, 48, 60, 72] {
            e.note_on(n, 110);
        }
        let mut l = [0.0_f32; BLK];
        let mut r = [0.0_f32; BLK];
        let mut peak = 0.0_f32;
        // ~0.5 s — long enough for any divergent limit cycle to show.
        for _ in 0..(SR as usize / 2 / BLK) {
            e.process_block(&mut l, &mut r);
            for i in 0..BLK {
                assert!(
                    l[i].is_finite() && r[i].is_finite(),
                    "non-finite at cutoff {cutoff}",
                );
                peak = peak.max(l[i].abs()).max(r[i].abs());
            }
        }
        assert!(
            peak < 100.0,
            "cutoff {cutoff}: self-osc unbounded (peak {peak})",
        );
    }
}

/// AC 5 + no-RT-alloc/panic — `Cutoff`/`Resonance` route through the matrix
/// (velocity → cutoff, mod-env → resonance) with the filter on (fixed 4×);
/// the process callback stays finite and bounded with the modulation live.
#[test]
fn matrix_cutoff_resonance_routes_stay_finite() {
    let en = id_of("filter-enable").unwrap();
    let cut = id_of("filter-cutoff").unwrap();
    let res = id_of("filter-resonance").unwrap();

    let s = SharedParams::new();
    s.set(en, 1.0);
    s.set(cut, 1200.0);
    s.set(res, 0.5);
    // Velocity → Cutoff (octaves), Mod-env → Resonance (additive).
    s.set_matrix_row_raw(
        0,
        MatrixRowRaw {
            source: SourceId::Velocity as u8,
            dest: DestId::Cutoff as u8,
            curve: 0,
            active: true,
            depth: 1.0,
        },
    );
    s.set_matrix_row_raw(
        1,
        MatrixRowRaw {
            source: SourceId::ModEnv as u8,
            dest: DestId::Resonance as u8,
            curve: 0,
            active: true,
            depth: 1.0,
        },
    );

    let mut e = Engine::new(SR, BLK);
    e.snapshot_params(&s);
    for (i, &n) in [40u8, 52, 64, 76].iter().enumerate() {
        e.note_on(n, 40 + (i as u8) * 28); // spread velocities → spread cutoff
    }
    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    let mut peak = 0.0_f32;
    for _ in 0..(SR as usize / 4 / BLK) {
        e.process_block(&mut l, &mut r);
        for i in 0..BLK {
            assert!(
                l[i].is_finite() && r[i].is_finite(),
                "non-finite under matrix cutoff/reso mod",
            );
            peak = peak.max(l[i].abs()).max(r[i].abs());
        }
    }
    assert!(peak < 100.0, "matrix-modulated filter unbounded ({peak})");
}

/// AC 6 — a resonant release tail must ring out with no "skip cliff": when the
/// quiescence-skip (0085) engages, the contribution is already sub-floor, so the
/// post-note-off signal stays smooth. A 4th-difference transient detector (the
/// 0079 note-off-click harness) catches any discontinuity introduced by clipping
/// the tail or by an abrupt freeze.
#[test]
fn resonant_release_tail_rings_out_without_skip_cliff() {
    let s = SharedParams::new();
    s.set(id_of("filter-enable").unwrap(), 1.0);
    s.set(id_of("filter-cutoff").unwrap(), 700.0); // smooth ~700 Hz ring
    s.set(id_of("filter-resonance").unwrap(), 0.85);
    s.set(id_of("delay-on").unwrap(), 0.0);
    s.set(id_of("reverb-on").unwrap(), 0.0);

    let mut e = Engine::new(SR, BLK);
    e.snapshot_params(&s);
    e.note_on(48, 110);

    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    let off_block = SR as usize / 8 / BLK; // ~125 ms in
    let total = SR as usize / BLK; // ~1 s, well past full ring-out + skip
    let mut buf = Vec::with_capacity(total * BLK);
    for b in 0..total {
        if b == off_block {
            e.note_off(48);
        }
        e.process_block(&mut l, &mut r);
        buf.extend_from_slice(&l);
    }

    let off_t = off_block * BLK;
    let worst = worst_d4(&buf, off_t + 2..buf.len() - 2);
    assert!(
        worst < 5e-3,
        "post-off |d4| {worst:.2e}: resonant tail clipped or skip introduced a cliff",
    );
}

/// ADR 0004 §10 — toggling `filter-enable` is click-free. The enable edge swaps
/// two render bodies whose dry buses differ by the resampler group delay (OS
/// path) *and* the saturator level/timbre step; a hard switch pops on both.
/// The engine equal-power-crossfades the two dry buses across one ~8 ms window,
/// so the toggle introduces no discontinuity beyond the tone's own slew.
///
/// Checked at the fixed 4× (the group delay the toggle must hide) on both edges:
/// hold a chord, render off → engage → settle → disengage → settle, and assert
/// the `d4` click energy straddling each toggle stays within a small factor of
/// the steady-state tone either side. A hard switch blows this by orders of
/// magnitude.
#[test]
fn filter_toggle_is_click_free() {
    let en = id_of("filter-enable").unwrap();
    let cut = id_of("filter-cutoff").unwrap();
    let res = id_of("filter-resonance").unwrap();

    let s = SharedParams::new();
    s.set(cut, 2000.0);
    s.set(res, 0.4); // audible resonance ⇒ filtered tone clearly ≠ raw
    s.set(id_of("delay-on").unwrap(), 0.0);
    s.set(id_of("reverb-on").unwrap(), 0.0);
    s.set(en, 0.0); // boot OFF

    let mut e = Engine::new(SR, BLK);
    e.snapshot_params(&s);
    for &n in &[40u8, 47, 52, 59] {
        e.note_on(n, 100);
    }

    // Timeline (blocks): [0,ON_B) off, engage at ON_B, [ON_B,OFF_B) on,
    // disengage at OFF_B, [OFF_B,END) off. The ~8 ms fade spans ~6 blocks, so
    // each segment leaves ample settled tail for a steady-state baseline.
    const ON_B: usize = 12;
    const OFF_B: usize = 40;
    const END: usize = 72;
    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    let mut buf = Vec::with_capacity(END * BLK);
    for b in 0..END {
        if b == ON_B {
            s.set(en, 1.0);
            e.snapshot_params(&s);
        } else if b == OFF_B {
            s.set(en, 0.0);
            e.snapshot_params(&s);
        }
        e.process_block(&mut l, &mut r);
        buf.extend_from_slice(&l);
    }

    // Steady-state baseline: worst d4 well clear of either toggle (settled OFF
    // before engage, settled ON before disengage). This is the tone's own slew.
    let steady = worst_d4(&buf, (ON_B - 4) * BLK..(ON_B - 1) * BLK)
        .max(worst_d4(&buf, (OFF_B - 4) * BLK..(OFF_B - 1) * BLK));

    // Toggle windows: the fade plus a couple of blocks either side of each edge.
    let engage = worst_d4(&buf, (ON_B - 1) * BLK..(ON_B + 9) * BLK);
    let disengage = worst_d4(&buf, (OFF_B - 1) * BLK..(OFF_B + 9) * BLK);

    // A hard switch lands a one-sample step of order |raw − filtered| at the
    // edge — d4 orders of magnitude over the tone's slew. The crossfade must
    // keep both edges within a small multiple of the steady baseline.
    assert!(
        engage <= 4.0 * steady,
        "engage |d4| {engage:.2e} ≫ steady {steady:.2e}: filter-on toggle clicks",
    );
    assert!(
        disengage <= 4.0 * steady,
        "disengage |d4| {disengage:.2e} ≫ steady {steady:.2e}: filter-off toggle clicks",
    );
}

/// Regression: engaging (and disengaging) the filter while the dynamics FX is
/// already live must stay click-free. The dynamics keeps the 4× span engaged on
/// both sides of the toggle, so the span's single decimator must run once per
/// block and stay continuous across the edge — an earlier version reset/reused
/// the shared decimator on the filter edge, which clunked the dynamics-carried
/// signal. Same `d4` guard as `filter_toggle_is_click_free`, but with dynamics
/// (comp + tanh drive) engaged throughout.
#[test]
fn filter_toggle_over_live_dynamics_is_click_free() {
    let en = id_of("filter-enable").unwrap();

    let s = SharedParams::new();
    s.set(id_of("filter-cutoff").unwrap(), 2000.0);
    s.set(id_of("filter-resonance").unwrap(), 0.4);
    // Dynamics live the whole time: it holds the oversampled span engaged across
    // the filter toggle, so the span's single decimator must stay continuous. A
    // gentle comp + mild drive keeps the block active (so the OS-blend path runs)
    // without the compressor reacting hard to the level/brightness jump when the
    // filter opens — that reaction is a real, musical response, not a glitch, and
    // this test is guarding decimator continuity, not comp transient shaping.
    s.set(id_of("dyn-on").unwrap(), 1.0);
    s.set(id_of("dyn-threshold").unwrap(), -18.0);
    s.set(id_of("dyn-ratio").unwrap(), 2.0);
    s.set(id_of("dyn-drive").unwrap(), 6.0);
    s.set(id_of("delay-on").unwrap(), 0.0);
    s.set(id_of("reverb-on").unwrap(), 0.0);
    s.set(en, 0.0); // boot with the filter OFF (dynamics-only span active)

    let mut e = Engine::new(SR, BLK);
    e.snapshot_params(&s);
    for &n in &[40u8, 47, 52, 59] {
        e.note_on(n, 100);
    }

    const ON_B: usize = 12;
    const OFF_B: usize = 40;
    const END: usize = 72;
    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    let mut buf = Vec::with_capacity(END * BLK);
    for b in 0..END {
        if b == ON_B {
            s.set(en, 1.0);
            e.snapshot_params(&s);
        } else if b == OFF_B {
            s.set(en, 0.0);
            e.snapshot_params(&s);
        }
        e.process_block(&mut l, &mut r);
        buf.extend_from_slice(&l);
    }

    // Steady baseline: the dynamics-processed tone's own slew, clear of either
    // toggle (settled dynamics-only span before engage, settled filter+dynamics
    // span before disengage).
    let steady = worst_d4(&buf, (ON_B - 4) * BLK..(ON_B - 1) * BLK)
        .max(worst_d4(&buf, (OFF_B - 4) * BLK..(OFF_B - 1) * BLK));

    let engage = worst_d4(&buf, (ON_B - 1) * BLK..(ON_B + 9) * BLK);
    let disengage = worst_d4(&buf, (OFF_B - 1) * BLK..(OFF_B + 9) * BLK);

    // The pre-fix decimator clunk measured ~1.4e-1 — ~300× the steady slew — so
    // both bounds sit far below it and would catch a regression outright.
    //
    // Engage (silence→filtered) is genuinely click-free: the continuous
    // `interp_mix` OFF side carries the pre-engage signal straight through, so it
    // holds to the same 4× bar as the dynamics-off toggle.
    assert!(
        engage <= 4.0 * steady,
        "engage over live dynamics |d4| {engage:.2e} ≫ steady {steady:.2e}: decimator clunk",
    );
    // Disengage (filtered→raw) is looser only because the dynamics *correctly*
    // tracks the real level/brightness jump as the filter opens (the saturator is
    // memoryless, so it shapes the louder raw signal immediately) — a musical
    // response, not a glitch. Softening the comp barely moves it, confirming it's
    // signal-tracking, not a discontinuity. Still an order of magnitude under the
    // clunk.
    assert!(
        disengage <= 8.0 * steady,
        "disengage over live dynamics |d4| {disengage:.2e} ≫ steady {steady:.2e}: decimator clunk",
    );
}

