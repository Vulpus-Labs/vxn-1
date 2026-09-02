//! `CrossModAmount` destination gate (ticket 0242).
//!
//! 0202 shipped the dest in the vocabulary (`DEST_NAMES` / `DEST_GAIN` = 4.0)
//! and 0208 deferred its application, so a matrix slot pointed at Cross-Mod Amt
//! rendered identically to no slot at all. These tests pin the wired behaviour:
//! the route adds to the patch's own amount, per voice, in FM mode only, and
//! clamps non-negative.
//!
//! The probe source is **velocity** — a per-voice constant latched at note-on —
//! so the smoothed offset snaps to its target on the trigger and holds. That
//! makes the modulated render directly comparable to an equivalent *patch*
//! amount, which is a far tighter statement than "the output changed".

use vxn1b_engine::matrix::{Polarity, Shape};
use vxn1b_engine::{DestId, Engine, Layer, MatrixSlot, ParamId, SourceId, clap_id_of};

const SR: f32 = 48_000.0;
const FRAMES: usize = 2048;

/// Cross-mod type variants (`CROSS_MOD_LABELS` order).
const OFF: f32 = 0.0;
const SYNC: f32 = 1.0;
const FM: f32 = 2.0;
const RING: f32 = 3.0;

/// `DEST_GAIN[CrossModAmount]` — depth 1.0 × velocity 1.0 lands this many index
/// units of offset.
const XMOD_GAIN: f32 = 4.0;

/// One held note on a two-oscillator patch, cross-mod `mode`, patch amount
/// `amount`, and optionally a velocity→Cross-Mod Amt route at `depth`.
fn render_patch(mode: f32, amount: f32, route_depth: Option<f32>) -> Vec<f32> {
    let mut e = Engine::new(SR);
    let id = |p| clap_id_of(Layer::L1, p);
    // Sine carrier + sine modulator: PM on sines is the clean case (no
    // aliasing to muddy an equality assertion — `vxn1-crossmod-pm-aliasing`).
    e.set_param(id(ParamId::Osc1Wave), 0.0);
    e.set_param(id(ParamId::Osc2Wave), 0.0);
    e.set_param(id(ParamId::Osc1Level), 0.9);
    e.set_param(id(ParamId::Osc2Level), 0.0);
    e.set_param(id(ParamId::SubLevel), 0.0);
    e.set_param(id(ParamId::NoiseLevel), 0.0);
    // Flat VCA so the comparison isn't dominated by envelope shape.
    e.set_param(id(ParamId::Env2Attack), 0.0005);
    e.set_param(id(ParamId::Env2Decay), 0.001);
    e.set_param(id(ParamId::Env2Sustain), 1.0);
    e.set_param(id(ParamId::CrossModType), mode);
    e.set_param(id(ParamId::CrossModAmount), amount);
    // Drop the seeded LFO1→Pitch vibrato: a moving carrier would swamp the
    // sample-wise comparison.
    e.matrix_mut(Layer::L1).slots[2].depth = 0.0;
    if let Some(depth) = route_depth {
        e.matrix_mut(Layer::L1).slots[3] = MatrixSlot {
            source: SourceId::Velocity,
            dest: DestId::CrossModAmount,
            depth,
            polarity: Polarity::None,
            shape: Shape::Lin,
            enabled: true,
            scale_polarity: Polarity::None,
            scale_shape: Shape::Lin,
            scale_src: SourceId::None,
        };
    }
    e.note_on(0, 60, 1.0);
    let mut l = vec![0.0f32; FRAMES];
    let mut r = vec![0.0f32; FRAMES];
    e.process_block(&mut l, &mut r);
    l
}

fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt() as f32
}

#[test]
fn route_equals_the_same_amount_dialled_into_the_patch() {
    // Velocity 1.0 × depth 0.5 × gain 4.0 = 2.0 index units of offset, on top of
    // a patch amount of 0.5 → 2.5. Dialling 2.5 in by hand must render the same
    // thing: the dest is an additive offset in the patch's own units.
    let modulated = render_patch(FM, 0.5, Some(0.5));
    let dialled = render_patch(FM, 0.5 + 0.5 * XMOD_GAIN, None);
    let d = max_abs_diff(&modulated, &dialled);
    assert!(
        d < 1e-5,
        "velocity→Cross-Mod Amt should equal the same index as a patch amount, max diff {d}"
    );
}

#[test]
fn route_drives_fm_from_a_zero_patch_amount() {
    // "FM swells in from nothing": amount parked at 0, an envelope/velocity
    // route supplying the whole index. Keying PM engagement off the patch
    // scalar alone would render this dry.
    let dry = render_patch(FM, 0.0, None);
    let modulated = render_patch(FM, 0.0, Some(0.5));
    let d = max_abs_diff(&dry, &modulated);
    assert!(
        d > 0.05 * rms(&dry).max(1e-6),
        "amount 0 + an active route must still produce FM (max diff {d})"
    );
}

#[test]
fn dest_is_inert_outside_fm_mode() {
    // Off/Sync/Ring ignore the cross-mod amount, so a route into the dest must
    // not perturb them — including via the smoother, which stays parked.
    for mode in [OFF, SYNC, RING] {
        let plain = render_patch(mode, 1.0, None);
        let routed = render_patch(mode, 1.0, Some(1.0));
        let d = max_abs_diff(&plain, &routed);
        assert!(d < 1e-6, "mode {mode} must ignore Cross-Mod Amt routes, max diff {d}");
    }
}

#[test]
fn negative_total_clamps_to_zero_index() {
    // A downward route below zero must land at index 0 (no through-zero
    // inversion), i.e. the same render as no PM at all.
    let none = render_patch(FM, 0.0, None);
    let negative = render_patch(FM, 0.0, Some(-1.0));
    let d = max_abs_diff(&none, &negative);
    assert!(d < 1e-6, "negative cross-mod total must clamp to 0, max diff {d}");
}

#[test]
fn route_is_per_voice_not_per_bank() {
    // Two notes at different velocities: a per-voice dest must give them
    // different FM indices, so the sum can't equal either voice's index applied
    // to both. Renders the pair against a bank-wide stand-in (both at the
    // harder velocity) and demands they differ.
    let two_velocities = {
        let mut e = Engine::new(SR);
        let id = |p| clap_id_of(Layer::L1, p);
        e.set_param(id(ParamId::Osc1Wave), 0.0);
        e.set_param(id(ParamId::Osc2Wave), 0.0);
        e.set_param(id(ParamId::Osc2Level), 0.0);
        e.set_param(id(ParamId::CrossModType), FM);
        e.set_param(id(ParamId::CrossModAmount), 0.0);
        e.matrix_mut(Layer::L1).slots[2].depth = 0.0;
        e.matrix_mut(Layer::L1).slots[3] = MatrixSlot {
            source: SourceId::Velocity,
            dest: DestId::CrossModAmount,
            depth: 0.5,
            polarity: Polarity::None,
            shape: Shape::Lin,
            enabled: true,
            scale_polarity: Polarity::None,
            scale_shape: Shape::Lin,
            scale_src: SourceId::None,
        };
        e.note_on(0, 60, 1.0);
        e.note_on(0, 67, 0.2);
        let mut l = vec![0.0f32; FRAMES];
        let mut r = vec![0.0f32; FRAMES];
        e.process_block(&mut l, &mut r);
        l
    };
    let one_index = {
        let mut e = Engine::new(SR);
        let id = |p| clap_id_of(Layer::L1, p);
        e.set_param(id(ParamId::Osc1Wave), 0.0);
        e.set_param(id(ParamId::Osc2Wave), 0.0);
        e.set_param(id(ParamId::Osc2Level), 0.0);
        e.set_param(id(ParamId::CrossModType), FM);
        // Both voices at the loud note's index — what a bank-wide apply gives.
        e.set_param(id(ParamId::CrossModAmount), 0.5 * XMOD_GAIN);
        e.matrix_mut(Layer::L1).slots[2].depth = 0.0;
        e.note_on(0, 60, 1.0);
        e.note_on(0, 67, 0.2);
        let mut l = vec![0.0f32; FRAMES];
        let mut r = vec![0.0f32; FRAMES];
        e.process_block(&mut l, &mut r);
        l
    };
    let d = max_abs_diff(&two_velocities, &one_index);
    assert!(d > 1e-3, "the dest must be applied per voice, max diff {d}");
}
