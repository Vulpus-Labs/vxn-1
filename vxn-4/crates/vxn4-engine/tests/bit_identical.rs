//! The safety net for the ownership inversion (0382).
//!
//! Moving the patch off the audio thread changes **who owns the numbers** and
//! nothing about what is computed with them, so every factory patch has to come
//! out of the renderer bit for bit as it did before. That is a stronger claim
//! than "still sounds right" and it is the only one worth making here: a
//! rendering difference introduced by this ticket is a bug in the transport, not
//! an improvement to the synth, and a tolerance would hide exactly the class of
//! mistake the port is exposed to (a field applied in the wrong order, a
//! derived table rebuilt one control tick late, a clamp applied twice).
//!
//! The digests below were captured from the build **immediately before** the
//! inversion landed, at commit `b663dfd` (0381), and are checked in as
//! constants rather than recomputed by the test. A self-comparing test would
//! pass through any change that was merely self-consistent, which is most of
//! them.
//!
//! An FNV-1a over the raw `f32` bits, not a peak or an energy: a hash sees a
//! single sample out of place in a 100 000-sample render, and none of the
//! summary statistics the engine's own tests use can. When this fails there is
//! no useful information in the number, which is deliberate — the test's job is
//! to say *whether*, and the debugging is done by diffing renders.
//!
//! Lives in `tests/` rather than beside the engine so it drives the crate
//! through its public surface only. If this file needs a private field to say
//! what it wants to say, the inversion has leaked.

use vxn4_engine::{Engine, N_MACROS, N_PATCHES, Quality};

const SR: f32 = 48_000.0;

/// FNV-1a over the bit patterns of a sample slice.
fn fold(mut h: u64, x: &[f32]) -> u64 {
    for s in x {
        for b in s.to_bits().to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}

/// A deterministic exercise of one patch: a five-note chord under a fixed but
/// non-trivial macro pose, held, then released and allowed to ring out.
///
/// The macro pose matters. Every macro at zero renders the patch as authored
/// and would leave the whole modulation path — the matrix evaluation, the
/// damping, detune, pan and spread branches, and the `mod_dirty` bookkeeping —
/// untested by this file, which is precisely the machinery the inversion moves.
/// The positions are a fixed function of patch and knob so the pose is
/// reproducible and differs between patches.
fn render(patch: usize, quality: Quality) -> u64 {
    let mut e = Engine::new(SR);
    e.set_patch(patch);
    e.set_quality(quality);
    for m in 0..N_MACROS {
        e.set_macro(m, ((m * 7 + patch * 3) % 11) as f32 / 10.0);
    }

    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let (mut l, mut r) = (vec![0.0f32; 1024], vec![0.0f32; 1024]);

    // A block before the first note, so the control-rate countdown is mid-cycle
    // when the notes land rather than aligned to a block boundary.
    e.process(&mut l, &mut r);
    h = fold(fold(h, &l), &r);

    for n in [48u8, 55, 60, 63, 67] {
        e.note_on(n, 100);
    }
    for _ in 0..8 {
        e.process(&mut l, &mut r);
        h = fold(fold(h, &l), &r);
    }

    // A macro move under a held note: the control tick has to pick it up.
    e.set_macro(0, 0.83);
    for _ in 0..4 {
        e.process(&mut l, &mut r);
        h = fold(fold(h, &l), &r);
    }

    // A note started after the knob moved — the `live_ratios` path.
    e.note_on(72, 120);
    for _ in 0..4 {
        e.process(&mut l, &mut r);
        h = fold(fold(h, &l), &r);
    }

    for n in [48u8, 55, 60, 63, 67, 72] {
        e.note_off(n);
    }
    for _ in 0..16 {
        e.process(&mut l, &mut r);
        h = fold(fold(h, &l), &r);
    }
    h
}

/// `[8x, 16x]` per patch, in [`vxn4_engine::patch`] order.
const EXPECTED: [[u64; 2]; N_PATCHES] = [
    [0x1c44_966f_246d_07b5, 0xdf55_1a17_60cb_8ee5],
    [0xeeab_0cc4_8427_f983, 0x45f6_f79a_077b_f910],
    [0x2d9a_9469_a308_9049, 0x9c22_39a9_bcf3_7753],
    [0x422f_bd2c_8e79_4c23, 0xb09f_a9a4_7760_b633],
    [0xa467_59c4_e498_6571, 0x0c49_f8d2_dd43_0d4b],
    [0xbdd7_9447_6dff_e967, 0xbac7_9d4b_9051_2b39],
    [0x2fac_d29b_3cb6_9691, 0xc796_5b26_599c_353a],
];

#[test]
fn every_factory_patch_renders_bit_identically_to_the_pre_inversion_build() {
    let mut actual = [[0u64; 2]; N_PATCHES];
    for (p, row) in actual.iter_mut().enumerate() {
        row[0] = render(p, Quality::X8);
        row[1] = render(p, Quality::X16);
    }
    if actual != EXPECTED {
        for row in &actual {
            println!("    [{:#018x}, {:#018x}],", row[0], row[1]);
        }
    }
    assert_eq!(
        actual, EXPECTED,
        "a factory patch moved; see the printed table above"
    );
}

/// The digests must actually be functions of the audio, or the test above is a
/// tautology that would survive the renderer being replaced by silence.
#[test]
fn the_digests_distinguish_the_patches_from_each_other() {
    let mut seen: Vec<u64> = (0..N_PATCHES)
        .flat_map(|p| [render(p, Quality::X8), render(p, Quality::X16)])
        .collect();
    let total = seen.len();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), total, "two renders hashed the same");
}
