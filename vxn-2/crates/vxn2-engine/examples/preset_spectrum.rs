//! Scratch probe: harmonic spectrum of a factory preset's held note.
//!
//! Renders one note with the stack collapsed to a single lane and the time FX
//! off, then Goertzel-evaluates each harmonic of the played fundamental over an
//! attack window and a sustain window. Prints partial levels in dB relative to
//! the fundamental plus a spectral centroid (in harmonic number), which is the
//! single figure that says "brass" or "sine".
//!
//! Usage: cargo run --release -p vxn2-engine --example preset_spectrum -- [Category/Name ...]

use vxn2_engine::engine::Engine;
use vxn2_engine::params::id_of;
use vxn2_engine::preset::from_toml_str;
use vxn2_engine::shared::{ParamModel, SharedParams};

const SR: f32 = 48_000.0;
const BLOCK: usize = vxn_core_dsp::control::CONTROL_BLOCK;
const NOTE: u8 = 48; // C3, 130.81 Hz — room for 24 harmonics under Nyquist
const VEL: u8 = 100;
const N_HARM: usize = 24;

fn main() {
    let filters: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("presets/factory");
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for cat in std::fs::read_dir(&dir).unwrap().flatten() {
        if !cat.path().is_dir() {
            continue;
        }
        for f in std::fs::read_dir(cat.path()).unwrap().flatten() {
            if f.path().extension().and_then(|e| e.to_str()) == Some("toml") {
                files.push(f.path());
            }
        }
    }
    files.sort();

    println!("{:<32} {:>10} {:>10}   partials (dB re fundamental)", "preset", "cent.atk", "cent.sus");
    for path in &files {
        let label = format!(
            "{}/{}",
            path.parent().unwrap().file_name().unwrap().to_string_lossy(),
            path.file_stem().unwrap().to_string_lossy()
        );
        if !filters.is_empty() && !filters.iter().any(|f| label.contains(f.as_str())) {
            continue;
        }
        let src = std::fs::read_to_string(path).unwrap();
        let (_m, blob, _w) = from_toml_str(&src).unwrap();
        let (atk, sus) = spectrum(&blob);
        println!(
            "{label:<32} {:>10.2} {:>10.2}   {}",
            centroid(&atk),
            centroid(&sus),
            sus.iter()
                .take(12)
                .map(|&a| {
                    let db = 20.0 * (a / sus[0].max(1e-12)).max(1e-6).log10();
                    format!("{db:>5.0}")
                })
                .collect::<Vec<_>>()
                .join("")
        );
    }
}

/// Harmonic magnitudes over an attack window (10–90 ms) and a sustain window
/// (600–1100 ms).
fn spectrum(blob: &[u8]) -> ([f32; N_HARM], [f32; N_HARM]) {
    let shared = SharedParams::new();
    shared.load_bytes(blob).unwrap();
    // Isolate the voice: one lane (no detune smear), no time FX, no limiter.
    for (id, v) in [
        ("stack-density", 1.0),
        ("reverb-on", 0.0),
        ("delay-on", 0.0),
        ("phaser-on", 0.0),
        ("dyn-on", 0.0),
        ("limiter-on", 0.0),
        ("master-volume", 0.0),
    ] {
        shared.set(id_of(id).unwrap(), v);
    }

    let mut engine = Engine::new(SR, BLOCK);
    engine.snapshot_params(&shared);
    engine.apply_block_params();

    let f0 = 440.0 * 2_f32.powf((NOTE as f32 - 69.0) / 12.0);
    let total = (SR * 2.6) as usize;
    let atk_win = ((SR * 0.010) as usize, (SR * 0.090) as usize);
    let sus_win = ((SR * 0.600) as usize, (SR * 2.600) as usize);

    let mut buf = Vec::with_capacity(total);
    let mut l = vec![0.0f32; BLOCK];
    let mut r = vec![0.0f32; BLOCK];
    engine.note_on(NOTE, VEL);
    let mut n = 0;
    while n < total {
        let k = BLOCK.min(total - n);
        l[..k].fill(0.0);
        r[..k].fill(0.0);
        engine.process_block(&mut l[..k], &mut r[..k]);
        buf.extend_from_slice(&l[..k]);
        n += k;
    }

    (dft(&buf[atk_win.0..atk_win.1], f0), dft(&buf[sus_win.0..sus_win.1], f0))
}

/// Naive DFT evaluated only at `k·f0`, Hann-windowed.
fn dft(x: &[f32], f0: f32) -> [f32; N_HARM] {
    let mut out = [0.0f32; N_HARM];
    let n = x.len() as f32;
    for (h, o) in out.iter_mut().enumerate() {
        let f = f0 * (h + 1) as f32;
        if f >= SR * 0.5 {
            continue;
        }
        let w = std::f32::consts::TAU * f / SR;
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (i, &s) in x.iter().enumerate() {
            let t = i as f32;
            let hann = 0.5 - 0.5 * (std::f32::consts::TAU * t / n).cos();
            let v = s * hann;
            re += v * (w * t).cos();
            im -= v * (w * t).sin();
        }
        *o = (re * re + im * im).sqrt() / n;
    }
    out
}

/// Amplitude-weighted mean harmonic number. 1.0 = pure sine.
fn centroid(a: &[f32; N_HARM]) -> f32 {
    let num: f32 = a.iter().enumerate().map(|(i, &v)| (i + 1) as f32 * v).sum();
    let den: f32 = a.iter().sum();
    if den <= 0.0 { 0.0 } else { num / den }
}
