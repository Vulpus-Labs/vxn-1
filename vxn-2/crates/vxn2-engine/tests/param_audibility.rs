//! Ticket 0069 — param audibility sweep.
//!
//! Every CLAP param, swept min → max under a patch where it *should* matter,
//! must change the rendered output. This is the mechanical guard for the E006
//! review's headline pattern — `lfo1-depth`, `AmpSens`, `PitchSmoother` were
//! all structurally complete but functionally inert; three of the five review
//! bugs would have been caught here.
//!
//! Mechanism: render the patch twice — param at min, param at max — under an
//! identical, deterministic context (two fresh engines, same note script, same
//! fixed RNG seeds). Capture the full L+R buffer across attack → sustain →
//! release and compare. A param that does *something* makes the two buffers
//! differ beyond a relative epsilon.
//!
//! The hard part is per-param context: most params are inaudible under a bare
//! patch (a modulator's level when its algo doesn't route it; delay-feedback
//! with delay-mix at 0; `op{n}-fixed-hz` while `ratio-mode = Ratio`). A rich
//! base context (algo 32 = six parallel carriers, eight active matrix routes,
//! FX up, EG that moves) covers most; [`context_override`] fixes the rest.
//!
//! Exclusions ([`EXCLUDED`]) are documented decisions, not silent skips —
//! each carries a reason. The list is meant to stay short.
//!
//! Known-inert UI surface that is *not* a param and so doesn't trip this test:
//! the deferred matrix destinations `Lfo2Phase`, `Lfo1Rate`, `Lfo2Rate`,
//! `StackDetune`, `StackSpread` are dest-enum entries, wired E008 (0091-0093);
//! the matrix *routing* fields (source/dest/active) are non-CLAP patch state.

use vxn2_engine::engine::Engine;
use vxn2_engine::matrix::{DestId, SourceId};
use vxn2_engine::params::{PARAMS, TOTAL_PARAMS, id_of};
use vxn2_engine::shared::{MatrixRowRaw, SharedParams};

const SR: f32 = 48_000.0;
const BLK: usize = 64;

/// Default note script: gate at sample 0, hold `sustain` blocks, release, then
/// render `release` more blocks so attack/decay/sustain *and* release-stage
/// params land in the captured window.
#[derive(Clone, Copy)]
struct Capture {
    note: u8,
    velocity: u8,
    sustain_blocks: usize,
    release_blocks: usize,
}

impl Default for Capture {
    fn default() -> Self {
        Self { note: 72, velocity: 80, sustain_blocks: 28, release_blocks: 22 }
    }
}

/// Eight active matrix routes seeded into the base context so the eight
/// CLAP-automatable `mtx{n}-depth` params have something to scale. Each routes
/// a (mostly time-varying) source onto an audible destination; sweeping the
/// depth from −1 to +1 inverts the modulation, so the two renders differ.
const BASE_ROUTES: [(SourceId, DestId); 8] = [
    (SourceId::ModEnv, DestId::Op1Level),
    (SourceId::Lfo1, DestId::GlobalPitch),
    (SourceId::PitchEg, DestId::Op2Level),
    (SourceId::Lfo2, DestId::Op3Pitch),
    (SourceId::Velocity, DestId::Op4Level),
    (SourceId::ModEnv, DestId::Op5Level),
    (SourceId::Lfo1, DestId::Op6Pitch),
    (SourceId::Lfo1, DestId::DelayMix),
];

/// Build the rich base patch: six parallel carriers (algo 32) so every op's
/// params sound directly, FX engaged, EG/mod-env contours that move, and the
/// eight matrix routes active at a moderate depth.
fn base_context(s: &SharedParams) {
    let set = |name: &str, v: f32| s.set(id_of(name).unwrap(), v);

    set("algo", 32.0); // six independent carriers → every op audible
    set("feedback", 2.0); // structural FB op carries a non-trivial timbre

    // Pin every op to a full-level carrier with a high EG sustain so its per-op
    // params (level, ratio, detune, pan, KS, EG, and any route onto its
    // level/pitch) move the mix directly. Under the DX7 log level curve
    // (E026/0123) the default patch's modulator ops sustain at ≈ −37 dB, which
    // dropped op2 (and parts of op6, plus the PitchEg→Op2Level / Lfo1→Op6Pitch
    // routes and `feedback`) below the audibility floor even though their wiring
    // is intact. Restoring a fair, loud carrier context fixes that without
    // weakening AUDIBLE_EPS. The per-param `-eg-` override still re-shapes the
    // op under test, so this only sets the *baseline* sustain.
    for op in 1..=6 {
        set(&format!("op{op}-level"), 99.0);
        set(&format!("op{op}-eg-l3"), 90.0);
    }

    // FX fully in-circuit so their params matter.
    set("delay-on", 1.0);
    set("delay-sync", 0.0); // free-running so delay-time is in ms
    set("delay-mix", 0.5);
    set("delay-feedback", 0.5);
    set("reverb-on", 1.0);
    set("reverb-mix", 0.4);

    // A mod-env that actually moves (fast attack, audible decay to a mid
    // sustain) so its contour — and anything routing it — is non-flat.
    set("mod-env-a", 2.0);
    set("mod-env-d", 200.0);
    set("mod-env-s", 0.5);
    set("mod-env-r", 200.0);

    // LFO2 immediately active — its default 180 ms delay + 320 ms fade keeps
    // it silent through a short window, which would make every LFO2 param (and
    // the Lfo2→Op3Pitch route) inert. The lfo2-delay / lfo2-fade sweeps
    // re-introduce those times explicitly.
    set("lfo2-delay", 0.0);
    set("lfo2-fade", 0.0);

    // Pitch EG with a non-zero target so peg rate/level params bend pitch.
    set("peg-l1", 40.0);
    set("peg-depth", 1.0);

    // Stack spread so per-lane sources (Lfo2, VoiceSpread) differ across lanes.
    set("stack-density", 4.0);
    set("stack-detune", 10.0);

    for (slot, (src, dst)) in BASE_ROUTES.iter().enumerate() {
        s.set_matrix_row_raw(
            slot,
            MatrixRowRaw {
                source: *src as u8,
                dest: *dst as u8,
                curve: 0,
                active: true,
                depth: 0.5,
            },
        );
    }
}

/// Per-param context tweak + capture override. Returns the capture script;
/// mutates `s` with any extra patch state the param needs to be audible.
///
/// Implemented as a table of `(matcher, action)` pairs so each param's context
/// is a named, greppable entry. Multiple rows may match the same param name —
/// they execute in order, exactly as the original `if`-chain did. A param with
/// no matching row uses `Capture::default()` unchanged.
fn context_override(name: &str, s: &SharedParams) -> Capture {
    type Action = fn(&str, &SharedParams, &mut Capture);

    /// Each row: a predicate over the param name, and an action that tweak `s`
    /// and/or `cap`. Rows are checked in order; all matching rows execute.
    static TABLE: &[(fn(&str) -> bool, Action)] = &[
        // ── Fixed-Hz tuning ────────────────────────────────────────────────
        // Per-op fixed-hz is inert unless the op is in Fixed tuning mode.
        (
            |n| n.ends_with("-fixed-hz") && parse_op(n).is_some(),
            |n, s, _cap| {
                let op = parse_op(n).unwrap();
                s.set(id_of(&format!("op{op}-ratio-mode")).unwrap(), 1.0); // Fixed
            },
        ),
        // ── EG stage params (per-op) ────────────────────────────────────────
        // Under a near-instant EG most stage transitions never occupy the
        // window. Give the op a slow zig-zag contour and a long capture window.
        (
            |n| parse_op(n).is_some() && n.contains("-eg-"),
            |n, s, cap| {
                let op = parse_op(n).unwrap();
                // The base matrix routes modulate several op levels; silence
                // them so the op's own amplitude envelope is the only mover.
                deactivate_base_routes(s);
                // Delay + reverb are common-mode here: their wet tails are
                // identical in the min and max renders, so they add only to the
                // rel-diff denominator (|x|+|y|), never the numerator (|x-y|).
                // That dilutes a subtle EG-stage sweep — an `eg-l3` plateau
                // difference sits an order of magnitude below the FX-inflated
                // energy. Take them out of circuit so the sweep is measured on
                // the dry op alone.
                s.set(id_of("delay-on").unwrap(), 0.0);
                s.set(id_of("reverb-on").unwrap(), 0.0);
                // Algo 32 is six parallel carriers. The other five ring at full
                // level through sustain *and* release, so a release-stage sweep
                // (r4/l3/l4) on this op moves at most ~1/6 of the mix — diluted
                // below AUDIBLE_EPS. Silence them so this op is the sole carrier
                // and its whole envelope, release tail included, drives the mix.
                for other in 1..=6 {
                    if other != op {
                        s.set(id_of(&format!("op{other}-level")).unwrap(), 0.0);
                    }
                }
                let o = |suf: &str| format!("op{op}-{suf}");
                // Rates fast enough that every stage is reached inside the
                // window; levels a big zig-zag so each stage is distinct.
                s.set(id_of(&o("eg-r1")).unwrap(), 80.0);
                s.set(id_of(&o("eg-r2")).unwrap(), 72.0);
                s.set(id_of(&o("eg-r3")).unwrap(), 64.0);
                s.set(id_of(&o("eg-r4")).unwrap(), 40.0);
                s.set(id_of(&o("eg-l1")).unwrap(), 99.0);
                s.set(id_of(&o("eg-l2")).unwrap(), 8.0);
                s.set(id_of(&o("eg-l3")).unwrap(), 85.0);
                s.set(id_of(&o("eg-l4")).unwrap(), 0.0);
                // Hold well past the L2→L3 rise (rate 64, an 8→85 climb) so
                // *every* op fully reaches and dwells at the stage-3 plateau
                // before note-off. At the old 55-block sustain the note-off
                // preempted stage 3 — op5 skipped it outright and op1 only
                // grazed it, so their `eg-l3` / `eg-r3` sweeps read inaudible
                // (rel-diff 0 / 8e-5) even though the wiring is intact. The
                // longer hold keeps the early-stage (l1/l2/r1/r2) and release
                // (l4/r4) sweeps audible too — they sit orders of magnitude
                // above the floor.
                cap.sustain_blocks = 130;
                // Long enough that the moderate R4 release actually reaches L4,
                // so an `eg-l4` sweep (0 vs 99) separates the two release tails
                // instead of both freezing partway.
                cap.release_blocks = 130;
            },
        ),
        // ── KS: pin op ratio for high-note tests ────────────────────────────
        // KS tests play note 96. The default patch gives op2 a ratio-14
        // modulator, which at note 96 runs at ~29 kHz — above Nyquist. With
        // the 0073 Nyquist fade that op is muted, masking the KS effect.
        // Pin the op to ratio 1 so the fundamental stays in-band.
        (
            |n| parse_op(n).is_some()
                && (n.ends_with("-ks-r-depth") || n.ends_with("-ks-rate")),
            |n, s, _cap| {
                let op = parse_op(n).unwrap();
                s.set(id_of(&format!("op{op}-num")).unwrap(), 1.0);
                s.set(id_of(&format!("op{op}-denom")).unwrap(), 1.0);
            },
        ),
        // ── KS right-side depth ─────────────────────────────────────────────
        // R side hot, L side silent; play high (note 96) for strong R scaling.
        (
            |n| parse_op(n).is_some() && n.ends_with("-ks-r-depth"),
            |n, s, cap| {
                let op = parse_op(n).unwrap();
                s.set(id_of(&format!("op{op}-ks-l-depth")).unwrap(), 0.0);
                s.set(id_of(&format!("op{op}-ks-break-pt")).unwrap(), 48.0);
                cap.note = 96; // four octaves above the break → strong R scaling
            },
        ),
        // ── KS left-side depth ─────────────────────────────────────────────
        // L side hot, R side silent; play low (note 36) for strong L scaling.
        (
            |n| parse_op(n).is_some() && n.ends_with("-ks-l-depth"),
            |n, s, cap| {
                let op = parse_op(n).unwrap();
                s.set(id_of(&format!("op{op}-ks-r-depth")).unwrap(), 0.0);
                s.set(id_of(&format!("op{op}-ks-break-pt")).unwrap(), 108.0);
                cap.note = 36; // well below the break → strong L scaling
            },
        ),
        // ── KS break point ──────────────────────────────────────────────────
        // R side hot, L side cold: sliding the break across the note flips
        // between full R-scaling and none.
        (
            |n| parse_op(n).is_some() && n.ends_with("-ks-break-pt"),
            |n, s, cap| {
                let op = parse_op(n).unwrap();
                s.set(id_of(&format!("op{op}-ks-l-depth")).unwrap(), 0.0);
                s.set(id_of(&format!("op{op}-ks-r-depth")).unwrap(), 99.0);
                cap.note = 72;
            },
        ),
        // ── KS rate scaling ─────────────────────────────────────────────────
        // Rate scaling shortens EG times above A3 (note 57). Give the op a
        // slow release on a high note and watch the release window.
        (
            |n| parse_op(n).is_some() && n.ends_with("-ks-rate"),
            |n, s, cap| {
                let op = parse_op(n).unwrap();
                s.set(id_of(&format!("op{op}-eg-l3")).unwrap(), 85.0);
                s.set(id_of(&format!("op{op}-eg-r4")).unwrap(), 18.0);
                cap.note = 96;
                cap.sustain_blocks = 40;
                cap.release_blocks = 180;
            },
        ),
        // ── Pitch EG (global) ───────────────────────────────────────────────
        // A flat pitch envelope makes every peg rate/level inert. Give it a
        // moving zig-zag contour spanning the capture window.
        (
            |n| n.starts_with("peg-"),
            |_n, s, cap| {
                s.set(id_of("peg-r1").unwrap(), 80.0);
                s.set(id_of("peg-r2").unwrap(), 72.0);
                s.set(id_of("peg-r3").unwrap(), 64.0);
                s.set(id_of("peg-r4").unwrap(), 40.0);
                s.set(id_of("peg-l1").unwrap(), 70.0);
                s.set(id_of("peg-l2").unwrap(), -70.0);
                s.set(id_of("peg-l3").unwrap(), 60.0);
                s.set(id_of("peg-l4").unwrap(), 0.0);
                s.set(id_of("peg-depth").unwrap(), 1.0);
                cap.sustain_blocks = 55;
                cap.release_blocks = 60;
            },
        ),
        // ── LFO2 delay / fade ───────────────────────────────────────────────
        // LFO2 is routed onto Op3 pitch in the base patch. The delay/fade
        // sweeps need a long window to show the silent-vs-active extremes.
        (
            |n| n == "lfo2-delay" || n == "lfo2-fade",
            |_n, _s, cap| {
                cap.sustain_blocks = 150;
            },
        ),
        // ── Stack spread ────────────────────────────────────────────────────
        // `stack-spread` scales the VoiceSpread matrix source. It does nothing
        // without a route that reads VoiceSpread, so wire one and widen the
        // stack so lanes diverge visibly.
        (
            |n| n == "stack-spread",
            |_n, s, cap| {
                s.set(id_of("stack-density").unwrap(), 8.0);
                s.set(id_of("stack-detune").unwrap(), 40.0);
                s.set_matrix_row_raw(
                    0,
                    MatrixRowRaw {
                        source: SourceId::VoiceSpread as u8,
                        dest: DestId::Op1Pitch as u8,
                        curve: 0,
                        active: true,
                        depth: 1.0,
                    },
                );
                cap.sustain_blocks = 110;
            },
        ),
        // ── Filter params (non-enable) ──────────────────────────────────────
        // Filter is off by default; turn it on and drive cutoff + resonance so
        // any filter param sweep reshapes the spectrum audibly.
        (
            |n| n.starts_with("filter-") && n != "filter-enable",
            |_n, s, _cap| {
                s.set(id_of("filter-enable").unwrap(), 1.0);
                s.set(id_of("filter-cutoff").unwrap(), 800.0);
                s.set(id_of("filter-resonance").unwrap(), 0.6);
            },
        ),
        // ── filter-enable itself ────────────────────────────────────────────
        // Lower the cutoff + raise resonance so the on ≠ off renders differ
        // audibly (not just the bypass switch).
        (
            |n| n == "filter-enable",
            |_n, s, _cap| {
                s.set(id_of("filter-cutoff").unwrap(), 500.0);
                s.set(id_of("filter-resonance").unwrap(), 0.6);
            },
        ),
        // ── Delay (all delay-* params) ──────────────────────────────────────
        // The default 375 ms tap never returns inside a short window. Shorten
        // the tap, raise mix/feedback, render long enough for several echoes.
        (
            |n| n.starts_with("delay-"),
            |n, s, cap| {
                s.set(id_of("delay-on").unwrap(), 1.0);
                s.set(id_of("delay-mix").unwrap(), 0.7);
                s.set(id_of("delay-feedback").unwrap(), 0.6);
                if n != "delay-time" {
                    s.set(id_of("delay-sync").unwrap(), 0.0);
                    s.set(id_of("delay-time").unwrap(), 70.0);
                }
                // Ping-pong only diverges from a normal delay on a stereo
                // input. Pan two ops hard L/R so the delay sees L ≠ R.
                if n == "delay-pingpong" {
                    s.set(id_of("op1-pan").unwrap(), -1.0);
                    s.set(id_of("op2-pan").unwrap(), 1.0);
                }
                cap.sustain_blocks = 36;
                cap.release_blocks = 170;
            },
        ),
        // ── Reverb tails ────────────────────────────────────────────────────
        (
            |n| n.starts_with("reverb-"),
            |_n, s, cap| {
                s.set(id_of("reverb-on").unwrap(), 1.0);
                s.set(id_of("reverb-mix").unwrap(), 0.6);
                cap.release_blocks = 160;
            },
        ),
        // ── Phaser (E025) ───────────────────────────────────────────────────
        // Off by default → every phaser param is inert unless the stage is on.
        // Drive depth/mix/feedback and render long enough for the slow LFO
        // (rate floor 0.05 Hz) to walk the notches so even a rate sweep
        // diverges across the window.
        (
            |n| n.starts_with("phaser-"),
            |n, s, cap| {
                s.set(id_of("phaser-on").unwrap(), 1.0);
                if n != "phaser-mix" {
                    s.set(id_of("phaser-mix").unwrap(), 0.7);
                }
                if n != "phaser-depth" {
                    s.set(id_of("phaser-depth").unwrap(), 0.8);
                }
                if n != "phaser-feedback" {
                    s.set(id_of("phaser-feedback").unwrap(), 0.6);
                }
                cap.sustain_blocks = 110;
                cap.release_blocks = 60;
            },
        ),
        // ── Dynamics (E028) ─────────────────────────────────────────────────
        // Off by default → every dyn-* knob except `dyn-on` is inert without
        // the block engaged. Turn it on, drive a hot threshold so the comp is
        // actually working, and keep mix at full wet so makeup / drive land on
        // the bus. `dyn-attack` / `dyn-release` only diverge when the envelope
        // is tracking transients — the base velocity-120 note-on covers that.
        (
            |n| n.starts_with("dyn-"),
            |n, s, _cap| {
                if n != "dyn-on" {
                    s.set(id_of("dyn-on").unwrap(), 1.0);
                }
                if n != "dyn-mix" {
                    s.set(id_of("dyn-mix").unwrap(), 1.0);
                }
                // Hot threshold + non-1 ratio so a sweep of either, or of the
                // time constants, audibly reshapes the bus.
                if n != "dyn-threshold" {
                    s.set(id_of("dyn-threshold").unwrap(), -30.0);
                }
                if n != "dyn-ratio" {
                    s.set(id_of("dyn-ratio").unwrap(), 8.0);
                }
            },
        ),
        // ── Mod-env params ──────────────────────────────────────────────────
        // The mod-env is routed in the base context; give it a long window so
        // its contour is fully visible.
        (
            |n| n.starts_with("mod-env-"),
            |_n, _s, cap| {
                cap.sustain_blocks = 150;
            },
        ),
    ];

    let mut cap = Capture::default();
    for (matches, action) in TABLE {
        if matches(name) {
            action(name, s, &mut cap);
        }
    }
    cap
}

/// Clear the eight base matrix routes (set inactive, depth 0). Used by the EG
/// sweeps so the routes' identical level modulation doesn't dilute the EG
/// change under test.
fn deactivate_base_routes(s: &SharedParams) {
    for slot in 0..8 {
        s.set_matrix_row_raw(slot, MatrixRowRaw::default());
    }
}

fn parse_op(name: &str) -> Option<u8> {
    name.strip_prefix("op").and_then(|r| r.as_bytes().first()).and_then(|b| {
        let d = b.wrapping_sub(b'0');
        (1..=6).contains(&d).then_some(d)
    })
}

/// Params deliberately not swept, each with a reason. Keep this short.
const EXCLUDED: &[(&str, &str)] = &[
    ("assign-mode", "Poly vs Solo only diverges with overlapping notes; a \
        single-note sweep renders identically — needs a note-overlap script."),
    ("legato", "only affects retriggering when notes overlap with glide; \
        inert under a single gated note."),
    ("glide-time", "only audible across a legato note change; no pitch glide \
        with one note."),
    ("filter-cutoff-tuned", "UI-only cutoff display mode (Hz vs note-tuned \
        readout); the engine never reads it — the stored cutoff is always Hz \
        (shared.rs `read_filter`)."),
    // Per-op phase (0074) is cyclic: a fraction of one cycle, so min 0.0 and
    // max 1.0 are the *same* phase by construction → a min→max sweep is a no-op
    // by definition (the Q32 conversion wraps 1.0 → 0). The param is genuinely
    // audible at intermediate offsets (see stack.rs `per_op_phase_shifts_waveform`);
    // the extremes just happen to coincide. Excluded from the min→max guard only.
    ("op1-phase", "cyclic param: min 0.0 ≡ max 1.0 (one full cycle), so min→max is a no-op."),
    ("op2-phase", "cyclic param: min 0.0 ≡ max 1.0 (one full cycle), so min→max is a no-op."),
    ("op3-phase", "cyclic param: min 0.0 ≡ max 1.0 (one full cycle), so min→max is a no-op."),
    ("op4-phase", "cyclic param: min 0.0 ≡ max 1.0 (one full cycle), so min→max is a no-op."),
    ("op5-phase", "cyclic param: min 0.0 ≡ max 1.0 (one full cycle), so min→max is a no-op."),
    ("op6-phase", "cyclic param: min 0.0 ≡ max 1.0 (one full cycle), so min→max is a no-op."),
];

/// Render the configured patch through the capture script, returning the
/// interleaved L,R buffer. `scale` stretches the window (1 = default fast run;
/// the thorough variant uses a larger factor to catch slow-tail interactions).
fn render_capture(s: &SharedParams, cap: Capture, scale: usize) -> Vec<f32> {
    let mut e = Engine::new(SR, BLK);
    e.snapshot_params(s);
    e.note_on(cap.note, cap.velocity);
    let sustain = cap.sustain_blocks * scale;
    let release = cap.release_blocks * scale;
    let total = sustain + release;
    let mut buf = Vec::with_capacity(total * BLK * 2);
    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    for b in 0..total {
        if b == sustain {
            e.note_off(cap.note);
        }
        e.process_block(&mut l, &mut r);
        for i in 0..BLK {
            buf.push(l[i]);
            buf.push(r[i]);
        }
    }
    buf
}

/// Relative L1 difference between two equal-length buffers in `[0, 1]`.
fn rel_diff(a: &[f32], b: &[f32]) -> f64 {
    let mut num = 0.0_f64;
    let mut den = 0.0_f64;
    for (x, y) in a.iter().zip(b.iter()) {
        num += (*x as f64 - *y as f64).abs();
        den += (*x as f64).abs() + (*y as f64).abs();
    }
    if den < 1e-12 { 0.0 } else { num / den }
}

/// Sweep one param min → max under its context and return the relative diff.
fn audibility_of(id: usize, scale: usize) -> f64 {
    let desc = &PARAMS[id];

    let s_min = SharedParams::new();
    base_context(&s_min);
    let cap = context_override(desc.id, &s_min);
    s_min.set(id, desc.min);

    let s_max = SharedParams::new();
    base_context(&s_max);
    let _ = context_override(desc.id, &s_max);
    s_max.set(id, desc.max);

    let a = render_capture(&s_min, cap, scale);
    let b = render_capture(&s_max, cap, scale);
    rel_diff(&a, &b)
}

/// Sweep the whole param table at `scale` and panic listing every inert param.
fn run_sweep(scale: usize) {
    let excluded: std::collections::HashSet<&str> =
        EXCLUDED.iter().map(|(n, _)| *n).collect();

    let mut inert: Vec<(String, f64)> = Vec::new();
    for id in 0..TOTAL_PARAMS {
        let name = PARAMS[id].id;
        if excluded.contains(name) {
            continue;
        }
        let d = audibility_of(id, scale);
        if d < AUDIBLE_EPS {
            inert.push((name.to_string(), d));
        }
    }

    assert!(
        inert.is_empty(),
        "params swept min→max produced no audible change (rel-diff < {AUDIBLE_EPS:.0e}); \
         each is either severed wiring or needs a context override / exclusion:\n{}",
        inert.iter().map(|(n, d)| format!("  {n}: rel-diff {d:.2e}")).collect::<Vec<_>>().join("\n")
    );
}

/// Audibility threshold: 0.01 % relative L1. The renders are deterministic
/// (fixed RNG seeds, identical note script), so a *severed* param produces a
/// bit-identical pair — rel-diff exactly 0. A *wired* param always moves the
/// output by far more: the smallest real effect observed is a transient EG
/// attack/decay-stage level at ≈ 3e-4 (those stages occupy only a slice of the
/// window). 1e-4 sits an order of magnitude below the smallest real effect and
/// far above the 0 a severed param yields, so it cleanly separates the two
/// without false-failing on genuinely-subtle-but-wired params.
const AUDIBLE_EPS: f64 = 1e-4;

/// Default fast run: short windows, full table (~9 s). The mechanical guard
/// the E006 review wanted — a new param with no audibility context fails here
/// rather than passing silently.
///
/// Verified to have teeth (acceptance, ticket 0069): temporarily forcing the
/// matrix depth projection to 0 in `engine.rs` (`depth = 0.0 * mtx_depths[s]`)
/// makes this fail, listing the eight `mtx{n}-depth` params, `stack-spread`,
/// and every LFO2 / mod-env param that reaches the output only through those
/// routes — then it passes again once restored.
#[test]
fn every_param_sweep_is_audible() {
    run_sweep(1);
}


/// Thorough variant: 3× longer windows so slow-tail interactions get more
/// settling time. `#[ignore]`d — run with `--ignored`. Same assertion; the
/// longer render is a stricter sanity check, not a different contract.
#[test]
#[ignore = "3× render windows; run manually with --ignored"]
fn every_param_sweep_is_audible_thorough() {
    run_sweep(3);
}
