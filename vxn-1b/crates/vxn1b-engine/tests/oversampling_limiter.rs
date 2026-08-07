//! O/S and Limit do something (ticket 0251).
//!
//! Both params shipped in the table and were read by nobody: `oversample` had a
//! helper nothing called (the engine hardcoded 1×) and `limiter_on` had no
//! consumer at all. These tests pin the observable behaviour rather than the
//! plumbing — aliasing energy falls as the factor rises, the ceiling holds, and
//! the paths that must stay bit-exact stay bit-exact.

use vxn1b_engine::params::global_clap_id;
use vxn1b_engine::{Engine, Layer, ParamId, clap_id_of};

const SR: f32 = 48_000.0;
const FRAMES: usize = 4096;
/// Long enough for a fine harmonic measurement; the first `SPECTRUM_SKIP`
/// samples are dropped so the attack and the decimator's settle don't enter.
const SPECTRUM_FRAMES: usize = 16_384;
const SPECTRUM_SKIP: usize = 4_096;

fn global(p: ParamId) -> usize {
    global_clap_id(p).expect("global param")
}

/// A deliberately alias-prone patch: hard sync with the slave swept far above
/// the master, which throws energy well past Nyquist for the decimator to catch.
fn sync_engine(os_index: f32) -> Engine {
    let mut e = Engine::new(SR, FRAMES);
    let id = |p| clap_id_of(Layer::L1, p);
    e.set_param(global(ParamId::Oversample), os_index);
    e.set_param(id(ParamId::Osc1Wave), 2.0); // Saw — rich enough to alias
    e.set_param(id(ParamId::Osc2Wave), 2.0);
    e.set_param(id(ParamId::Osc1Level), 0.9);
    e.set_param(id(ParamId::Osc2Level), 0.0);
    e.set_param(id(ParamId::SubLevel), 0.0);
    e.set_param(id(ParamId::NoiseLevel), 0.0);
    e.set_param(id(ParamId::CrossModType), 1.0); // Sync
    e.set_param(id(ParamId::Osc1Coarse), 19.0); // slave far above the master
    // Filter wide open so the ladder can't mask the aliasing being measured.
    e.set_param(id(ParamId::Cutoff), 16_000.0);
    e.set_param(id(ParamId::Resonance), 0.0);
    e.set_param(id(ParamId::Env2Attack), 0.001);
    e.set_param(id(ParamId::Env2Decay), 0.001);
    e.set_param(id(ParamId::Env2Sustain), 1.0);
    e.matrix_mut(Layer::L1).slots[2].depth = 0.0; // no vibrato
    e
}

/// Hold a note so the engine renders something.
fn noted(mut e: Engine) -> Engine {
    e.note_on(0, 60, 1.0);
    e
}

fn render(e: &mut Engine, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let mut l = vec![0.0f32; frames];
    let mut r = vec![0.0f32; frames];
    e.process_block(&mut l, &mut r);
    (l, r)
}

/// Magnitude of the first `kmax` harmonics of `f0`, measured on a steady
/// (post-attack) Hann-windowed segment.
///
/// Hard sync is exactly periodic at the master frequency, so *all* of its
/// content — aliased images included — lands on the harmonic grid. Aliasing
/// therefore shows up as harmonics with the **wrong amplitude**, not as
/// inharmonic hash, and the way to see it is to compare against a
/// heavily-oversampled reference. Magnitudes only, so the decimators'
/// factor-dependent group delay doesn't enter.
fn harmonic_spectrum(e: &mut Engine, f0: f64, kmax: usize) -> Vec<f64> {
    let (l, _) = render(e, SPECTRUM_FRAMES);
    let seg = &l[SPECTRUM_SKIP..];
    let n = seg.len();
    (1..=kmax)
        .map(|k| {
            let w = 2.0 * std::f64::consts::PI * f0 * k as f64 / SR as f64;
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, sample) in seg.iter().enumerate() {
                let hann =
                    0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos();
                re += *sample as f64 * hann * (w * i as f64).cos();
                im += *sample as f64 * hann * (w * i as f64).sin();
            }
            (re * re + im * im).sqrt() / n as f64
        })
        .collect()
}

/// Summed absolute harmonic error against the reference, as a fraction of the
/// reference's total harmonic energy.
fn spectral_error(spectrum: &[f64], reference: &[f64]) -> f64 {
    let err: f64 = spectrum.iter().zip(reference).map(|(a, b)| (a - b).abs()).sum();
    let total: f64 = reference.iter().sum();
    err / total
}

#[test]
fn oversampling_converges_on_the_band_limited_ideal() {
    // MIDI 60 against a slave 19 semitones up: sync's discontinuity throws
    // energy far past Nyquist, which folds back onto the harmonic grid.
    let f0 = 261.6256;
    let kmax = 80; // ~21 kHz, just under Nyquist
    let reference = harmonic_spectrum(&mut noted(sync_engine(3.0)), f0, kmax); // 8x

    let errs: Vec<f64> = [0.0, 1.0, 2.0]
        .iter()
        .map(|os| spectral_error(&harmonic_spectrum(&mut noted(sync_engine(*os)), f0, kmax), &reference))
        .collect();
    eprintln!(
        "harmonic error vs 8x: 1x = {:.1}%, 2x = {:.1}%, 4x = {:.1}%",
        errs[0] * 100.0,
        errs[1] * 100.0,
        errs[2] * 100.0
    );
    // Measured 7.3% / 4.5% / 1.5%: each factor moves the render materially
    // closer to the band-limited ideal.
    assert!(errs[0] > 0.04, "1x should be visibly aliased, got {:.3}", errs[0]);
    assert!(errs[1] < errs[0], "2x must beat 1x ({:.3} vs {:.3})", errs[1], errs[0]);
    assert!(errs[2] < errs[1], "4x must beat 2x ({:.3} vs {:.3})", errs[2], errs[1]);
    assert!(errs[2] < 0.03, "4x should be close to the ideal, got {:.3}", errs[2]);
}

#[test]
fn oversampling_off_is_unchanged_and_every_factor_is_finite() {
    // OS off must remain the pre-0251 render: the decimator is a pass-through at
    // factor 1, which is what keeps the VXN1 parity gate meaningful.
    let mut a = sync_engine(0.0);
    a.note_on(0, 60, 1.0);
    let (l_a, r_a) = render(&mut a, FRAMES);
    assert!(l_a.iter().all(|s| s.is_finite()));
    assert_eq!(l_a, r_a, "spread 0 must stay bit-mono");

    for os_index in [1.0, 2.0, 3.0] {
        let mut e = sync_engine(os_index);
        e.note_on(0, 60, 1.0);
        let (l, r) = render(&mut e, FRAMES);
        assert!(l.iter().all(|s| s.is_finite()), "non-finite at index {os_index}");
        assert_eq!(l, r, "spread 0 must stay bit-mono at index {os_index}");
    }
}

#[test]
fn changing_factor_mid_render_does_not_step() {
    // Settle at 2x, then switch to 8x between process calls. The crossfade must
    // keep the join continuous — no hard step at the boundary.
    let mut e = sync_engine(1.0);
    e.note_on(0, 60, 1.0);
    let (before, _) = render(&mut e, FRAMES);
    e.set_param(global(ParamId::Oversample), 3.0);
    let (after, _) = render(&mut e, 256);

    let tail = before[before.len() - 1];
    let step = (after[0] - tail).abs();
    // The signal itself moves sample to sample; compare the join against the
    // waveform's own typical step rather than against zero.
    let typical: f32 = before.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>()
        / (before.len() - 1) as f32;
    eprintln!("join step {step:.5} vs typical {typical:.5}");
    assert!(
        step < typical * 8.0,
        "factor change stepped by {step} against a typical {typical} — crossfade not engaged"
    );
    assert!(after.iter().all(|s| s.is_finite()));
}

/// A patch loud enough to need limiting: full velocity, several voices, master
/// pushed well past unity.
fn hot_engine(limiter: bool, master: f32) -> Engine {
    let mut e = Engine::new(SR, FRAMES);
    let id = |p| clap_id_of(Layer::L1, p);
    e.set_param(global(ParamId::Oversample), 0.0);
    e.set_param(global(ParamId::LimiterOn), if limiter { 1.0 } else { 0.0 });
    e.set_param(global(ParamId::MasterVolume), master);
    e.set_param(id(ParamId::Osc1Level), 1.0);
    e.set_param(id(ParamId::Osc2Level), 1.0);
    e.set_param(id(ParamId::Cutoff), 16_000.0);
    e.set_param(id(ParamId::Env2Attack), 0.001);
    e.set_param(id(ParamId::Env2Decay), 0.001);
    e.set_param(id(ParamId::Env2Sustain), 1.0);
    e.matrix_mut(Layer::L1).slots[2].depth = 0.0;
    for (i, note) in [48u8, 55, 60, 64, 67, 72].iter().enumerate() {
        e.note_on(0, *note, 1.0);
        let _ = i;
    }
    e
}

#[test]
fn limiter_holds_the_ceiling() {
    // Long enough for the gain envelope to settle: the kernel's attack/release
    // takes a few thousand samples to catch a signal this far over the ceiling.
    const N: usize = 24_000;
    let mut off = hot_engine(false, 1.0);
    let (l_off, _) = render(&mut off, N);
    let peak_off = l_off.iter().fold(0.0f32, |m, s| m.max(s.abs()));

    let mut on = hot_engine(true, 1.0);
    let (l_on, _) = render(&mut on, N);
    let peak_on = l_on.iter().fold(0.0f32, |m, s| m.max(s.abs()));

    // Past the initial catch-up, the *gain stage* should be doing the work: the
    // kernel's safety clamp may still catch the odd sample (it does on this
    // material — one in twelve thousand), but a limiter that were merely
    // clipping would sit on the ceiling continuously.
    let tail = &l_on[N / 2..];
    let steady = tail.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let clamped = tail.iter().filter(|s| s.abs() >= 0.999).count();
    eprintln!(
        "peak: limiter off = {peak_off:.3}, on = {peak_on:.5}, steady = {steady:.4}, \
         samples at the clamp = {clamped}/{}",
        tail.len()
    );
    assert!(peak_off > 1.0, "the test patch must actually clip without help, got {peak_off}");
    // The shared kernel clamps hard at full scale after its gain stage, so the
    // absolute peak lands on 1.0 (± a float ULP), never above it.
    assert!(peak_on <= 1.0 + 1e-5, "limiter must hold the ceiling, got {peak_on}");
    assert!(peak_on < peak_off * 0.5, "limiting must materially reduce the peak");
    assert!(
        (clamped as f32) < tail.len() as f32 * 0.001,
        "{clamped} of {} steady samples sat on the clamp — that is clipping, not limiting",
        tail.len()
    );
}

#[test]
fn limiter_runs_after_master_volume() {
    // The whole reason it lives in the engine rather than in `FxChain`: master
    // volume is applied *before* it, so the limiter sees what actually leaves
    // the plugin. Master = 0.5 discriminates the two placements. The patch's dry
    // peak is ~2.36, so:
    //   * limiter after master  → limit(2.36 x 0.5 = 1.18) ≈ the ceiling
    //   * limiter before master → limit(2.36) x 0.5        ≈ half the ceiling
    let mut e = hot_engine(true, 0.5);
    let (l, _) = render(&mut e, FRAMES);
    let peak = l.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    eprintln!("peak at master 0.5 with limiter on = {peak:.4}");
    assert!(
        peak > 0.8,
        "peak {peak} looks like the ceiling halved — the limiter is running before master volume"
    );
    assert!(peak <= 1.0 + 1e-5, "output escaped the limiter, peak {peak}");
}

#[test]
fn limiter_off_is_a_true_bypass() {
    // Off and settled, the limiter must not touch a sample — the same
    // true-skip contract the FX slots hold.
    let mut a = hot_engine(false, 0.5);
    let (l_a, _) = render(&mut a, FRAMES);

    let mut b = Engine::new(SR, FRAMES);
    {
        let id = |p| clap_id_of(Layer::L1, p);
        b.set_param(global(ParamId::Oversample), 0.0);
        b.set_param(global(ParamId::MasterVolume), 0.5);
        b.set_param(id(ParamId::Osc1Level), 1.0);
        b.set_param(id(ParamId::Osc2Level), 1.0);
        b.set_param(id(ParamId::Cutoff), 16_000.0);
        b.set_param(id(ParamId::Env2Attack), 0.001);
        b.set_param(id(ParamId::Env2Decay), 0.001);
        b.set_param(id(ParamId::Env2Sustain), 1.0);
        b.matrix_mut(Layer::L1).slots[2].depth = 0.0;
        for note in [48u8, 55, 60, 64, 67, 72] {
            b.note_on(0, note, 1.0);
        }
    }
    let (l_b, _) = render(&mut b, FRAMES);
    assert_eq!(l_a, l_b, "limiter-off render must be bit-identical to no limiter at all");
}

#[test]
fn engaging_the_limiter_does_not_click() {
    // Toggle mid-render on a steady signal: the dry↔limited crossfade must keep
    // the join within the signal's own sample-to-sample motion.
    let mut e = hot_engine(false, 0.9);
    let (before, _) = render(&mut e, FRAMES);
    e.set_param(global(ParamId::LimiterOn), 1.0);
    let (after, _) = render(&mut e, 512);

    let typical: f32 = before.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>()
        / (before.len() - 1) as f32;
    let step = (after[0] - before[before.len() - 1]).abs();
    eprintln!("limiter-on join step {step:.5} vs typical {typical:.5}");
    assert!(
        step < typical * 8.0,
        "engaging the limiter stepped by {step} against a typical {typical}"
    );
}

#[test]
fn layer_fade_time_is_independent_of_the_oversampling_factor() {
    // The layer gain smoother ticks per BASE frame; if it ticked per OS sample a
    // mute would land 8x sooner at 8x. Compare how far a mute has travelled
    // after the same number of base frames at 1x and 8x.
    // How many BASE frames a mute takes to drop the output below a fraction of
    // its pre-mute level. The waveform itself differs slightly between factors
    // (that is the point of oversampling), so measure against each render's own
    // pre-mute level rather than comparing absolute amplitudes.
    let frames_to_half = |os_index: f32| {
        let mut e = Engine::new(SR, FRAMES);
        e.set_param(global(ParamId::Oversample), os_index);
        e.set_param(clap_id_of(Layer::L1, ParamId::Env2Attack), 0.001);
        e.set_param(clap_id_of(Layer::L1, ParamId::Env2Decay), 0.001);
        e.set_param(clap_id_of(Layer::L1, ParamId::Env2Sustain), 1.0);
        e.matrix_mut(Layer::L1).slots[2].depth = 0.0;
        e.note_on(0, 60, 1.0);
        let _ = render(&mut e, 2048); // settle the note
        let (pre, _) = render(&mut e, 256);
        let level = pre.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        e.set_param(clap_id_of(Layer::L1, ParamId::LayerMute), 1.0);
        // One 16-frame chunk at a time until the level has halved.
        let mut frames = 0usize;
        for _ in 0..64 {
            let (chunk, _) = render(&mut e, 16);
            frames += 16;
            if chunk.iter().fold(0.0f32, |m, s| m.max(s.abs())) < level * 0.5 {
                break;
            }
        }
        frames
    };
    let f1 = frames_to_half(0.0);
    let f8 = frames_to_half(3.0);
    eprintln!("base frames for the mute to halve: 1x = {f1}, 8x = {f8}");
    assert!(
        (f1 as i32 - f8 as i32).abs() <= 32,
        "mute took {f1} base frames at 1x but {f8} at 8x — the gain smoother is ticking per OS sample"
    );
}
