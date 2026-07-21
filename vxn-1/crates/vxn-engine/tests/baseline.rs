//! Audio baseline tripwire (E001/0010).
//!
//! Renders a fixed MIDI sequence through a default-patch `Synth` and
//! hashes the output. If a refactor perturbs the audio at any bit, the
//! hash mismatches and the test fails. Bit-identity is a stronger gate
//! than the ticket's 1e-6 RMS but it makes the failure mode obvious:
//! the offending change either preserved bit-identity (test passes) or
//! it didn't (test fails and the developer runs the full RMS diff
//! manually).
//!
//! Sequence: hold C major chord (60/64/67) for 0.25s, release, render
//! 0.25s tail. 0.5s @ 48 kHz = 24 000 stereo samples. Synth uses the
//! all-defaults patch from `Synth::new` so nothing depends on the
//! preset bank format / serde path.
//!
//! To regenerate the golden hash after an intentional audio change:
//!
//! ```ignore
//! UPDATE_GOLDEN=1 cargo test -p vxn-engine --test baseline
//! ```
//!
//! The test prints the new hash; copy it into `GOLDEN_HASH` below.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use vxn_engine::Synth;

const SR: f32 = 48_000.0;
const BLOCK: usize = 64;

/// SHA-style stable hash of the rendered samples. Computed once after
/// E001/0007–0009 lands; subsequent refactors must preserve it (or
/// update intentionally with `UPDATE_GOLDEN=1`).
const GOLDEN_HASH: u64 = 0x4fdcf5e72764bf25;

#[test]
fn baseline_render_is_stable() {
    let mut synth = Synth::new(SR);
    let chord = [60u8, 64, 67];
    for &n in &chord {
        synth.note_on(n, 0.78);
    }
    let mut samples: Vec<f32> = Vec::with_capacity(48_000);
    let mut l = [0.0_f32; BLOCK];
    let mut r = [0.0_f32; BLOCK];
    // 0.25s gated render (12 000 samples = 188 blocks of 64).
    for _ in 0..188 {
        synth.process(&mut l, &mut r);
        for i in 0..BLOCK {
            samples.push(l[i]);
            samples.push(r[i]);
        }
    }
    // Release.
    for &n in &chord {
        synth.note_off(n);
    }
    // 0.25s release tail.
    for _ in 0..188 {
        synth.process(&mut l, &mut r);
        for i in 0..BLOCK {
            samples.push(l[i]);
            samples.push(r[i]);
        }
    }

    let hash = hash_samples(&samples);

    if std::env::var("UPDATE_GOLDEN").is_ok() || GOLDEN_HASH == 0x0 {
        eprintln!("baseline render hash: 0x{hash:016x}");
        eprintln!(
            "to lock in, set `const GOLDEN_HASH: u64 = 0x{hash:016x};` in {}",
            file!()
        );
        if GOLDEN_HASH == 0x0 {
            // Sentinel: not yet baselined. Print and pass.
            return;
        }
    }

    assert_eq!(
        hash, GOLDEN_HASH,
        "baseline render perturbed — got 0x{hash:016x}, expected 0x{GOLDEN_HASH:016x}"
    );
}

fn hash_samples(samples: &[f32]) -> u64 {
    let mut h = DefaultHasher::new();
    samples.len().hash(&mut h);
    for &s in samples {
        s.to_bits().hash(&mut h);
    }
    h.finish()
}
