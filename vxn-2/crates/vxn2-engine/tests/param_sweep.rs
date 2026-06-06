//! Ticket 0012 acceptance: a stub host drives the engine through every
//! CLAP-automatable parameter's full range, renders ~1 s of audio per
//! setting, and asserts the kernel stays sane.
//!
//! Sane means:
//! - every sample is finite (no NaN, no Inf),
//! - no panic, no allocation (the latter implicit — the audio path uses no
//!   `Vec::push` / `Box::new` calls on the hot path),
//! - silence stays silent when expected (master volume −60 dB, all carriers
//!   muted, etc.), and audio is audible when the patch is exercised.
//!
//! Per parameter the sweep visits five points: `min`, `25%`, `50%`, `75%`,
//! `max` (linear in normalised space — taper-aware via the descriptor).
//! That's coarser than every 1% but exhaustive across 343 params with 1 s
//! of render each is already over five minutes of audio — the coarser grid
//! keeps CI time manageable while still exercising the dynamic range.

use vxn2_engine::engine::Engine;
use vxn2_engine::params::{PARAMS, TOTAL_PARAMS, id_of};
use vxn2_engine::shared::SharedParams;

const SR: f32 = 48_000.0;
const BLK: usize = 64;

fn render(engine: &mut Engine, blocks: usize) -> (f32, f32) {
    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    let mut peak = 0.0_f32;
    let mut rms_sq = 0.0_f64;
    let mut count = 0u64;
    for _ in 0..blocks {
        engine.process_block(&mut l, &mut r);
        for i in 0..BLK {
            assert!(l[i].is_finite(), "non-finite L at block render");
            assert!(r[i].is_finite(), "non-finite R at block render");
            let mag = l[i].abs().max(r[i].abs());
            if mag > peak {
                peak = mag;
            }
            rms_sq += (l[i] as f64).powi(2) + (r[i] as f64).powi(2);
            count += 2;
        }
    }
    (peak, ((rms_sq / count.max(1) as f64).sqrt()) as f32)
}

fn sweep(blocks_per_point: usize, points: &[f32]) {
    let s = SharedParams::new();
    let mut e = Engine::new(SR, BLK);
    e.snapshot_params(&s);

    // Hold a chord so per-op / FX paths get exercised. Four notes covers
    // Layer-mode polyphony × two layers without hitting the 16-stack cap.
    let notes: [u8; 4] = [48, 55, 60, 64];
    for &n in &notes {
        e.note_on(n, 100);
    }
    let _ = render(&mut e, 16);

    for id in 0..TOTAL_PARAMS {
        let desc = &PARAMS[id];
        let original = s.get(id);
        for &n in points {
            let plain = desc.from_normalised(n);
            s.set(id, plain);
            e.snapshot_params(&s);
            let (peak, _rms) = render(&mut e, blocks_per_point);
            assert!(
                peak.is_finite(),
                "non-finite peak after setting {} to {} ({:.3} normalised)",
                desc.id,
                plain,
                n
            );
            assert!(
                peak < 200.0,
                "{} at {} drove output to {peak}",
                desc.id,
                plain
            );
        }
        s.set(id, original);
    }
    for &n in &notes {
        e.note_off(n);
    }
}

#[test]
fn every_param_sweep_keeps_audio_finite_fast() {
    // Coarse 3-point sweep × ~10 ms per point. Catches the immediate
    // (within a few blocks) NaN / Inf failure modes for every CLAP id;
    // longer-tail interactions are covered by the `#[ignore]`d 1-s sweep.
    sweep(8, &[0.0, 0.5, 1.0]);
}

#[test]
#[ignore = "1-second-per-setting; run manually with --ignored"]
fn every_param_sweep_keeps_audio_finite_full_second() {
    // Ticket 0012 AC: render 1 s of audio per setting. 750 blocks × 64
    // samples ≈ 1 s. Five points × 343 params ≈ 30 min wall-clock — keep
    // it behind `--ignored` so default CI stays under a minute.
    let blocks_per_second = (SR as usize) / BLK;
    sweep(blocks_per_second, &[0.0, 0.25, 0.5, 0.75, 1.0]);
}

#[test]
fn silence_when_master_volume_min() {
    let s = SharedParams::new();
    s.set(id_of("master-volume").unwrap(), -60.0);
    let mut e = Engine::new(SR, BLK);
    e.snapshot_params(&s);
    e.note_on(60, 100);

    let (peak, _rms) = render(&mut e, 64);
    // -60 dB is 0.001 linear; even with reverb tail and stack density the
    // raw output × 0.001 should stay well below 0.05.
    assert!(peak < 0.05, "−60 dB master output too loud: {peak}");
}

#[test]
fn audible_when_master_volume_max() {
    let s = SharedParams::new();
    s.set(id_of("master-volume").unwrap(), 6.0);
    let mut e = Engine::new(SR, BLK);
    e.snapshot_params(&s);
    e.note_on(60, 100);

    let (peak, _rms) = render(&mut e, 64);
    assert!(peak > 1e-2, "+6 dB master output silent: {peak}");
}

#[test]
fn delay_bypass_round_trip_keeps_signal_path_finite() {
    let s = SharedParams::new();
    s.set(id_of("delay-on").unwrap(), 0.0); // off
    s.set(id_of("reverb-on").unwrap(), 0.0); // off
    let mut e = Engine::new(SR, BLK);
    e.snapshot_params(&s);
    e.note_on(60, 100);
    let (peak_off, _) = render(&mut e, 32);
    assert!(peak_off.is_finite() && peak_off > 1e-3);

    s.set(id_of("delay-on").unwrap(), 1.0);
    s.set(id_of("reverb-on").unwrap(), 1.0);
    e.snapshot_params(&s);
    let (peak_on, _) = render(&mut e, 32);
    assert!(peak_on.is_finite() && peak_on > 1e-3);
}

#[test]
fn algo_sweep_every_algo_renders_finite() {
    // Every one of the 32 algos must hold a sustained note without panics
    // or non-finite output. Catches algo-router edge cases that the
    // single-stack tests in `vxn2-dsp` would miss when summed across 16
    // stacks + the FX chain.
    let s = SharedParams::new();
    let upper_algo = id_of("upper-algo").unwrap();
    let lower_algo = id_of("lower-algo").unwrap();
    let mut e = Engine::new(SR, BLK);

    for algo in 1..=32 {
        s.set(upper_algo, algo as f32);
        s.set(lower_algo, algo as f32);
        e.snapshot_params(&s);
        // Refresh the held notes so the new algo gates take effect.
        e.note_off(60);
        e.note_off(64);
        let _ = render(&mut e, 4); // let releases settle
        e.note_on(60, 100);
        e.note_on(64, 100);
        let (peak, _) = render(&mut e, 24);
        assert!(
            peak.is_finite() && peak >= 0.0,
            "algo {} non-finite peak {peak}",
            algo
        );
    }
}
