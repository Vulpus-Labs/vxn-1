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
/// Returns `None` to fall through to [`Capture::default`].
fn context_override(name: &str, s: &SharedParams) -> Capture {
    let set = |n: &str, v: f32| s.set(id_of(n).unwrap(), v);
    let mut cap = Capture::default();

    // ── Per-op operator params ─────────────────────────────────────────────
    // Per-op fixed-hz is inert unless the op is in Fixed tuning mode.
    if let Some(op) = name.strip_suffix("-fixed-hz").and_then(|_| parse_op(name)) {
        set(&format!("op{op}-ratio-mode"), 1.0); // Fixed
    }

    // EG stage params: under the default near-instant EG most stage transitions
    // never occupy the window, and one carrier's amplitude is 1/6 of the mix,
    // so a mid-stage tweak barely moves the sum. Give the op a slow zig-zag
    // contour (every stage a big, distinct move) and a long window so any
    // single rate / level change reshapes its amplitude audibly.
    if let Some(op) = parse_op(name) {
        if name.contains("-eg-") {
            // The base matrix routes modulate several op levels (e.g.
            // PitchEg→Op2Level); that competing modulation is identical in both
            // renders and dilutes the EG change under test. Silence the routes
            // so the op's own amplitude envelope is the only thing moving.
            deactivate_base_routes(s);
            // Rates fast enough that every stage is actually reached inside the
            // window (a too-slow env never enters stage 3, leaving r3/l3 inert);
            // levels a big zig-zag so each stage is a distinct, audible move.
            let o = |suf: &str| format!("op{op}-{suf}");
            set(&o("eg-r1"), 80.0);
            set(&o("eg-r2"), 72.0);
            set(&o("eg-r3"), 64.0);
            set(&o("eg-r4"), 40.0);
            set(&o("eg-l1"), 99.0);
            set(&o("eg-l2"), 8.0);
            set(&o("eg-l3"), 85.0);
            set(&o("eg-l4"), 0.0);
            cap.sustain_blocks = 55;
            cap.release_blocks = 60;
        }
    }

    // ── Keyboard scaling ───────────────────────────────────────────────────
    // Each KS mechanism scales one side of the break point; isolate the side
    // under test (zero the other), put the played note far onto that side, and
    // drive the depth hard so the level swing dominates the op.
    if let Some(op) = parse_op(name) {
        if name.ends_with("-ks-r-depth") {
            set(&format!("op{op}-ks-l-depth"), 0.0);
            set(&format!("op{op}-ks-break-pt"), 48.0);
            cap.note = 96; // four octaves above the break → strong R scaling
        }
        if name.ends_with("-ks-l-depth") {
            set(&format!("op{op}-ks-r-depth"), 0.0);
            set(&format!("op{op}-ks-break-pt"), 108.0);
            cap.note = 36; // well below the break → strong L scaling
        }
        if name.ends_with("-ks-break-pt") {
            // R side hot, L side cold: sliding the break across the note flips
            // between full R-scaling and none.
            set(&format!("op{op}-ks-l-depth"), 0.0);
            set(&format!("op{op}-ks-r-depth"), 99.0);
            cap.note = 72;
        }
        if name.ends_with("-ks-rate") {
            // Rate scaling shortens EG times about A3 (note 57). Give the op a
            // slow release on a high note and watch the release window.
            set(&format!("op{op}-eg-l3"), 85.0);
            set(&format!("op{op}-eg-r4"), 18.0);
            cap.note = 96;
            cap.sustain_blocks = 40;
            cap.release_blocks = 180;
        }
    }

    // ── Pitch EG (global) ──────────────────────────────────────────────────
    // Same idea as the op EG: a flat pitch envelope makes every peg rate/level
    // inert. Give it a moving zig-zag contour spanning the window.
    if name.starts_with("peg-") {
        set("peg-r1", 80.0);
        set("peg-r2", 72.0);
        set("peg-r3", 64.0);
        set("peg-r4", 40.0);
        set("peg-l1", 70.0);
        set("peg-l2", -70.0);
        set("peg-l3", 60.0);
        set("peg-l4", 0.0);
        set("peg-depth", 1.0);
        cap.sustain_blocks = 55;
        cap.release_blocks = 60;
    }

    // ── LFO2 (per-voice) ───────────────────────────────────────────────────
    // Routed onto Op3 pitch in the base patch. The delay/fade sweeps need a
    // long window to show the silent-vs-active extremes.
    if name == "lfo2-delay" || name == "lfo2-fade" {
        cap.sustain_blocks = 150;
    }

    // ── Stack spread ───────────────────────────────────────────────────────
    // `stack-spread` is not a direct DSP knob — it is the gain on the matrix's
    // `VoiceSpread` source (stack.rs `cook` / `eval_sources`). It does nothing
    // unless a route reads VoiceSpread, so wire one and widen the stack.
    if name == "stack-spread" {
        set("stack-density", 8.0);
        set("stack-detune", 40.0);
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
    }

    // ── Filter ─────────────────────────────────────────────────────────────
    if name.starts_with("filter-") && name != "filter-enable" {
        set("filter-enable", 1.0);
        set("filter-cutoff", 800.0);
        set("filter-resonance", 0.6);
    }
    // filter-enable itself: lower the cutoff + raise resonance so on ≠ off.
    if name == "filter-enable" {
        set("filter-cutoff", 500.0);
        set("filter-resonance", 0.6);
    }

    // ── Delay ──────────────────────────────────────────────────────────────
    // The default 375 ms tap never returns inside a short window. Shorten the
    // tap, raise mix/feedback, and render long enough for several echoes so
    // time / sync / feedback / ping-pong all surface.
    if name.starts_with("delay-") {
        set("delay-on", 1.0);
        set("delay-mix", 0.7);
        set("delay-feedback", 0.6);
        if name != "delay-time" {
            set("delay-sync", 0.0);
            set("delay-time", 70.0);
        }
        // Ping-pong only diverges from a normal delay on a *stereo* input —
        // with a centred (mono) sum the L/R feedback swap is a no-op. Pan two
        // ops hard L/R so the delay sees L ≠ R.
        if name == "delay-pingpong" {
            set("op1-pan", -1.0);
            set("op2-pan", 1.0);
        }
        cap.sustain_blocks = 36;
        cap.release_blocks = 170;
    }

    // ── Reverb tails ───────────────────────────────────────────────────────
    if name.starts_with("reverb-") {
        set("reverb-on", 1.0);
        set("reverb-mix", 0.6);
        cap.release_blocks = 160;
    }

    // ── Long mod-env evolution ─────────────────────────────────────────────
    if name.starts_with("mod-env-") {
        cap.sustain_blocks = 150;
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
