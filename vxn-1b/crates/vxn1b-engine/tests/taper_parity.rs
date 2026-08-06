//! Fader-calibration parity with VXN1 (ticket 0243).
//!
//! Two halves, because a fader's feel is two things:
//!
//! 1. **The descriptor.** Every float param VXN1b shares by name with VXN1 must
//!    declare the same range, default, unit and *taper*. A shared name that
//!    drifts to a different curve is a control that feels different for no
//!    stated reason.
//! 2. **The application.** `SharedParams`'s normalised accessors — the only path
//!    the editor's faders write and read — must apply that taper (`to_fader` /
//!    `from_fader`), as VXN1's do. VXN1b's fork used the linear
//!    `to_normalized` / `from_normalized` pair, so every `Exp` param was
//!    effectively uncalibrated in the editor: cutoff's whole low end sat in the
//!    bottom twentieth of the travel.
//!
//! CLAP and the preset/state formats exchange **plain** values, so neither half
//! touches automation or persistence.

use vxn_app::{GlobalParam, PatchParam};
use vxn_core_app::{ParamDesc, ParamKind, Taper};
use vxn1b_engine::{ParamId, SharedParams, clap_id_of, Layer};

/// The comparable shape of a float param: everything that decides where a value
/// lands on the fader.
#[derive(PartialEq, Debug)]
struct Calibration {
    min: f32,
    max: f32,
    default: f32,
    unit: &'static str,
    taper: Taper,
}

fn calibration(d: &ParamDesc) -> Option<Calibration> {
    match d.kind {
        ParamKind::Float { unit, taper } => Some(Calibration {
            min: d.min,
            max: d.max,
            default: d.default,
            unit,
            taper,
        }),
        _ => None,
    }
}

fn vxn1_floats() -> Vec<(&'static str, Calibration)> {
    let patch = PatchParam::all().map(|p| p.desc());
    let global = GlobalParam::all().map(|p| p.desc());
    patch
        .chain(global)
        .filter_map(|d| calibration(d).map(|c| (d.name, c)))
        .collect()
}

fn vxn1b_floats() -> Vec<(&'static str, Calibration)> {
    ParamId::all()
        .filter_map(|p| {
            let d = p.desc();
            calibration(d).map(|c| (d.name, c))
        })
        .collect()
}

#[test]
fn shared_param_names_carry_identical_calibration() {
    let v1 = vxn1_floats();
    let mut mismatches = Vec::new();
    for (name, cal_1b) in vxn1b_floats() {
        let Some((_, cal_1)) = v1.iter().find(|(n, _)| *n == name) else {
            continue; // VXN1b-only (matrix depths, dynamics, split rate…)
        };
        if *cal_1 != cal_1b {
            mismatches.push(format!("{name}\n  vxn1 : {cal_1:?}\n  vxn1b: {cal_1b:?}"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "shared params drifted from VXN1's calibration:\n{}",
        mismatches.join("\n")
    );
}

/// VXN1's LFO rate is one param; VXN1b has a per-layer LFO 1 and LFO 2. They are
/// the same control, so they take the same calibration — checked by hand since
/// the names differ.
#[test]
fn split_lfo_rates_match_vxn1s_single_rate() {
    let v1 = vxn1_floats();
    let (_, rate) = v1.iter().find(|(n, _)| *n == "lfo_rate").expect("vxn1 lfo_rate");
    for p in [ParamId::Lfo1Rate, ParamId::Lfo2Rate] {
        let cal = calibration(p.desc()).expect("float");
        assert_eq!(&cal, rate, "{} diverged from VXN1's lfo_rate", p.desc().name);
    }
}

/// The accessors the editor actually drives must be the tapered pair. Cutoff is
/// the canonical case: `Exp { mid: 800 }`, so half travel is 800 Hz — not the
/// 8 kHz a linear mapping would give.
#[test]
fn shared_params_normalised_accessors_are_tapered() {
    let params = SharedParams::new();
    let cutoff = clap_id_of(Layer::L1, ParamId::Cutoff);

    params.set_normalized(cutoff, 0.5);
    let mid = params.get(cutoff);
    assert!(
        (mid - 800.0).abs() < 1.0,
        "half travel should read the taper's midpoint (800 Hz), got {mid}"
    );

    // Round-trip: position → value → position, across the travel.
    for i in 0..=20 {
        let n = i as f32 / 20.0;
        params.set_normalized(cutoff, n);
        let back = params.get_normalized(cutoff);
        assert!((back - n).abs() < 1e-3, "fader position {n} round-tripped to {back}");
    }

    // The low end gets real travel: an octave near the bottom must move the
    // fader by roughly as much as an octave near the top (that is the whole
    // point of the exponential taper).
    let span = |lo: f32, hi: f32| {
        params.set(cutoff, lo);
        let a = params.get_normalized(cutoff);
        params.set(cutoff, hi);
        params.get_normalized(cutoff) - a
    };
    let low_octave = span(50.0, 100.0);
    let high_octave = span(4000.0, 8000.0);
    assert!(low_octave > 0.05, "an octave at the bottom got {low_octave} of travel");
    assert!(
        (low_octave - high_octave).abs() < 0.06,
        "octaves should get comparable travel, got low {low_octave} vs high {high_octave}"
    );
}

/// Linear params must be untouched by the change — `to_fader` falls through to
/// the linear mapping, so a linear fader still reads its plain fraction.
#[test]
fn linear_params_are_unchanged_by_the_taper_path() {
    let params = SharedParams::new();
    let level = clap_id_of(Layer::L1, ParamId::Osc1Level);
    params.set_normalized(level, 0.25);
    assert!((params.get(level) - 0.25).abs() < 1e-6);

    let tune = clap_id_of(Layer::L1, ParamId::MasterTune);
    params.set_normalized(tune, 0.5);
    assert!(params.get(tune).abs() < 1e-5, "bipolar centre stays centred");
}
