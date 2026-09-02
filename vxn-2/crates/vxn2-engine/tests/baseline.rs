//! Render-hash baseline. Drives the engine through a fixed, matrix-rich patch
//! and hashes the rendered stereo output bit-for-bit, guarding against a
//! cook-stage reorder or ramp-index regression that flips the hash.
//!
//! The patch lights up every cook stage at once: per-op level / pan / phase
//! ramps, the stack-pitch scatter, the global-pitch smoother, FX-mix
//! aggregation, per-stack feedback, and the deferred routes — across a held
//! chord with spread lanes, driven the way the CLAP shell drives it.
//!
//! The hash folds raw f32 bits, which round differently across targets and OS
//! releases, so `EXPECTED` is enforced on CI only (gated behind
//! `VXN_RENDER_HASH=1`) and dev machines skip. Re-capture after an intentional
//! DSP change by reading the `BASELINE render hash = 0x…` line from a CI log.
//!
//! **The null test** (ticket 0329) is the second half, and it is the bar E049
//! actually judges against: a hash is binary and cannot express a tolerance,
//! while several E049 tickets legitimately reorder float operations — which
//! changes bits without changing what anyone hears. It compares a fresh render
//! of the same patch against the checked-in `reference_render.f32` and requires
//! the difference peak at or below −100 dBFS. It is the **inverse** of the
//! hash's gating — it runs on dev machines and skips where `VXN_RENDER_HASH`
//! marks CI — because each artefact is only valid where it was captured, and
//! the dev machine is where E049's refactors are judged. See
//! [`skip_null_test_here`].
//!
//! The reference file is raw little-endian `f32`, interleaved L/R at 48 kHz,
//! and it is one second long **on purpose**: E049 §"The bar" — a ULP-scale
//! pitch perturbation integrates into phase drift, so a long render lets an
//! inaudible reorder walk past −100 dBFS and fail a ticket that changed nothing
//! audible. Re-capture with `VXN_CAPTURE_REFERENCE=1`, and only ever as a
//! deliberate, named act in a ticket close-out.
//!
//! **Compare like with like: vxn-2's render is profile-sensitive.** The file is
//! captured by the default `cargo test` (debug); the same code under
//! `--release` nulls against it at **−120.4 dBFS** rather than `-inf`. That
//! still clears the −100 dBFS bar, but with 20 dB of headroom rather than 40,
//! so read a −120 dBFS release figure as the floor and not as a regression, and
//! capture and check in one profile. (vxn-1b's render is identical in both
//! profiles; only vxn-2 shows this.)
//!
//! Found while measuring that, and worth confirming next time this hash is
//! re-captured: `EXPECTED` reproduces on a macOS 14 / aarch64 dev box under
//! `--release`, but the debug profile there yields `0x95ac_9a59_d27a_addd` —
//! and CI runs `cargo test --workspace` in debug.

use vxn2_engine::MatrixRowRaw;
use vxn2_engine::engine::Engine;
use vxn_core_dsp::test_util::{assert_null_test, null_test_peak_dbfs};

use std::hash::{Hash, Hasher};
use std::path::PathBuf;

const SR: f32 = 48_000.0;
const BLK: usize = 32;

/// Golden hash of the reference render. Behaviour-preserving refactors must
/// leave it untouched; an intentional DSP change re-captures it (see header).
const EXPECTED: u64 = 0x9b76_78e7_f9d3_534b;

/// E049's bar: the difference peak between two renders of the same patch must
/// sit at or below this, which is beneath the 16-bit noise floor and far
/// beneath audibility, while leaving ample room for last-bit reordering.
const NULL_LIMIT_DBFS: f64 = -100.0;

/// Frames in the checked-in reference render: 1 s of stereo at 48 kHz — short
/// on purpose (see header).
const REF_FRAMES: usize = 48_000;
/// Blocks the chord is held for; the remainder renders the release tail.
const HELD_BLOCKS: usize = (REF_FRAMES * 3 / 4) / BLK;
/// Blocks rendered after the note-offs.
const RELEASE_BLOCKS: usize = REF_FRAMES / BLK - HELD_BLOCKS;

/// The chord both halves play.
const CHORD: [u8; 4] = [48, 55, 60, 64];

/// Build the reference engine: a matrix-rich, deterministic patch.
fn reference_engine() -> Engine {
    let mut e = Engine::new(SR, BLK);

    // FX on so the FX-mix aggregation stage actually feeds the chain.
    e.params.delay.on = true;
    e.params.delay.mix = 0.3;
    e.params.reverb.on = true;
    e.params.reverb.mix = 0.25;

    // Moving sources for the matrix.
    e.params.mod_params.lfo1.rate_hz = 5.0;
    // LFO2 is a per-voice (per-stack) modulator — its params live on the voice.
    e.params.patch.voice.lfo2.rate_hz = 7.3;
    // Spread lanes so per-lane pan / voice-spread paths carry real motion.
    e.params.patch.stack.density = 4;
    e.params.patch.stack.spread = 0.6;

    // Ten routes, one per cook stage of interest. Dest ids are `DestId as u8`
    // (None = 0): Op1Level = 2, Op2Pan = 6, GlobalPitch = 19, Lfo1Rate = 20,
    // Lfo2Phase = 22, StackDetune = 23, DelayMix = 25, Feedback = 27,
    // Op1StackPitch = 30, Op1Phase = 36. Sources (SourceId as u8): Lfo1 = 1,
    // Lfo2 = 2, ModEnv = 4, ModWheel = 5, Velocity = 7.
    let routes: [(u8, u8, f32); 10] = [
        (1, 2, 1.0),   // Lfo1   → Op1Level     (level ramp)
        (1, 6, 0.8),   // Lfo1   → Op2Pan       (pan ramp)
        (2, 36, 0.5),  // Lfo2   → Op1Phase     (phase ramp)
        (4, 30, 0.7),  // ModEnv → Op1StackPitch(stack-pitch scatter)
        (1, 19, 0.4),  // Lfo1   → GlobalPitch  (pitch smoother)
        (7, 25, 0.6),  // Velocity → DelayMix   (FX aggregation)
        (5, 27, 0.5),  // ModWheel → Feedback   (per-stack feedback mod)
        (1, 20, 0.5),  // Lfo1   → Lfo1Rate     (deferred lfo1-rate)
        (4, 23, 0.5),  // ModEnv → StackDetune  (deferred stack macro)
        (2, 22, 0.5),  // Lfo2   → Lfo2Phase    (deferred lfo2-phase)
    ];
    for (s, &(source, dest, depth)) in routes.iter().enumerate() {
        e.params.matrix_rows[s] = MatrixRowRaw {
            source,
            dest,
            curve: 0,
            active: true,
            depth,
            scale_src: 0,
            scale_curve: 0,
        };
        // Slots < N_CLAP_DEPTH_SLOTS (8) read the CLAP depth; later slots read
        // the row depth. Set both so every route's depth lands regardless.
        if s < 8 {
            e.params.mtx_depths[s] = depth;
        }
    }
    e.apply_block_params();
    e
}

/// Render `blocks` control blocks the CLAP way (re-apply params each block),
/// returning interleaved L/R samples.
fn render_interleaved(e: &mut Engine, blocks: usize) -> Vec<f32> {
    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    let mut out = Vec::with_capacity(blocks * BLK * 2);
    for _ in 0..blocks {
        e.apply_block_params();
        e.process_block(&mut l, &mut r);
        for i in 0..BLK {
            out.push(l[i]);
            out.push(r[i]);
        }
    }
    out
}

/// Fold every output sample's bit pattern into the hash. Interleaved order, so
/// the digest is the same sequence of `to_bits()` the pre-0329 loop produced —
/// factoring the render out of it did not re-baseline `EXPECTED`.
fn render_hash(e: &mut Engine, blocks: usize, h: &mut impl Hasher) {
    for s in render_interleaved(e, blocks) {
        s.to_bits().hash(h);
    }
}

/// The reference engine with the chord down and the controllers off their zero,
/// so the wheel / pressure sources contribute rather than multiplying by
/// nothing.
fn playing_engine() -> Engine {
    let mut e = reference_engine();
    e.set_mod_wheel(0.7);
    e.set_aftertouch(0.4);
    for &note in &CHORD {
        e.note_on(note, 100);
    }
    e
}

/// One second of the reference patch: three quarters held, one quarter ringing
/// out after the note-offs so the release path is in the measurement too.
fn reference_render() -> Vec<f32> {
    let mut e = playing_engine();
    let mut out = render_interleaved(&mut e, HELD_BLOCKS);
    for &note in &CHORD {
        e.note_off(note);
    }
    out.extend(render_interleaved(&mut e, RELEASE_BLOCKS));
    out
}

/// Whether the CI-only render hash is being enforced.
///
/// Any value except `0` counts as on. The bare presence test this replaces made
/// `VXN_RENDER_HASH=0` mean *enabled*, which is the opposite of what anyone
/// typing it intends — and it drove two gates in opposite directions at once.
fn render_hash_enforced() -> bool {
    std::env::var("VXN_RENDER_HASH").is_ok_and(|v| v != "0")
}

/// Whether to skip the reference null test in this environment.
///
/// The two baselines are **environment-complementary**. `EXPECTED` is a CI
/// artefact — the header's own rationale is that the hash rounds differently
/// across targets and OS releases — while the reference render is captured on
/// the developer's machine, which is where E049's refactors are judged. Whether
/// a dev-captured render nulls within −100 dBFS on the runner has never been
/// established, and a one-second render integrates ULP-scale pitch differences,
/// so the null test stands down where `VXN_RENDER_HASH` marks CI rather than
/// gambling a red build on it. CI owns the hash; the dev machine owns the null
/// test.
fn skip_null_test_here() -> bool {
    if render_hash_enforced() {
        eprintln!(
            "skipping reference_render_nulls_out: VXN_RENDER_HASH marks the CI environment, \
             where the dev-captured reference render does not apply"
        );
        return true;
    }
    false
}

fn reference_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/reference_render.f32")
}

/// Read the checked-in reference render: raw little-endian `f32`, interleaved.
///
/// Deliberately not shared with vxn-1b's copy of this plumbing: only the
/// *comparator* is shared (`vxn_core_dsp::test_util`), because a reference
/// render needs this synth's engine and this synth's patch.
fn read_reference() -> Vec<f32> {
    let path = reference_path();
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("reference render missing at {}: {e}", path.display()));
    assert_eq!(bytes.len() % 4, 0, "reference render is not a whole number of f32s");
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Overwrite the reference render, then **fail**. Only reachable with
/// `VXN_CAPTURE_REFERENCE` set — re-capturing is a deliberate act that belongs
/// in a ticket close-out, never something a test does because it noticed a
/// mismatch.
///
/// Failing rather than returning `ok` is the point: an exported variable
/// outlives the command that needed it, and a capture run that reports green
/// would silently re-baseline this file on every `cargo test` thereafter while
/// the null test claimed to pass. The other reference checks would not catch
/// it — a regressed render is still one second long, still audible and still
/// finite.
fn capture_reference(samples: &[f32]) -> ! {
    let path = reference_path();
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(&path, &bytes).expect("write reference render");
    panic!(
        "captured {} samples to {} — rerun without VXN_CAPTURE_REFERENCE to check against it",
        samples.len(),
        path.display()
    );
}

#[test]
#[cfg_attr(
    not(all(target_os = "macos", target_arch = "aarch64")),
    ignore = "render hash is captured on macOS/aarch64; f32 rounding differs per target"
)]
fn render_hash_unchanged() {
    // Enforced only where `VXN_RENDER_HASH=1` is set; dev machines skip.
    if !render_hash_enforced() {
        eprintln!("skipping render_hash_unchanged: VXN_RENDER_HASH unset (CI-only)");
        return;
    }

    // A held chord with motion: mod-wheel + aftertouch keep the ModWheel /
    // (and patch-global) sources off their zero, so their routes contribute.
    let mut e = playing_engine();

    let mut h = std::collections::hash_map::DefaultHasher::new();
    // ~0.25 s of audio: long enough for EG attack/decay, LFO travel, and the
    // one-block-latency deferred routes to settle and then move.
    render_hash(&mut e, (SR as usize / 4) / BLK, &mut h);
    // Release and let the tails ring so the OFF-path render loop is exercised.
    for &note in &CHORD {
        e.note_off(note);
    }
    render_hash(&mut e, (SR as usize / 8) / BLK, &mut h);

    let got = h.finish();
    println!("BASELINE render hash = {got:#018x}");
    assert_eq!(
        got, EXPECTED,
        "render hash changed: cook-stage reorder or ramp-index regression \
         (or an intentional DSP change — run the null test, then re-capture \
         EXPECTED; see header)"
    );
}

/// E049's actual bar. Renders the reference patch and compares against the
/// checked-in file; a reordered sum should land near −140 dBFS, well inside the
/// −100 dBFS limit, while a real routing change will not.
#[test]
#[cfg_attr(
    not(all(target_os = "macos", target_arch = "aarch64")),
    ignore = "reference render is captured on macOS/aarch64; libm rounding differs per target"
)]
fn reference_render_nulls_out() {
    if skip_null_test_here() {
        return;
    }
    let got = reference_render();
    if std::env::var_os("VXN_CAPTURE_REFERENCE").is_some() {
        capture_reference(&got);
    }
    let want = read_reference();
    assert_eq!(want.len(), got.len(), "reference render length changed — re-capture it");
    // Report the reading even on success: "−138 dBFS" and "−101 dBFS" both pass
    // and mean very different things about how much headroom a change has left.
    println!("NULL TEST peak = {:.2} dBFS", null_test_peak_dbfs(&want, &got));
    assert_null_test(&want, &got, NULL_LIMIT_DBFS);
}

/// Prove the harness by making it fail. A null test that silently passes on
/// everything is worse than no null test, and that is the failure mode nobody
/// notices — so perturb the reference by a *known* amount and check the
/// reported peak is that amount, at the sample it was applied to.
///
/// Reads the checked-in file rather than rendering, so it exercises the same
/// bytes the real check compares against and runs on every target.
#[test]
fn a_perturbed_reference_is_caught_at_the_right_level_and_sample() {
    let want = read_reference();
    const AT: usize = 12_345;
    const BY: f32 = 1e-3; // −60 dBFS, 40 dB over the limit
    let mut got = want.clone();
    got[AT] += BY;

    // Not exactly −60: adding 1e-3 to a sample of ordinary magnitude rounds in
    // f32, so the recovered difference is within an ULP of the perturbation.
    // A tenth of a dB is far tighter than the decision the limit expresses.
    let peak = null_test_peak_dbfs(&want, &got);
    assert!((peak - -60.0).abs() < 0.1, "a 1e-3 perturbation should read −60 dBFS, got {peak}");

    let err = std::panic::catch_unwind(move || assert_null_test(&want, &got, NULL_LIMIT_DBFS))
        .expect_err("−60 dBFS is 40 dB over the limit and must fail");
    let msg = err.downcast_ref::<String>().cloned().expect("assert! panics with a String");
    assert!(msg.contains("-60.0"), "failure lost the measured peak: {msg}");
    assert!(msg.contains(&format!("sample {AT}")), "failure lost the sample index: {msg}");
}

/// The reference is the right shape and is not silence — a file of zeros would
/// null against a broken engine perfectly.
#[test]
fn the_reference_render_is_one_second_of_real_audio() {
    let want = read_reference();
    assert_eq!(want.len(), REF_FRAMES * 2, "1 s of interleaved stereo at 48 kHz");
    let peak = want.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.05, "reference render is near-silent (peak {peak}) — recapture it");
    assert!(want.iter().all(|s| s.is_finite()), "reference render contains non-finite samples");
}
