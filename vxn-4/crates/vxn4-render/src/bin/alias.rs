//! Does 16x oversampling buy anything over 8x? Measure, rather than squint.
//!
//! ```text
//! cargo run --release -p vxn4-render --bin alias
//! ```
//!
//! Renders the same held note through the same patch at both qualities, then
//! reports how far apart they actually are.
//!
//! ## Why the difference signal is the right metric
//!
//! Everything about the two renders is identical except the operator-block
//! rate: same patch, same envelopes, same decimator below 4x, same limiter. So
//! subtracting one from the other cancels the music and leaves only what the
//! oversampling changed.
//!
//! And in-band, that residue is essentially all aliasing. At 16x the mip
//! selector keeps roughly twice as many harmonics (there is twice the headroom
//! before Nyquist), but those extra harmonics live above 100 kHz and the
//! decimator removes them at both rates. The only way the operator rate can
//! change anything below 24 kHz is by changing what folds down into it.
//!
//! Reported as dB relative to the signal, which is the number that answers the
//! question: -70 dB is inaudible and 16x is a waste; -30 dB is audible fizz and
//! 16x is buying something real.

#[path = "../fft.rs"]
mod fft;

use vxn4_engine::{Engine, Quality, N_PATCHES, patch_names};

const SR: f32 = 48_000.0;
const WINDOW: usize = 32_768;
/// Skipped before measuring, so the decimator has converged and the attack
/// transient is past.
const SETTLE: usize = 8_192;

fn render(patch: usize, note: u8, q: Quality) -> Vec<f32> {
    let mut e = Engine::new(SR);
    e.set_patch(patch);
    e.set_quality(q);
    e.note_on(note, 100);
    let n = SETTLE + WINDOW;
    let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
    e.process(&mut l, &mut r);
    l.drain(..SETTLE);
    l
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

fn db(x: f32) -> f32 {
    if x <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * x.log10()
    }
}

/// Best integer alignment of `b` against `a`, searched over a few samples.
///
/// The two chains differ by one sample of group delay (14 vs 15), but that is
/// derived rather than measured, so the shift is found rather than assumed —
/// a one-sample misalignment would masquerade as a large high-frequency
/// difference and completely invert the conclusion.
fn best_shift(a: &[f32], b: &[f32]) -> (isize, f32) {
    let span = 8isize;
    let n = a.len() - 2 * span as usize;
    let mut best = (0isize, f32::MAX);
    for s in -span..=span {
        let mut acc = 0.0f32;
        for i in 0..n {
            let ai = (i as isize + span) as usize;
            let bi = (i as isize + span + s) as usize;
            let d = a[ai] - b[bi];
            acc += d * d;
        }
        let e = (acc / n as f32).sqrt();
        if e < best.1 {
            best = (s, e);
        }
    }
    best
}

/// Energy in a frequency band, as a fraction of total.
fn band_fraction(spec: &[f32], lo_hz: f32, hi_hz: f32) -> f32 {
    let bins = spec.len() as f32;
    let nyq = SR / 2.0;
    let lo = ((lo_hz / nyq) * bins) as usize;
    let hi = (((hi_hz / nyq) * bins) as usize).min(spec.len());
    let total: f32 = spec.iter().map(|v| v * v).sum();
    if total <= 0.0 {
        return 0.0;
    }
    let band: f32 = spec[lo.min(hi)..hi].iter().map(|v| v * v).sum();
    band / total
}

fn main() {
    println!("vxn-4 — 8x vs 16x, measured\n");
    println!("Held note, {WINDOW} samples of steady state, difference after alignment.");
    println!("`diff` is the 8x-vs-16x residue relative to the signal: more negative = more alike.\n");

    // Up to MIDI 108 (C8, 4186 Hz). Above ~note 96 the fundamental is high
    // enough that fold-down lands in the middle of the audible range rather
    // than above it, which is where aliasing stops being an HF detail.
    let notes: [u8; 6] = [48, 72, 84, 96, 102, 108];

    // Where the residue lives matters as much as how big it is. Energy that
    // lands on top of a strong partial is masked; energy in a quiet part of the
    // spectrum is not, at the same level.
    println!(
        "{:>8} {:>5} {:>6} {:>8} {:>9} {:>26}",
        "patch", "note", "Hz", "diff dB", "HF 12-24k", "diff spectrum 0-2/2-6/6-12/12-24k"
    );
    println!("{}", "-".repeat(84));

    let mut worst: (f32, String) = (f32::NEG_INFINITY, String::new());

    for p in 0..N_PATCHES {
        for note in notes {
            let a = render(p, note, Quality::X8);
            let b = render(p, note, Quality::X16);

            let (shift, diff) = best_shift(&a, &b);
            let sig = rms(&a);
            let rel = db(diff / sig.max(1e-12));

            // High-frequency content at 8x, where fold-down lands audibly.
            let spec = fft::spectrum(&a);
            let hf = band_fraction(&spec, 12_000.0, 24_000.0);

            // Difference signal, aligned, then its own spectrum.
            let span = 8usize;
            let n = a.len() - 2 * span;
            let d: Vec<f32> = (0..n)
                .map(|i| a[i + span] - b[(i as isize + span as isize + shift) as usize])
                .collect();
            let dspec = fft::spectrum(&d);
            let bands = [
                band_fraction(&dspec, 0.0, 2_000.0),
                band_fraction(&dspec, 2_000.0, 6_000.0),
                band_fraction(&dspec, 6_000.0, 12_000.0),
                band_fraction(&dspec, 12_000.0, 24_000.0),
            ];

            let hz = 440.0 * ((note as f32 - 69.0) / 12.0).exp2();
            println!(
                "{:>8} {:>5} {:>6.0} {:>8.1} {:>8.3}% {:>7.0}%{:>6.0}%{:>6.0}%{:>6.0}%",
                patch_names()[p],
                note,
                hz,
                rel,
                hf * 100.0,
                bands[0] * 100.0,
                bands[1] * 100.0,
                bands[2] * 100.0,
                bands[3] * 100.0,
            );

            if rel > worst.0 {
                worst = (rel, format!("{} note {note}", patch_names()[p]));
            }
        }
        println!();
    }

    println!("Worst-case divergence: {:.1} dB ({})", worst.0, worst.1);
    println!(
        "\nReference points: -60 dB is at the edge of audibility on a sustained\n\
         tone; -40 dB is plainly audible as fizz; -20 dB is a different sound."
    );
}
