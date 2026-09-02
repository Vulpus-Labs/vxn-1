//! Render baselines for vxn-1b: a **hash** tripwire and a **null test**.
//!
//! Both drive one fixed, matrix-rich patch — every source wired, all three
//! polarities and all three shapes represented, two routes summing into the
//! same destination, one route gated by a scale VCA, the one-block-lagged
//! `Lfo1Rate` dest and the note-on-latched `Env1Scale` dest — through a held
//! chord and its release, the way the CLAP shell drives it.
//!
//! **The hash** folds raw f32 bits, which round differently across targets and
//! OS releases, so `EXPECTED` is enforced on CI only (gated behind
//! `VXN_RENDER_HASH=1`) and dev machines skip. Re-capture after an intentional
//! DSP change by reading the `BASELINE render hash = 0x…` line from a CI log.
//! Mirrors `vxn-2/crates/vxn2-engine/tests/baseline.rs`, which vxn-1b had no
//! equivalent of until ticket 0329.
//!
//! **The null test** is the bar E049 actually judges against, because a hash is
//! binary and cannot express a tolerance: several tickets there legitimately
//! reorder float operations, which changes bits without changing what anyone
//! hears. It compares a fresh render against the checked-in
//! `reference_render.f32` and requires the difference peak at or below
//! −100 dBFS. It is the **inverse** of the hash's gating — it runs on dev
//! machines and skips where `VXN_RENDER_HASH` marks CI — because each artefact
//! is only valid where it was captured, and the dev machine is where E049's
//! refactors are judged. See [`skip_null_test_here`].
//!
//! The reference file is raw little-endian `f32`, interleaved L/R at 48 kHz,
//! and it is one second long **on purpose**: E049 §"The bar" — a ULP-scale
//! pitch perturbation integrates into phase drift, so a long render lets an
//! inaudible reorder walk past −100 dBFS and fail a ticket that changed
//! nothing audible. Re-capture with `VXN_CAPTURE_REFERENCE=1`, and only ever as
//! a deliberate, named act in a ticket close-out.
//!
//! vxn-1b's render is **identical in debug and release** — hash and null test
//! both — so unlike vxn-2's baseline this one carries no profile caveat. That
//! is measured, not assumed, and worth re-checking if the reference is ever
//! captured under a different profile than it is compared in.
//!
//! The file plumbing below is deliberately *not* shared with vxn-2's copy of
//! it: only the comparator is shared (`vxn_core_dsp::test_util`), because the
//! reference render needs this synth's engine and this synth's patch. Twenty
//! lines of `fs::read` is the cheaper half of that trade.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use vxn1b_engine::matrix::{DestId, MatrixSlot, MatrixTable, Polarity, Shape, SourceId};
use vxn1b_engine::params::{global_clap_id, patch_clap_id};
use vxn1b_engine::{Engine, Layer, ParamId};
use vxn_core_dsp::test_util::{assert_null_test, null_test_peak_dbfs};

const SR: f32 = 48_000.0;
/// Render chunk handed to `process_block`. The engine splits host blocks into
/// its own `CONTROL_BLOCK` internally, so this only has to be a fixed number.
const BLK: usize = 32;

/// Golden hash of the reference render. Behaviour-preserving refactors must
/// leave it untouched; an intentional DSP change re-captures it (see header).
///
/// **Captured on dev hardware** (macOS 14 / aarch64, debug — the profile CI
/// runs) by ticket 0329, not from a CI log: the ticket that introduced this
/// test had no CI run to read one from. The build profile is ruled out as an
/// axis (this render hashes the same in release), so the one untested
/// difference against the `macos-15` runner is the OS release — which the
/// header's own rationale says can move it. If the **first CI run after this
/// lands fails here**, that is why: take the printed
/// `BASELINE render hash = 0x…` from that log and re-capture, exactly as the
/// header prescribes. After that one correction the tripwire is live.
///
/// **Re-captured by ticket 0231** (E041), which moved the delay onto the shared
/// kernel — a real DSP change, and the delay is in this patch. The previous
/// value, `0xef1c_866f_d4a3_8540`, still verified on this machine at the commit
/// before the migration, so the move is the whole of the difference.
const EXPECTED: u64 = 0x5d7f_71bf_c17f_b2f2;

/// E049's bar: the difference peak between two renders of the same patch must
/// sit at or below this, which is beneath the 16-bit noise floor and far
/// beneath audibility, while leaving ample room for last-bit reordering.
const NULL_LIMIT_DBFS: f64 = -100.0;

/// Frames in the checked-in reference render: 1 s of stereo at 48 kHz — short
/// on purpose (see header).
const REF_FRAMES: usize = 48_000;
/// Blocks the note is held for; the remainder renders the release tail.
const HELD_BLOCKS: usize = (REF_FRAMES * 3 / 4) / BLK;
/// Blocks rendered after the note-offs.
const RELEASE_BLOCKS: usize = REF_FRAMES / BLK - HELD_BLOCKS;

/// The chord, with distinct velocities so the `Velocity` and `Key` sources
/// carry real per-voice variation rather than one repeated value.
const CHORD: [(u8, f32); 4] = [(36, 0.62), (48, 0.85), (60, 1.0), (67, 0.44)];

/// The matrix-rich reference patch, as `(source, dest, depth, polarity, shape,
/// scale source, scale shape)`.
///
/// All 16 slots are filled and all 12 sources appear, so no source's fan-out is
/// silently untested. `Env2 → Amp` and `Velocity → Amp` sum into one dest (the
/// additive-accumulate path); `Lfo1 → Pitch` and `PitchWheel → Pitch` do the
/// same into the cubic-tapered one. `Lfo1 → Pitch` carries the mod-wheel VCA,
/// so the scale path runs too.
#[rustfmt::skip]
const ROUTES: [(SourceId, DestId, f32, Polarity, Shape, SourceId, Shape); 16] = [
    (SourceId::Env2,       DestId::Amp,            1.0,  Polarity::Direct,  Shape::Lin, SourceId::None,     Shape::Lin),
    (SourceId::Env1,       DestId::Cutoff,         0.6,  Polarity::Direct,  Shape::Exp, SourceId::None,     Shape::Lin),
    (SourceId::Spread,     DestId::Pan,            1.0,  Polarity::Direct,  Shape::Lin, SourceId::None,     Shape::Lin),
    (SourceId::Lfo1,       DestId::Pitch,          0.55, Polarity::Direct,  Shape::Lin, SourceId::ModWheel, Shape::Exp),
    (SourceId::Lfo2,       DestId::Pwm,            0.5,  Polarity::Direct,  Shape::Lin, SourceId::None,     Shape::Lin),
    (SourceId::Velocity,   DestId::Amp,            0.4,  Polarity::Direct,  Shape::Exp, SourceId::None,     Shape::Lin),
    (SourceId::Key,        DestId::Cutoff,         0.5,  Polarity::Direct,  Shape::Lin, SourceId::None,     Shape::Lin),
    (SourceId::Aftertouch, DestId::CrossModAmount, 0.7,  Polarity::Direct,  Shape::Lin, SourceId::None,     Shape::Lin),
    (SourceId::NoteRandom, DestId::Osc1Pwm,        0.3,  Polarity::Bipolar, Shape::Lin, SourceId::None,     Shape::Lin),
    (SourceId::StackPos,   DestId::Osc2Pwm,        0.4,  Polarity::Direct,  Shape::Lin, SourceId::None,     Shape::Lin),
    (SourceId::PitchWheel, DestId::Pitch,          0.35, Polarity::Direct,  Shape::Lin, SourceId::None,     Shape::Lin),
    (SourceId::Lfo2,       DestId::Lfo1Rate,       0.5,  Polarity::Direct,  Shape::Lin, SourceId::None,     Shape::Lin),
    (SourceId::Env2,       DestId::Resonance,      0.3,  Polarity::Abs,     Shape::Lin, SourceId::None,     Shape::Lin),
    (SourceId::ModWheel,   DestId::HpfCutoff,      0.4,  Polarity::Direct,  Shape::Log, SourceId::None,     Shape::Lin),
    (SourceId::Velocity,   DestId::Env1Scale,      0.5,  Polarity::Bipolar, Shape::Lin, SourceId::None,     Shape::Lin),
    (SourceId::Env1,       DestId::XModSweep,      0.6,  Polarity::Direct,  Shape::Lin, SourceId::None,     Shape::Lin),
];

/// Build the reference engine: the patch above, with a panel that keeps every
/// modulated destination somewhere it can actually move.
fn reference_engine() -> Engine {
    let mut e = Engine::new(SR);

    // Globals: the whole FX chain on, so the post-synth path is in the render
    // rather than bypassed. Oversampling stays off — it is a separate axis, and
    // pinning it here would make this baseline a test of the decimator too.
    for (p, v) in [
        (ParamId::ChorusOn, 1.0),
        (ParamId::ChorusMix, 0.35),
        (ParamId::DelayOn, 1.0),
        (ParamId::DelayMix, 0.25),
        (ParamId::ReverbOn, 1.0),
        (ParamId::ReverbMix, 0.2),
        (ParamId::Oversample, 0.0),
    ] {
        let id = global_clap_id(p).expect("global param");
        e.set_param(id, v);
    }

    // Panel (layer 1). Pulse osc 1 an octave up hard-syncing osc 2, so the PWM
    // and cross-mod destinations reach real DSP; a 4-lane stack with spread so
    // `Spread` / `StackPos` are non-zero and the bank runs several lanes a note.
    for (p, v) in [
        (ParamId::Osc1Wave, 3.0),   // Pulse
        (ParamId::Osc1Octave, 1.0),
        (ParamId::Osc1Level, 0.8),
        (ParamId::Osc1PulseWidth, 0.5),
        (ParamId::Osc2Level, 0.7),
        (ParamId::Osc2Coarse, 7.0),
        (ParamId::SubLevel, 0.25),
        (ParamId::NoiseLevel, 0.08),
        (ParamId::CrossModType, 1.0), // Sync
        (ParamId::CrossModAmount, 0.4),
        (ParamId::Resonance, 0.6),
        (ParamId::Env1Sustain, 0.7),
        (ParamId::Env2Sustain, 0.8),
        (ParamId::Lfo1Rate, 5.0),
        (ParamId::Lfo2Rate, 3.0),
        (ParamId::StackWidth, 2.0), // 4 lanes per note
        (ParamId::Spread, 0.6),
        (ParamId::UnisonDetune, 0.3),
    ] {
        let id = patch_clap_id(Layer::L1, p).expect("patch param");
        e.set_param(id, v);
    }

    // Topology first, then the depth params — `set_param` mirrors a slot-depth
    // param straight into the table, so writing the table afterwards would be
    // undone by the next automation event and the two would disagree.
    let mut table = MatrixTable::default();
    for (i, &(source, dest, depth, polarity, shape, scale_src, scale_shape)) in
        ROUTES.iter().enumerate()
    {
        table.slots[i] = MatrixSlot {
            source,
            dest,
            depth,
            polarity,
            shape,
            enabled: true,
            scale_src,
            scale_polarity: Polarity::Direct,
            scale_shape,
        };
    }
    *e.matrix_mut(Layer::L1) = table;
    for (i, &(_, _, depth, ..)) in ROUTES.iter().enumerate() {
        let p = ParamId::slot_depth(i).expect("16 slot-depth params exist");
        let id = patch_clap_id(Layer::L1, p).expect("patch param");
        e.set_param(id, depth);
    }

    e
}

/// The engine with the chord down and the controllers off their zero, so the
/// wheel/pressure sources contribute rather than multiplying by nothing.
fn playing_engine() -> Engine {
    let mut e = reference_engine();
    e.set_mod_wheel(0.7);
    e.set_pitch_bend(0.25);
    e.channel_pressure(0, 0.4);
    for &(note, vel) in &CHORD {
        e.note_on(0, note, vel);
    }
    e
}

/// Render `blocks` blocks, returning interleaved L/R samples.
fn render_interleaved(e: &mut Engine, blocks: usize) -> Vec<f32> {
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

/// One second of the reference patch: three quarters held, one quarter ringing
/// out after the note-offs so the release path is in the measurement too.
fn reference_render() -> Vec<f32> {
    let mut e = playing_engine();
    let mut out = render_interleaved(&mut e, HELD_BLOCKS);
    for &(note, _) in &CHORD {
        e.note_off(0, note);
    }
    out.extend(render_interleaved(&mut e, RELEASE_BLOCKS));
    out
}

/// Whether the CI-only render hash is being enforced.
///
/// Any value except `0` counts as on. A bare presence test would make
/// `VXN_RENDER_HASH=0` mean *enabled*, which is the opposite of what anyone
/// typing it intends — and it would drive two gates in opposite directions.
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

    let mut h = std::collections::hash_map::DefaultHasher::new();
    for s in reference_render() {
        s.to_bits().hash(&mut h);
    }

    let got = h.finish();
    println!("BASELINE render hash = {got:#018x}");
    assert_eq!(
        got, EXPECTED,
        "render hash changed: a matrix reorder, a cook-stage change or a ramp-index \
         regression (or an intentional DSP change — run the null test, then re-capture \
         EXPECTED; see header). If this is the FIRST CI run since 0329 landed, EXPECTED \
         is still the dev-hardware capture — take the hash printed above."
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
/// reported peak is the amount, at the sample it was applied to.
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
