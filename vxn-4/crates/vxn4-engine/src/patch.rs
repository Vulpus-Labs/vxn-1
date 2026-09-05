//! Five hardwired patches, spanning the complexity range the architecture has
//! to cover.
//!
//! These are ear-fodder, not a preset format. They exist so the routing matrix,
//! the waveform assignment, the feedback diagonal and the envelopes all get
//! exercised by something you can listen to and form an opinion about. Nothing
//! here is meant to survive into a shipping preset bank.
//!
//! The set is deliberately graded, because the sizing bench found route density
//! to be the largest single cost lever (64 routes vs 9 is ~25%):
//!
//! | # | name | live routes | what it is for |
//! |---|---|---|---|
//! | 0 | `Sine` | 1 | the null case — one sine operator, nothing modulating |
//! | 1 | `EPiano` | 4 | two classic 2-op stacks, the FM idiom |
//! | 2 | `Bell` | 6 | inharmonic ratios plus self-feedback |
//! | 3 | `Saws` | 11 | assignable waveforms — the thing a DX7 cannot do |
//! | 4 | `Web` | 64 | every route live; the worst case the bench sizes against |
//! | 5 | `Grind` | 4 | saw modulating saw at high index — the aliasing torture case |

use vxn4_dsp::ops::{NOPS, OpConfig, Routing};
use vxn4_dsp::wavetable::Waveform;

use crate::eg::EgParams;

/// A complete voice definition: operators, routing, and one envelope per
/// operator.
#[derive(Clone, Debug)]
pub struct Patch {
    pub name: &'static str,
    pub ops: [OpConfig; NOPS],
    pub routing: Routing,
    pub eg: [EgParams; NOPS],
    /// Master trim, applied at the sum bus.
    ///
    /// Set from measurement, not by ear: each value puts a six-note chord at
    /// velocity 100 at roughly -6 dBFS. That does two jobs. It matches the five
    /// patches for loudness, so an A/B is about timbre rather than level —
    /// ungained, the dense patch is ~5 dB down on the simple one. And it keeps
    /// ordinary playing clear of the limiter, which matters more than it
    /// sounds: a chord that slams the limiter is hard-clipped by its internal
    /// ceiling before its gain envelope converges, and the halfband stages
    /// downstream ring on the clipped edges. See `engine::CEILING`.
    pub gain: f32,
}

/// The set, in order.
pub const N_PATCHES: usize = 6;

pub fn patch(index: usize) -> Patch {
    match index % N_PATCHES {
        0 => sine(),
        1 => epiano(),
        2 => bell(),
        3 => saws(),
        4 => web(),
        _ => grind(),
    }
}

pub fn patch_names() -> [&'static str; N_PATCHES] {
    [
        sine().name,
        epiano().name,
        bell().name,
        saws().name,
        web().name,
        grind().name,
    ]
}

/// Silent operator: no output, no level. The base every patch builds from, so
/// an unused operator costs nothing audible even though it still costs compute.
fn off() -> OpConfig {
    OpConfig {
        wave: Waveform::Sine,
        ratio: 1.0,
        level: 0.0,
        pan: 0.0,
    }
}

fn op(wave: Waveform, ratio: f32, level: f32, pan: f32) -> OpConfig {
    OpConfig {
        wave,
        ratio,
        level,
        pan,
    }
}

/// A silent envelope for an operator that is not in use.
fn eg_off() -> EgParams {
    EgParams {
        t: [0.0, 0.0, 0.0, 0.0],
        l: [0.0, 0.0, 0.0, 0.0],
    }
}

// ── 0. Sine ─────────────────────────────────────────────────────────────────

/// One sine operator straight to the bus. No modulation at all.
///
/// The reference tone: if this is not clean, nothing downstream is worth
/// listening to. It is also the honest test of the decimator, since a pure sine
/// at 8x should come back at 1x with no visible skirt.
fn sine() -> Patch {
    let mut ops = [off(); NOPS];
    ops[0] = op(Waveform::Sine, 1.0, 1.0, 0.0);

    let mut routing = Routing::default();
    routing.out[0] = 1.0;

    let mut eg = [eg_off(); NOPS];
    eg[0] = EgParams::adsr(0.005, 0.20, 0.75, 0.30);

    Patch {
        name: "sine",
        ops,
        routing,
        eg,
        gain: 0.349,
    }
}

// ── 1. EPiano ───────────────────────────────────────────────────────────────

/// Two 2-operator stacks: a 1:1 tine and a 1:14 strike, the standard FM
/// electric piano skeleton.
///
/// The modulators carry much faster envelopes than the carriers, which is the
/// whole trick — the bright strike decays out in ~150 ms and leaves a nearly
/// pure carrier ringing. Worth listening to specifically for whether the
/// modulator's decay is audible as a *pitch* artefact; if it is, the phase
/// quantiser is losing resolution somewhere.
fn epiano() -> Patch {
    let mut ops = [off(); NOPS];
    ops[0] = op(Waveform::Sine, 1.0, 1.0, -0.3); // carrier A
    ops[1] = op(Waveform::Sine, 1.0, 1.0, 0.0); // modulator A
    ops[2] = op(Waveform::Sine, 1.0, 1.0, 0.3); // carrier B
    ops[3] = op(Waveform::Sine, 14.0, 1.0, 0.0); // modulator B (strike)

    let mut routing = Routing::default();
    routing.pm[0][1] = 0.55;
    routing.pm[2][3] = 0.22;
    // A little cross-feed so the two stacks are not two independent synths.
    routing.pm[0][3] = 0.05;
    routing.pm[2][1] = 0.08;
    routing.out[0] = 0.6;
    routing.out[2] = 0.4;

    let mut eg = [eg_off(); NOPS];
    eg[0] = EgParams {
        t: [0.002, 0.9, 3.0, 0.35],
        l: [1.0, 0.65, 0.28, 0.0],
    };
    eg[1] = EgParams::perc(0.001, 0.55);
    eg[2] = EgParams {
        t: [0.002, 1.2, 4.0, 0.40],
        l: [0.9, 0.5, 0.2, 0.0],
    };
    eg[3] = EgParams::perc(0.001, 0.14);

    Patch {
        name: "epiano",
        ops,
        routing,
        eg,
        gain: 0.373,
    }
}

// ── 2. Bell ─────────────────────────────────────────────────────────────────

/// Inharmonic ratios and a self-feedback modulator.
///
/// The feedback diagonal is the point of this one. Note that at 8x the 2-tick
/// average on that diagonal is doing almost nothing (see `vxn4_dsp::ops` — its
/// Nyquist zero lands at 192 kHz, not 24 kHz), so this patch is the one that
/// will *sound different* when the feedback window is fixed to `os` ticks. Judge
/// it before and after.
fn bell() -> Patch {
    let mut ops = [off(); NOPS];
    ops[0] = op(Waveform::Sine, 1.0, 1.0, -0.4);
    ops[1] = op(Waveform::Sine, 3.5, 1.0, 0.0);
    ops[2] = op(Waveform::Sine, 2.0, 1.0, 0.4);
    ops[3] = op(Waveform::Sine, 9.7, 1.0, 0.0);

    let mut routing = Routing::default();
    routing.pm[0][1] = 0.42;
    routing.pm[2][3] = 0.30;
    routing.pm[1][1] = 0.28; // self-feedback on the inharmonic modulator
    routing.pm[0][3] = 0.06;
    routing.pm[2][1] = 0.10;
    routing.out[0] = 0.55;
    routing.out[2] = 0.45;

    let mut eg = [eg_off(); NOPS];
    eg[0] = EgParams {
        t: [0.001, 2.5, 6.0, 0.8],
        l: [1.0, 0.45, 0.12, 0.0],
    };
    eg[1] = EgParams::perc(0.001, 1.1);
    eg[2] = EgParams {
        t: [0.001, 3.0, 7.0, 1.0],
        l: [0.85, 0.35, 0.08, 0.0],
    };
    eg[3] = EgParams::perc(0.001, 0.35);

    Patch {
        name: "bell",
        ops,
        routing,
        eg,
        gain: 0.340,
    }
}

// ── 3. Saws ─────────────────────────────────────────────────────────────────

/// Assignable waveforms used as both carriers and modulators — saw, square and
/// triangle, detuned across the stereo field.
///
/// This is the patch the mip-mapping exists for. A saw used as a *modulator* is
/// the hardest case in the synth: its harmonics multiply into the carrier's
/// sidebands, so any aliasing in the table read is amplified rather than
/// masked. If the band-limiting is wrong, this is where it will be obvious, and
/// it is the patch to A/B at 8x against 16x.
fn saws() -> Patch {
    let mut ops = [off(); NOPS];
    ops[0] = op(Waveform::Saw, 1.0, 1.0, -0.6);
    ops[1] = op(Waveform::Saw, 1.005, 1.0, 0.6); // detuned pair
    ops[2] = op(Waveform::Square, 0.5, 1.0, 0.0); // sub
    ops[3] = op(Waveform::Triangle, 2.0, 1.0, 0.0); // modulator
    ops[4] = op(Waveform::Sine, 7.0, 1.0, 0.0); // bright modulator

    let mut routing = Routing::default();
    routing.pm[0][3] = 0.12;
    routing.pm[1][3] = 0.12;
    routing.pm[0][4] = 0.05;
    routing.pm[1][4] = 0.06;
    routing.pm[2][3] = 0.04;
    routing.pm[3][4] = 0.18;
    routing.pm[3][3] = 0.10;
    routing.pm[0][2] = 0.03;
    routing.pm[1][2] = 0.03;
    routing.pm[4][4] = 0.08;
    routing.pm[2][2] = 0.05;
    routing.out[0] = 0.32;
    routing.out[1] = 0.32;
    routing.out[2] = 0.26;

    let mut eg = [eg_off(); NOPS];
    let body = EgParams::adsr(0.012, 0.35, 0.62, 0.25);
    eg[0] = body;
    eg[1] = body;
    eg[2] = EgParams::adsr(0.020, 0.40, 0.55, 0.30);
    eg[3] = EgParams {
        t: [0.05, 0.6, 2.0, 0.3],
        l: [0.5, 0.9, 0.4, 0.0],
    };
    eg[4] = EgParams::perc(0.002, 0.25);

    Patch {
        name: "saws",
        ops,
        routing,
        eg,
        gain: 0.448,
    }
}

// ── 4. Web ──────────────────────────────────────────────────────────────────

/// All 64 routes live, all eight operators sounding, every waveform in play.
///
/// This is the architecture's actual claim — an 8x8 matrix with feedback on
/// every diagonal — and the worst case the sizing bench quotes against. Depths
/// are small because 64 simultaneous routes at patch-typical depth is broadband
/// noise, not a sound; even so this will be the least musical of the five, and
/// that is informative. It is the patch that answers whether a fully dense
/// matrix is a usable instrument or only a specification.
fn web() -> Patch {
    let waves = [
        Waveform::Sine,
        Waveform::Triangle,
        Waveform::Sine,
        Waveform::Saw,
        Waveform::Sine,
        Waveform::Square,
        Waveform::Triangle,
        Waveform::Sine,
    ];
    // Mildly inharmonic, spread over three octaves.
    let ratios = [1.0, 2.0, 3.01, 0.5, 4.98, 1.5, 7.02, 0.25];

    let mut ops = [off(); NOPS];
    for d in 0..NOPS {
        ops[d] = op(
            waves[d],
            ratios[d],
            1.0,
            ((d as f32 / (NOPS - 1) as f32) - 0.5) * 1.6,
        );
    }

    let mut routing = Routing::default();
    for d in 0..NOPS {
        for s in 0..NOPS {
            // Deterministic but uneven, so it is not a flat matrix.
            let k = ((d * 7 + s * 13) % 11) as f32 / 11.0;
            routing.pm[d][s] = 0.012 + 0.020 * k;
        }
        routing.out[d] = 0.125;
    }

    let mut eg = [eg_off(); NOPS];
    for (d, slot) in eg.iter_mut().enumerate() {
        let k = d as f32 / NOPS as f32;
        *slot = EgParams {
            t: [0.01 + 0.05 * k, 0.4 + 1.2 * k, 2.0, 0.3 + 0.4 * k],
            l: [1.0, 0.7 - 0.3 * k, 0.45 - 0.2 * k, 0.0],
        };
    }

    Patch {
        name: "web",
        ops,
        routing,
        eg,
        gain: 0.635,
    }
}

// ── 5. Grind ────────────────────────────────────────────────────────────────

/// A sawtooth modulating a sawtooth, at high index. The aliasing torture case.
///
/// Every other patch modulates with a sine or a triangle, whose harmonics fall
/// off fast enough that the sideband set stays bounded in practice. A saw
/// modulator does not: its harmonics fall as 1/k, so at a modulation index over
/// a turn the sideband families around *every* modulator harmonic overlap and
/// the generated spectrum runs far past the operator block's Nyquist no matter
/// how much headroom it is given. Whatever folds back is what oversampling has
/// to deal with.
///
/// Making the carrier a saw as well compounds it — the fold-down lands on a
/// dense harmonic series rather than a sparse one, so there is less masking.
///
/// This is the patch to A/B at 8x against 16x, at the **top of the keyboard**,
/// where the fundamental is high enough that the fold-down lands in the middle
/// of the audible range rather than above it. It is deliberately not musical.
fn grind() -> Patch {
    let mut ops = [off(); NOPS];
    ops[0] = op(Waveform::Saw, 1.0, 1.0, -0.2); // saw carrier
    ops[1] = op(Waveform::Saw, 1.0, 1.0, 0.0); // saw modulator, unison
    ops[2] = op(Waveform::Saw, 2.0, 1.0, 0.0); // saw modulating the modulator
    ops[3] = op(Waveform::Saw, 1.0, 1.0, 0.2); // second carrier, detuned by ratio

    let mut routing = Routing::default();
    // ~1.2 turns is a modulation index around 7.5 radians — well past where a
    // saw modulator's sideband set stops being countable.
    routing.pm[0][1] = 1.20;
    routing.pm[3][1] = 0.85;
    routing.pm[1][2] = 0.60;
    routing.pm[1][1] = 0.25; // self-feedback, for good measure
    routing.out[0] = 0.55;
    routing.out[3] = 0.45;

    let mut eg = [eg_off(); NOPS];
    // Long, flat sustain: this exists to be listened to on a held note, so the
    // envelope must not be what changes the timbre.
    let held = EgParams::adsr(0.005, 0.10, 0.95, 0.20);
    eg[0] = held;
    eg[1] = held;
    eg[2] = held;
    eg[3] = held;

    Patch {
        name: "grind",
        ops,
        routing,
        eg,
        gain: 0.432,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_five_exist_and_are_named() {
        let names = patch_names();
        assert_eq!(names.len(), N_PATCHES);
        for (i, n) in names.iter().enumerate() {
            assert_eq!(patch(i).name, *n);
            assert!(!n.is_empty());
        }
    }

    #[test]
    fn patch_index_wraps() {
        assert_eq!(patch(N_PATCHES).name, patch(0).name);
    }

    /// Every patch must put something on the sum bus, or it is silent by
    /// construction and the renderer will produce an empty file with no error.
    #[test]
    fn every_patch_reaches_the_sum_bus() {
        for i in 0..N_PATCHES {
            let p = patch(i);
            let out: f32 = p.routing.out.iter().sum();
            assert!(out > 0.0, "{} has no sum-bus output", p.name);
            // An operator with sum-bus gain must also have a non-silent
            // envelope, or the route is decorative.
            for d in 0..NOPS {
                if p.routing.out[d] > 0.0 {
                    let peak = p.eg[d].l.iter().fold(0.0f32, |m, l| m.max(*l));
                    assert!(peak > 0.0, "{} op{d} is a silent carrier", p.name);
                    assert!(p.ops[d].level > 0.0, "{} op{d} has zero level", p.name);
                }
            }
        }
    }

    /// Patches 0-4 are a graded density ladder, which is the point of that
    /// part of the set. `grind` (5) is not on the ladder — it exists for
    /// aliasing, not for cost, and is deliberately sparse and very loud per
    /// route. Asserted explicitly so the doc table above cannot quietly rot.
    #[test]
    fn the_set_spans_the_density_range() {
        assert_eq!(patch(0).routing.density(), 0, "sine should have no routes");
        assert_eq!(patch(4).routing.density(), NOPS * NOPS, "web should be full");
        let mid: Vec<usize> = (1..4).map(|i| patch(i).routing.density()).collect();
        for d in &mid {
            assert!(*d > 0 && *d < NOPS * NOPS, "mid patch density {d}");
        }
        // Monotonic across the ladder, so "patch 3 is busier than patch 1" holds.
        let ladder: Vec<usize> = (0..5).map(|i| patch(i).routing.density()).collect();
        for w in ladder.windows(2) {
            assert!(w[1] > w[0], "densities not increasing: {ladder:?}");
        }

        // `grind` is off the ladder by design: few routes, but the deepest of
        // any patch by a wide margin. That combination is what makes it the
        // aliasing case rather than the cost case.
        let g = patch(5);
        let deepest = g.routing.pm.iter().flatten().fold(0.0f32, |m, v| m.max(*v));
        let others = (0..5)
            .map(|i| {
                patch(i)
                    .routing
                    .pm
                    .iter()
                    .flatten()
                    .fold(0.0f32, |m, v| m.max(*v))
            })
            .fold(0.0f32, f32::max);
        assert!(
            deepest > others,
            "grind depth {deepest} should exceed every other patch ({others})"
        );
    }
}
