//! Six hardwired patches, spanning the complexity range the architecture has
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
//! | 6 | `Supersaw` | 0 | seven detuned saws; every parameter is a macro |

use vxn_core_matrix::curve::{Polarity, Shape};
use vxn_core_matrix::slot::MatrixSlot;
use vxn4_dsp::ops::{DEFAULT_DAMP_HZ, NOPS, OpConfig, Routing};
use vxn4_dsp::wavetable::Waveform;

use crate::eg::EgParams;
use crate::matrix::{DestId, Matrix, SourceId};

/// One matrix slot, wired and switched on.
///
/// `depth` is **raw** — untapered. `DestId::cook_depth` applies the PM cubic at
/// evaluation, and cooking here as well would cube an already-cubed depth. That
/// hazard is called out twice in `vxn_core_matrix::slot`; it is worth naming a
/// third time at the only place in vxn-4 that authors a depth.
fn route(source: SourceId, dest: DestId, depth: f32) -> MatrixSlot<SourceId, DestId> {
    MatrixSlot {
        source,
        dest,
        depth,
        enabled: true,
        ..MatrixSlot::default()
    }
}

/// [`route`], with a second macro as a VCA on the route's depth.
///
/// The brief's "two sources per out, additive and scaling" — `source` adds,
/// `scale_src` multiplies. Both come from the same 8-macro table, so the pair
/// can never form a cycle.
fn scaled_route(
    source: SourceId,
    scale: SourceId,
    dest: DestId,
    depth: f32,
) -> MatrixSlot<SourceId, DestId> {
    MatrixSlot {
        scale_src: scale,
        scale_polarity: Polarity::None,
        scale_shape: Shape::Lin,
        ..route(source, dest, depth)
    }
}

/// Fill a table from a slice, leaving the remaining slots blank.
fn matrix(slots: &[MatrixSlot<SourceId, DestId>]) -> Matrix {
    let mut m = Matrix::default();
    m.slots[..slots.len()].copy_from_slice(slots);
    m
}

/// A complete voice definition: operators, routing, envelopes, and the
/// modulation table the macro knobs drive.
#[derive(Clone, Debug)]
pub struct Patch {
    pub name: &'static str,
    pub ops: [OpConfig; NOPS],
    /// Authored depths — where every route sits with all macros at zero.
    pub routing: Routing,
    /// What the eight macro knobs do to [`Self::routing`]. Totals are
    /// **added** to the authored depths, so a macro at zero is the patch as
    /// written.
    pub matrix: Matrix,
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
pub const N_PATCHES: usize = 7;

pub fn patch(index: usize) -> Patch {
    match index % N_PATCHES {
        0 => sine(),
        1 => epiano(),
        2 => bell(),
        3 => saws(),
        4 => web(),
        5 => grind(),
        _ => supersaw(),
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
        supersaw().name,
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
        damp_hz: DEFAULT_DAMP_HZ,
        // 1.0 is the historical decorrelating hash, bit-exactly. Every patch
        // but `supersaw` was voiced against it.
        phase: 0.0,
        phase_spread: 1.0,
    }
}

/// An operator at the default damping corner — nearly transparent, and what
/// every patch here uses until one is voiced deliberately against it.
fn op(wave: Waveform, ratio: f32, level: f32, pan: f32) -> OpConfig {
    OpConfig {
        wave,
        ratio,
        level,
        pan,
        damp_hz: DEFAULT_DAMP_HZ,
        // 1.0 is the historical decorrelating hash, bit-exactly. Every patch
        // but `supersaw` was voiced against it.
        phase: 0.0,
        phase_spread: 1.0,
    }
}

/// [`op`], phase-**coherent** at note onset instead of decorrelated by hash.
///
/// `phase_spread: 0.0` is what a unison stack needs and what nothing else in
/// the set does; `phase` places the operator within the cycle. See
/// [`OpConfig::phase_spread`] for why the two are separate axes, and why a
/// patch wants to be able to travel between them rather than pick one.
fn op_coherent(wave: Waveform, ratio: f32, level: f32, pan: f32, phase: f32) -> OpConfig {
    OpConfig {
        phase,
        phase_spread: 0.0,
        ..op(wave, ratio, level, pan)
    }
}

/// [`op`], with the modulation-input damping dialled down.
///
/// The bounded-chaos axis: lower corners bound harder what an operator can
/// receive, which is what lets a route be pushed to a depth that would
/// otherwise go straight to broadband noise.
fn op_damped(wave: Waveform, ratio: f32, level: f32, pan: f32, damp_hz: f32) -> OpConfig {
    OpConfig {
        damp_hz,
        ..op(wave, ratio, level, pan)
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
        // Macro 1 opens the self-feedback diagonal, taking a pure sine to a
        // buzz — from an authored depth of zero, so the knob is the only thing
        // that makes this patch anything but a sine.
        //
        // The diagonal needs no lane reserved for it: `CompiledRouting` keeps
        // feedback in its own `fb` array, which always has a slot per operator.
        // The force mask matters for *off*-diagonal routes, which `saws`
        // exercises.
        // M1 opens the feedback, M2 damps it: amount and brightness of the
        // same buzz on two knobs. The clearest pair in the set for hearing what
        // damping does, because nothing else is happening.
        matrix: matrix(&[
            route(SourceId::Macro1, DestId::Pm00, 0.70),
            route(SourceId::Macro2, DestId::Damp0, -0.85),
        ]),
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
        // M1 brightness on both stacks; M2 cross-feed, gated by M3 — the
        // additive-plus-scaling pair the brief asks for, on one route.
        matrix: matrix(&[
            route(SourceId::Macro1, DestId::Pm01, 0.63),
            route(SourceId::Macro1, DestId::Pm23, 0.63),
            scaled_route(SourceId::Macro2, SourceId::Macro3, DestId::Pm03, 0.55),
            // M4 damps both carriers. These are the operators receiving the
            // 14:1 strike, so this is the knob for the effect heard at the top
            // of the keyboard: the strike loses ~10 dB of index by C8 at the
            // default corner, and this makes that a dial rather than a fixed
            // property of the patch.
            route(SourceId::Macro4, DestId::Damp0, -0.55),
            route(SourceId::Macro4, DestId::Damp2, -0.55),
        ]),
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
        matrix: matrix(&[
            route(SourceId::Macro1, DestId::Pm11, 0.72),
            route(SourceId::Macro2, DestId::Pm01, 0.60),
            route(SourceId::Macro2, DestId::Pm23, 0.60),
            // M3 damps the carriers; M4 damps the self-feedback modulator,
            // which is where the inharmonic character comes from.
            route(SourceId::Macro3, DestId::Damp0, -0.60),
            route(SourceId::Macro3, DestId::Damp2, -0.60),
            route(SourceId::Macro4, DestId::Damp1, -0.75),
        ]),
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
        // M2 is the one out-dest route in the set: linear taper, straight onto
        // the sub's sum-bus send.
        //
        // M4 is the force-mask case. `pm[2][4]` is an off-diagonal route this
        // patch does not author, so without a lane reserved for it there is
        // nothing for `set_pm` to write and the knob is inert. Op4 is a live
        // bright modulator, so the route has something real behind it —
        // reserving a lane onto a silent operator would test the mask against
        // a route that could not be heard either way. Pinned by
        // `engine::tests::the_force_mask_is_exercised_by_the_patch_set`.
        matrix: matrix(&[
            route(SourceId::Macro1, DestId::Pm03, 0.60),
            route(SourceId::Macro1, DestId::Pm13, 0.60),
            route(SourceId::Macro2, DestId::Out2, 0.35),
            route(SourceId::Macro3, DestId::Pm34, 0.70),
            route(SourceId::Macro4, DestId::Pm24, 0.65),
            // M5 damps the two saw carriers together. This is the patch where
            // the modulators have real harmonic content, so damping bites
            // harder here than on the all-sine patches.
            route(SourceId::Macro5, DestId::Damp0, -0.70),
            route(SourceId::Macro5, DestId::Damp1, -0.70),
        ]),
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
        ops[d] = op_damped(
            waves[d],
            ratios[d],
            1.0,
            ((d as f32 / (NOPS - 1) as f32) - 0.5) * 1.6,
            // This patch is the reason the damping exists. It has seven
            // multi-operator cycles and every one of them was previously
            // undamped — stable only because the depths below were kept tiny,
            // which is gain staging standing in for design. A pole per operator
            // puts one filter in every cycle at every hop, so depth becomes a
            // usable axis instead of the thing holding the patch together.
            //
            // Spread across operators rather than uniform: identical corners
            // make every cycle roll off identically, which is a duller and less
            // interesting object than one where the loops differ.
            5_000.0 + 1_500.0 * d as f32,
        );
    }

    let mut routing = Routing::default();
    for d in 0..NOPS {
        for s in 0..NOPS {
            // Deterministic but uneven, so it is not a flat matrix.
            //
            // These were 0.012..0.032 — small enough that 64 simultaneous
            // routes stayed short of broadband noise on their own, which is
            // gain staging standing in for design. With a pole in every cycle
            // at every hop the depths can be what the patch actually wants,
            // and M3's master damping has something to work on: at the old
            // depths the knob moved the render by only 18 dB because there was
            // barely any modulation to damp.
            let k = ((d * 7 + s * 13) % 11) as f32 / 11.0;
            routing.pm[d][s] = 0.045 + 0.070 * k;
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
        // M1 opens four feedback diagonals at once, M2 the two corner routes.
        // Every route here already has an authored depth, so unlike `sine` this
        // exercises the *additive* path rather than the force mask.
        matrix: matrix(&[
            route(SourceId::Macro1, DestId::Pm00, 0.55),
            route(SourceId::Macro1, DestId::Pm11, 0.55),
            route(SourceId::Macro1, DestId::Pm22, 0.55),
            route(SourceId::Macro1, DestId::Pm33, 0.55),
            route(SourceId::Macro2, DestId::Pm07, 0.50),
            route(SourceId::Macro2, DestId::Pm70, 0.50),
            // M3 damps all eight operators at once — the master bounded-chaos
            // control, and the reason the slot table is 16 wide. Every one of
            // the seven cycles in this patch passes through several of these,
            // so one knob changes how fast the whole thing fills its spectrum.
            route(SourceId::Macro3, DestId::Damp0, -0.50),
            route(SourceId::Macro3, DestId::Damp1, -0.50),
            route(SourceId::Macro3, DestId::Damp2, -0.50),
            route(SourceId::Macro3, DestId::Damp3, -0.50),
            route(SourceId::Macro3, DestId::Damp4, -0.50),
            route(SourceId::Macro3, DestId::Damp5, -0.50),
            route(SourceId::Macro3, DestId::Damp6, -0.50),
            route(SourceId::Macro3, DestId::Damp7, -0.50),
        ]),
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
        // M1 drives the index past 1.7 turns, which is the point of the patch:
        // it makes the 8x-vs-16x difference a knob rather than a recompile.
        matrix: matrix(&[
            // Both carriers, at the same depth. M1 drove only op0 until now,
            // which made the left side (op0 pans -0.2) progressively grittier
            // than the right (op3, +0.2) as the knob came up — an accident of
            // writing the route without looking at the pair.
            route(SourceId::Macro1, DestId::Pm01, 0.79),
            route(SourceId::Macro1, DestId::Pm31, 0.79),
            scaled_route(SourceId::Macro2, SourceId::Macro3, DestId::Pm11, 0.63),
            // M4 damps both saw carriers — the fizz control. Down is tame, up
            // (knob at zero) is the SID-ish grit the patch has by default.
            // Deliberately the widest span in the set: this is the patch where
            // bounded chaos is most audible as a range rather than a setting.
            route(SourceId::Macro4, DestId::Damp0, -0.90),
            route(SourceId::Macro4, DestId::Damp3, -0.90),
        ]),
        eg,
        gain: 0.432,
    }
}

// ── 6. Supersaw ─────────────────────────────────────────────────────────────

/// Seven saws in a detuned, panned, level-tapered spread, plus a sine
/// modulating all of them. Every parameter of the spread is a macro.
///
/// The patch that motivated putting **detune** and **pan** in the roster. Both
/// are voice architecture rather than modulation in the brief's sense, and
/// without them a supersaw is three recompiles rather than three knobs.
///
/// Ops 0-6 are the spread: op0 at pitch, ops 1-3 sharp and right, ops 4-6 flat
/// and left. Op7 is the modulator, which reaches all seven.
///
/// **Every macro sits at zero in the authored patch**, so the base state is
/// seven unison saws — a thick, comb-filtered single saw, since `reset_lane`
/// decorrelates each operator's starting phase. That is deliberate: the ask was
/// for knobs that *determine* detune, width and taper, so each has to start
/// from nothing and add. It also makes each knob's contribution audible in
/// isolation, which a pre-voiced base would hide.
fn supersaw() -> Patch {
    // Distance from centre, in spread units. Sign is the side.
    const OFFSET: [f32; 7] = [0.0, 1.0, 2.0, 3.0, -1.0, -2.0, -3.0];

    let mut ops = [off(); NOPS];
    for (d, k) in OFFSET.iter().enumerate() {
        // Ratio, pan and level are all authored *neutral*; the macros supply
        // the spread. `level` is the operator's own gain, distinct from
        // `routing.out` below, which is the sum-bus send the taper moves.
        // Every saw starts at the *same* phase. Coherent, so the seven sum to
        // seven times one saw rather than to a comb — the hash they used to get
        // made them cancel, which is what made this patch quiet and thin.
        //
        // Not an even `d/7` spread, which is the intuitive choice and is much
        // worse: it cancels every harmonic that is not a multiple of seven.
        // Detune is what should break the coherence, and M1 is the knob for it.
        ops[d] = op_coherent(Waveform::Saw, 1.0, 1.0, 0.0, 0.0);
        let _ = k;
    }
    // The modulator. Ratio 7 rather than unison, and that is the whole
    // difference between M4 being a control and M4 being inaudible.
    //
    // At 1:1 every sideband lands on a harmonic the saw already has, so the
    // pitch, the harmonic positions and the 1/k envelope all stay put and only
    // the amplitudes shuffle — a large change in the *waveform* and almost none
    // in what you hear. Measured, it moved the spectral centroid from 4302 Hz
    // to 3447 Hz: it made the patch marginally *darker*. At ratio 7 the same
    // depth takes it to 5498 Hz, and at full depth to 8212 Hz.
    //
    // Still harmonic, so this stays a supersaw rather than becoming a bell —
    // an inharmonic ratio here is louder still but a different instrument.
    // The modulator starts coherent with the stack too. As M1 detunes the
    // saws apart, it goes progressively in and out of phase with each of them
    // at a different rate — so the modulation is not one uniform effect across
    // the spread, which is most of what makes it sound like seven voices.
    ops[7] = op_coherent(Waveform::Sine, 7.0, 1.0, 0.0, 0.0);

    let mut routing = Routing::default();
    for d in 0..7 {
        // Seven equal sends. The taper is subtractive from here, so M3 at zero
        // is a flat spread and turning it up thins the edges.
        routing.out[d] = 0.30;
    }

    let mut eg = [eg_off(); NOPS];
    // One shape across the spread — a supersaw's operators are one voice, not
    // seven, so anything per-operator here would read as a chorus artefact.
    let body = EgParams::adsr(0.008, 0.30, 0.85, 0.35);
    for slot in eg.iter_mut().take(7) {
        *slot = body;
    }
    eg[7] = EgParams::adsr(0.004, 0.45, 0.70, 0.30);

    // 39 slots: six each for detune, pan and taper, then seven each for the
    // modulation, its rolloff and the phase spread. This is the patch
    // `N_MATRIX_SLOTS` was raised for.
    let mut slots = Vec::with_capacity(39);

    const DETUNE: [DestId; 7] = [
        DestId::Ratio0,
        DestId::Ratio1,
        DestId::Ratio2,
        DestId::Ratio3,
        DestId::Ratio4,
        DestId::Ratio5,
        DestId::Ratio6,
    ];
    const PANS: [DestId; 7] = [
        DestId::Pan0,
        DestId::Pan1,
        DestId::Pan2,
        DestId::Pan3,
        DestId::Pan4,
        DestId::Pan5,
        DestId::Pan6,
    ];
    const SENDS: [DestId; 7] = [
        DestId::Out0,
        DestId::Out1,
        DestId::Out2,
        DestId::Out3,
        DestId::Out4,
        DestId::Out5,
        DestId::Out6,
    ];
    const PM: [DestId; 7] = [
        DestId::Pm07,
        DestId::Pm17,
        DestId::Pm27,
        DestId::Pm37,
        DestId::Pm47,
        DestId::Pm57,
        DestId::Pm67,
    ];
    const SPREAD: [DestId; 7] = [
        DestId::Spread0,
        DestId::Spread1,
        DestId::Spread2,
        DestId::Spread3,
        DestId::Spread4,
        DestId::Spread5,
        DestId::Spread6,
    ];
    const DAMP: [DestId; 7] = [
        DestId::Damp0,
        DestId::Damp1,
        DestId::Damp2,
        DestId::Damp3,
        DestId::Damp4,
        DestId::Damp5,
        DestId::Damp6,
    ];

    for (d, k) in OFFSET.iter().enumerate() {
        if *k != 0.0 {
            // M1 — detune, in semitones. 15 cents per spread unit puts the
            // outer pair at +/-45 cents at full travel, which is the classic
            // supersaw width. The centre operator is deliberately unrouted:
            // detuning it would move the patch's pitch rather than widen it.
            slots.push(route(SourceId::Macro1, DETUNE[d], 0.15 * k));

            // M2 — width. Outer saws reach +/-0.9 rather than hard left/right,
            // because a saw pinned fully to one side stops contributing to the
            // beating that makes the spread sound like one instrument.
            slots.push(route(SourceId::Macro2, PANS[d], 0.30 * k));

            // M3 — taper, subtracted from the send. Proportional to distance,
            // so the edges thin first and the centre never moves.
            slots.push(route(SourceId::Macro3, SENDS[d], -0.06 * k.abs()));
        }
        // M4 — how much op7 modulates this saw. Full depth: the cubic taper
        // means 0.70 cooks to 0.34 turns, and against a carrier that is
        // already broadband that was not enough to hear. 1.0 cooks to a full
        // turn and takes the centroid from 4302 Hz to 8212 Hz.
        slots.push(route(SourceId::Macro4, PM[d], 1.00));
        // M5 — rolloff on what arrives, in octaves against the 20 kHz default.
        // This is the knob for how much of op7's contribution survives, which
        // is a different question from how much is sent.
        slots.push(route(SourceId::Macro5, DAMP[d], -0.70));
        // M6 — phase decorrelation, from coherent (0) back out to the hash (1).
        //
        // The axis this patch was pinned to one end of. Coherent is loud and
        // full but cannot be widened, because M2 has seven identical signals to
        // pan and identical signals stay centred. Decorrelated is wide but
        // comb-filtered and ~5 dB quieter. Neither end is right for every
        // sound, which is exactly vxn-2's argument for `StackParams::phase`
        // being a continuous knob rather than a mode.
        //
        // Takes effect on the **next** note, not on notes already sounding.
        slots.push(route(SourceId::Macro6, SPREAD[d], 1.00));
    }

    Patch {
        name: "supersaw",
        ops,
        routing,
        matrix: matrix(&slots),
        eg,
        // Measured like the rest: a six-note chord at velocity 100 near
        // -6 dBFS in the **authored** state, which for this patch is the
        // loudest state it has.
        //
        // Seven phase-coherent saws sum to seven times one saw; detuning them
        // decorrelates the sum toward root-seven, so M1 at full travel is about
        // 6 dB quieter than M1 at zero. That is the physics of the spread and
        // not something to compensate for — but it does mean the trim has to be
        // set from the coherent end or ordinary playing rides the limiter.
        gain: 0.140,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_patch_exists_and_is_named() {
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
        assert_eq!(
            patch(4).routing.density(),
            NOPS * NOPS,
            "web should be full"
        );
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
