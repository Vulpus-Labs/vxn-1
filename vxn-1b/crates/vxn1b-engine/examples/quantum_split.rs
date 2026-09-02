//! Control-rate vs audio-rate split, measured on a busy matrix patch driven by
//! a real note stream (ticket-less profiling harness).
//!
//! The engine renders in fixed `CONTROL_BLOCK` (32-sample) quanta: `RenderBank`
//! phases 1–3 are per-quantum control work (LFO tick, matrix eval, smoother
//! targets, filter-coefficient cook, gain resolution), phase 4 is the
//! per-sample vectorised frame loop, and the engine's decimate/FX/master stage
//! is per sample too.
//!
//! `process_block` chunks to `min(len, CONTROL_BLOCK)`, so calling it with a
//! buffer shorter than 32 renders exactly one control block of `n` samples.
//! Rendering the *same* musical content at several `n` and regressing wall time
//! on the block count separates the two:
//!
//!     t(n) = blocks(n) · C  +  samples · S
//!
//! `C` is the fixed per-quantum control cost, `S` the marginal per-sample cost.
//! At the shipping quantum a block costs `C + 32·S`.
//!
//!   cargo run --release --example quantum_split -p vxn1b-engine -- sweep
//!   cargo run --release --example quantum_split -p vxn1b-engine -- run 3000
//!
//! Env knobs: `OS` (0/1/2 = 1×/2×/4× oversample), `FX` (0/1), `DUAL` (0/1),
//! `ROUTES` (number of matrix slots to fill, 0..=16), `VOICES` (held-note cap).

use std::time::Instant;

use vxn1b_engine::matrix::{DestId, MatrixSlot, Polarity, Shape, SourceId};
use vxn1b_engine::params::{global_clap_id, patch_clap_id};
use vxn1b_engine::{Engine, Layer, ParamId};

const SR: f32 = 48_000.0;
/// Audio rendered per timed repetition. Divisible by every sweep width below.
const SAMPLES: usize = 96_000; // 2 s
const REPS: usize = 9;
/// Block widths for the regression. All ≤ CONTROL_BLOCK (32) so each
/// `process_block` call is exactly one control block, and all divide `SAMPLES`.
const WIDTHS: [usize; 7] = [4, 8, 12, 16, 20, 24, 32];

// ── the note stream ─────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Ev {
    On(u8, f32),
    Off(u8),
    Wheel(f32),
    Pressure(f32),
    Bend(f32),
}

/// Deterministic 16th-note stream at 120 BPM: a rolling voicing that keeps
/// `cap` notes down, releasing the oldest as new ones arrive, so the allocator,
/// the release tails and voice stealing all stay live. Mod wheel, aftertouch
/// and pitch bend sweep underneath so the matrix's continuous sources move.
fn sequence(cap: usize) -> Vec<(usize, Ev)> {
    let step = (SR as usize * 60) / (120 * 4); // 16th @ 120 bpm = 6000 samples
    let mut evs = Vec::new();
    let mut held: Vec<u8> = Vec::new();
    let mut rng = 0x243f_6a88_85a3_08d3u64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    // Two octaves of a minor-pentatonic pool plus roots, so pitches differ per
    // lane (no accidental unison collapse of the filter coefficient work).
    let pool: [u8; 15] = [36, 43, 48, 51, 55, 58, 60, 63, 67, 70, 72, 75, 77, 79, 84];

    let steps = SAMPLES / step + 1;
    for s in 0..steps {
        let at = s * step;
        // Two or three fresh notes per 16th.
        let n_new = 2 + (next() % 2) as usize;
        for _ in 0..n_new {
            if held.len() >= cap {
                let old = held.remove(0);
                evs.push((at, Ev::Off(old)));
            }
            let note = pool[(next() % pool.len() as u64) as usize];
            let vel = 0.4 + (next() % 60) as f32 / 100.0;
            evs.push((at, Ev::On(note, vel)));
            held.push(note);
        }
        // Continuous controllers, one update per 16th (as a real host would).
        let ph = s as f32 * 0.13;
        evs.push((at, Ev::Wheel(0.5 + 0.5 * (ph).sin())));
        evs.push((at, Ev::Pressure(0.5 + 0.5 * (ph * 0.7).cos())));
        evs.push((at, Ev::Bend(0.6 * (ph * 0.31).sin())));
    }
    evs
}

// ── patch ───────────────────────────────────────────────────────────────────

/// The routes, in the order they are filled. Deliberately spread across cheap
/// dests (scalars the block-start pass just stores) and expensive ones (the
/// per-quantum smoothed pitch family, the filter cook, the chained `Lfo1Rate`),
/// with two `scale_src` VCAs and two non-linear shapes.
const ROUTES: [(SourceId, DestId, f32, Shape, SourceId); 12] = [
    (SourceId::Env2, DestId::Amp, 1.0, Shape::Lin, SourceId::None),
    (SourceId::Lfo1, DestId::Pitch, 0.35, Shape::Lin, SourceId::ModWheel),
    (SourceId::Spread, DestId::Pan, 1.0, Shape::Lin, SourceId::None),
    (SourceId::Env1, DestId::Cutoff, 0.55, Shape::Exp, SourceId::Velocity),
    (SourceId::Velocity, DestId::Amp, 0.4, Shape::Lin, SourceId::None),
    (SourceId::Lfo2, DestId::Lfo1Rate, 0.6, Shape::Lin, SourceId::None),
    (SourceId::Lfo1, DestId::Pwm, 0.5, Shape::Lin, SourceId::None),
    (SourceId::Key, DestId::Cutoff, 0.3, Shape::Lin, SourceId::None),
    (SourceId::Aftertouch, DestId::CrossModAmount, 0.7, Shape::Exp, SourceId::None),
    (SourceId::Env1, DestId::XModSweep, 0.45, Shape::Lin, SourceId::None),
    (SourceId::NoteRandom, DestId::Osc2Pwm, 0.3, Shape::Lin, SourceId::None),
    (SourceId::Lfo2, DestId::HpfCutoff, 0.35, Shape::Lin, SourceId::None),
];

fn env(key: &str, default: f32) -> f32 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn build(routes: usize, dual: bool, os: f32, fx: f32) -> Engine {
    let mut e = Engine::new(SR);

    for (p, v) in [
        (ParamId::Oversample, os),
        (ParamId::ChorusOn, fx),
        (ParamId::DelayOn, fx),
        (ParamId::ReverbOn, fx),
        (ParamId::LimiterOn, fx),
    ] {
        if let Some(id) = global_clap_id(p) {
            e.set_param(id, v);
        }
    }

    let layers: &[Layer] = if dual { &[Layer::L1, Layer::L2] } else { &[Layer::L1] };
    for &layer in layers {
        let set = |e: &mut Engine, p: ParamId, v: f32| {
            if let Some(id) = patch_clap_id(layer, p) {
                e.set_param(id, v);
            }
        };
        for (p, v) in [
            (ParamId::Osc1Wave, 3.0), // pulse — two polyBLEPs a sample
            (ParamId::Osc1Level, 0.8),
            (ParamId::Osc1PulseWidth, 0.45),
            (ParamId::Osc2Wave, 1.0),
            (ParamId::Osc2Level, 0.8),
            (ParamId::Osc2Coarse, 7.0),
            (ParamId::SubLevel, 0.3),
            (ParamId::NoiseLevel, 0.15),
            (ParamId::CrossModType, 1.0), // hard sync
            (ParamId::CrossModAmount, 0.5),
            (ParamId::Cutoff, 2000.0),
            (ParamId::Resonance, 0.8),
            (ParamId::Drive, 0.4),
            (ParamId::HpfCutoff, 80.0),
            (ParamId::FilterKeyTrack, 0.4),
            (ParamId::Env1Sustain, 0.6),
            (ParamId::Env2Sustain, 0.7),
            (ParamId::Env2Release, 0.6),
            (ParamId::Lfo1Rate, 5.3),
            (ParamId::Lfo2Rate, 0.7),
            (ParamId::Spread, 0.8),
            (ParamId::UnisonDetune, 8.0),
            (ParamId::PortamentoTime, 0.05),
            (ParamId::MasterDrift, 0.5), // per-lane trims + drift walk live
        ] {
            set(&mut e, p, v);
        }

        // Matrix: overwrite the default three and fill up to `routes`.
        {
            let t = e.matrix_mut(layer);
            for s in 0..vxn1b_engine::matrix::N_SLOTS {
                t.slots[s] = MatrixSlot::default();
            }
            for (s, r) in ROUTES.iter().enumerate().take(routes) {
                t.slots[s] = MatrixSlot {
                    source: r.0,
                    dest: r.1,
                    depth: r.2,
                    polarity: Polarity::None,
                    shape: r.3,
                    enabled: true,
                    scale_src: r.4,
                    scale_polarity: Polarity::None,
                    scale_shape: Shape::Lin,
                };
            }
        }
        for (s, r) in ROUTES.iter().enumerate().take(routes) {
            if let Some(p) = ParamId::slot_depth(s) {
                set(&mut e, p, r.2);
            }
        }
        for s in routes..vxn1b_engine::matrix::N_SLOTS {
            if let Some(p) = ParamId::slot_depth(s) {
                set(&mut e, p, 0.0);
            }
        }
    }
    e
}

// ── driver ──────────────────────────────────────────────────────────────────

/// Render `SAMPLES` of the sequence in `n`-sample blocks, delivering each event
/// at the block boundary that contains it. Returns elapsed seconds.
fn render(e: &mut Engine, evs: &[(usize, Ev)], n: usize, timed: bool) -> f64 {
    let mut l = vec![0.0f32; n];
    let mut r = vec![0.0f32; n];
    let mut i = 0;
    let mut acc = 0.0f32;
    let t0 = Instant::now();
    let mut pos = 0;
    while pos < SAMPLES {
        while i < evs.len() && evs[i].0 < pos + n {
            match evs[i].1 {
                Ev::On(note, vel) => {
                    e.note_on(0, note, vel);
                }
                Ev::Off(note) => e.note_off(0, note),
                Ev::Wheel(v) => e.set_mod_wheel(v),
                Ev::Pressure(v) => e.channel_pressure(0, v),
                Ev::Bend(v) => e.set_pitch_bend(v),
            }
            i += 1;
        }
        e.process_block(&mut l, &mut r);
        acc += l[0];
        pos += n;
    }
    let dt = t0.elapsed().as_secs_f64();
    std::hint::black_box(acc);
    let _ = timed;
    dt
}

/// Ordinary least squares of `t` on `blocks` — slope is the per-quantum fixed
/// cost, intercept the total per-sample cost over `SAMPLES`.
fn fit(pts: &[(f64, f64)]) -> (f64, f64, f64) {
    let n = pts.len() as f64;
    let (sx, sy) = pts.iter().fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
    let (mx, my) = (sx / n, sy / n);
    let sxx: f64 = pts.iter().map(|(x, _)| (x - mx) * (x - mx)).sum();
    let sxy: f64 = pts.iter().map(|(x, y)| (x - mx) * (y - my)).sum();
    let slope = sxy / sxx;
    let inter = my - slope * mx;
    let sst: f64 = pts.iter().map(|(_, y)| (y - my) * (y - my)).sum();
    let sse: f64 = pts.iter().map(|(x, y)| (y - (slope * x + inter)).powi(2)).sum();
    (slope, inter, 1.0 - sse / sst)
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "sweep".into());
    let os = env("OS", 0.0);
    let fx = env("FX", 1.0);
    let dual = env("DUAL", 0.0) != 0.0;
    let routes = env("ROUTES", 12.0) as usize;
    let cap = env("VOICES", 16.0) as usize;
    let os_factor = [1usize, 2, 4][os as usize];

    let evs = sequence(cap);
    let n_on = evs.iter().filter(|(_, e)| matches!(e, Ev::On(..))).count();

    if mode == "run" {
        let iters: usize = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(20);
        let mut e = build(routes, dual, os, fx);
        render(&mut e, &evs, 32, false);
        let t0 = Instant::now();
        for _ in 0..iters {
            render(&mut e, &evs, 32, true);
        }
        let audio = iters as f64 * SAMPLES as f64 / SR as f64;
        let dt = t0.elapsed().as_secs_f64();
        println!("run: {dt:.3} s for {audio:.1} s audio ({:.1}x RT)", audio / dt);
        return;
    }

    println!(
        "patch: {routes} routes, {} layer(s), {os_factor}x OS, FX {}, {cap}-note cap, \
         {n_on} note-ons over {:.1} s",
        if dual { 2 } else { 1 },
        if fx != 0.0 { "on" } else { "off" },
        SAMPLES as f64 / SR as f64,
    );
    println!("\n{:>6}  {:>10}  {:>12}  {:>12}", "block", "blocks", "min s", "ns/block");

    // Interleave the widths across rounds and keep the per-width minimum, so a
    // thermal drift over the run biases every width alike instead of loading
    // the regression's slope.
    let mut best = [f64::MAX; WIDTHS.len()];
    for _ in 0..REPS {
        for (k, &n) in WIDTHS.iter().enumerate() {
            let mut e = build(routes, dual, os, fx);
            render(&mut e, &evs, n, false); // warm to steady state
            best[k] = best[k].min(render(&mut e, &evs, n, true));
        }
    }
    let mut pts = Vec::new();
    for (k, &n) in WIDTHS.iter().enumerate() {
        let blocks = SAMPLES / n;
        println!(
            "{n:>6}  {blocks:>10}  {:>12.4}  {:>12.1}",
            best[k],
            best[k] / blocks as f64 * 1e9
        );
        pts.push((blocks as f64, best[k]));
    }

    let (c, inter, r2) = fit(&pts);
    let s = inter / SAMPLES as f64;
    let quantum = c + 32.0 * s;
    println!("\nfit (R2 = {r2:.5}):");
    println!("  control-rate, fixed per quantum : {:>9.1} ns", c * 1e9);
    println!("  audio-rate, per base sample     : {:>9.2} ns  ({:.1} ns / 32-sample quantum)", s * 1e9, 32.0 * s * 1e9);
    println!("  total per 32-sample quantum     : {:>9.1} ns", quantum * 1e9);
    println!(
        "  split                           : {:>8.1}% control  /  {:.1}% audio",
        100.0 * c / quantum,
        100.0 * 32.0 * s / quantum
    );
    let rt = 32.0 / SR as f64;
    println!("  realtime budget for one quantum : {:>9.1} ns  ({:.2}% of one core used)", rt * 1e9, 100.0 * quantum / rt);
}
