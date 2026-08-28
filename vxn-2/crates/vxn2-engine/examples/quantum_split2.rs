//! Control-rate vs audio-rate split for vxn-2, the counterpart to vxn-1b's
//! `quantum_split`.
//!
//! Same instrument: render identical musical content at several block sizes and
//! regress wall time on the block count, `t(n) = blocks·C + samples·S`.
//!
//! The structural difference the numbers turn on is that **vxn-2 has no
//! internal control block**. vxn-1b chunks every host buffer to
//! `CONTROL_BLOCK = 32` and re-cooks per lane inside each chunk; vxn-2 treats
//! the host buffer itself as the control block. So `C` is paid once per host
//! block here, not once per 32 samples, and the control share depends entirely
//! on what block size the host asks for.
//!
//!   cargo run --release --example quantum_split2 -p vxn2-engine
//!
//! Env knobs: `FILTER` (0/1, default 1 — the OTA path is off by default in the
//! patch), `OS` (filter oversample 1/2/4/8), `VOICES` (held-note cap).

use std::time::Instant;

use vxn2_engine::engine::Engine;

const SR: f32 = 48_000.0;
const SAMPLES: usize = 96_000; // 2 s
const REPS: usize = 9;
/// All divide `SAMPLES`. No `CONTROL_BLOCK` cap to stay under, so the sweep
/// runs over realistic host buffer sizes.
const WIDTHS: [usize; 8] = [32, 64, 96, 128, 192, 240, 320, 480];
const MAX_BLOCK: usize = 480;

#[derive(Clone, Copy)]
enum Ev {
    On(u8, u8),
    Off(u8),
    Wheel(f32),
}

/// 16th notes at 120 BPM: a rolling voicing that keeps `cap` notes down and
/// releases the oldest, so allocation, release tails and stealing stay live.
fn sequence(cap: usize) -> Vec<(usize, Ev)> {
    let step = (SR as usize * 60) / (120 * 4);
    let mut evs = Vec::new();
    let mut held: Vec<u8> = Vec::new();
    let mut rng = 0x243f_6a88_85a3_08d3u64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let pool: [u8; 15] = [36, 43, 48, 51, 55, 58, 60, 63, 67, 70, 72, 75, 77, 79, 84];
    for s in 0..(SAMPLES / step + 1) {
        let at = s * step;
        for _ in 0..(2 + (next() % 2) as usize) {
            if held.len() >= cap {
                evs.push((at, Ev::Off(held.remove(0))));
            }
            let note = pool[(next() % pool.len() as u64) as usize];
            evs.push((at, Ev::On(note, 40 + (next() % 80) as u8)));
            held.push(note);
        }
        evs.push((at, Ev::Wheel(0.5 + 0.5 * (s as f32 * 0.13).sin())));
    }
    evs
}

fn env(key: &str, default: f32) -> f32 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn build(filter: bool, os: usize) -> Engine {
    let mut e = Engine::new(SR, MAX_BLOCK);
    {
        let p = e.params_mut();
        // The OTA path is opt-in; this harness exists to measure it, so a run
        // with FILTER=0 is the honest "does the rest of the block cost this
        // anyway" control.
        p.filter.enable = filter;
        p.filter.oversample = os;
        p.filter.cutoff_hz = 2_400.0;
        p.filter.resonance = 0.7;
        p.filter.drive = 1.6;
        // Key-tracking makes every stack's cutoff differ, so the per-stack cook
        // is real work rather than 16 copies of one answer.
        p.filter.keytrack = 0.6;
    }
    e
}

fn render(e: &mut Engine, evs: &[(usize, Ev)], n: usize) -> f64 {
    let mut l = vec![0.0f32; n];
    let mut r = vec![0.0f32; n];
    let (mut i, mut pos) = (0, 0);
    let mut acc = 0.0f32;
    let t0 = Instant::now();
    while pos < SAMPLES {
        while i < evs.len() && evs[i].0 < pos + n {
            match evs[i].1 {
                Ev::On(note, vel) => e.note_on(note, vel),
                Ev::Off(note) => e.note_off(note),
                Ev::Wheel(v) => e.set_mod_wheel(v),
            }
            i += 1;
        }
        e.process_block(&mut l, &mut r);
        acc += l[0];
        pos += n;
    }
    let dt = t0.elapsed().as_secs_f64();
    std::hint::black_box(acc);
    dt
}

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
    let filter = env("FILTER", 1.0) != 0.0;
    let os = env("OS", 4.0) as usize;
    let cap = env("VOICES", 16.0) as usize;
    let evs = sequence(cap);

    println!(
        "vxn-2: filter {}, {os}x OS, {cap}-note cap, {:.1} s audio",
        if filter { "on" } else { "off" },
        SAMPLES as f64 / SR as f64
    );
    println!("\n{:>6}  {:>8}  {:>10}  {:>12}", "block", "blocks", "min s", "ns/block");

    let mut best = [f64::MAX; WIDTHS.len()];
    for _ in 0..REPS {
        for (k, &n) in WIDTHS.iter().enumerate() {
            let mut e = build(filter, os);
            render(&mut e, &evs, n); // warm
            best[k] = best[k].min(render(&mut e, &evs, n));
        }
    }
    let mut pts = Vec::new();
    for (k, &n) in WIDTHS.iter().enumerate() {
        let blocks = SAMPLES / n;
        println!("{n:>6}  {blocks:>8}  {:>10.4}  {:>12.1}", best[k], best[k] / blocks as f64 * 1e9);
        pts.push((blocks as f64, best[k]));
    }

    let (c, inter, r2) = fit(&pts);
    let s = inter / SAMPLES as f64;
    println!("\nfit (R2 = {r2:.5}):");
    println!("  control-rate, fixed per block : {:>10.1} ns", c * 1e9);
    println!("  audio-rate, per sample        : {:>10.2} ns", s * 1e9);
    for bs in [64usize, 128, 256, 512] {
        let tot = c + bs as f64 * s;
        println!(
            "  at a {bs:>3}-sample host block  : {:>8.1}% control / {:.1}% audio   ({:.2}% of one core)",
            100.0 * c / tot,
            100.0 * bs as f64 * s / tot,
            100.0 * tot / (bs as f64 / SR as f64),
        );
    }
}
