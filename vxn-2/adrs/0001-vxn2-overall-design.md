# ADR 0001 — VXN2 overall design

- **Status:** Accepted
- **Date:** 2026-06-05
- **Scope:** Architecture of VXN2 — an operator-based, voice-stacked FM
  synthesizer shipped as a Rust CLAP plugin.

## Context

VXN1 is a subtractive polysynth in the Jupiter-8 idiom. VXN2 is a *separate*
instrument in the same Vulpus Labs line, targeting a different sound world:
operator-based FM in the DX7/Montage lineage, extended with first-class
voice-stacking suitable for hypersaw-style supersaws *without* wavetable
storage. The two synths share infrastructure (CLAP shell, preset system,
faceplate UI idiom) and a build/release pipeline, but VXN2 is its own DSP
kernel, parameter table, and patch format.

Constraints carried over from VXN1:

- Real-time process callback: allocation-free, predictable, no panics across
  the FFI boundary.
- Permissive licensing only (MIT / Apache-2.0). CLAP via `clack`.
- macOS / Apple Silicon first, Windows/Linux not structurally precluded.
- Hardware-style faceplate UI (HTML via `webkit2gtk` / WebView2), not generic
  host knobs.

New constraints for VXN2:

- The DX7 lineage means a much larger parameter surface than VXN1 (~6× ops,
  most params repeating per op). The UI and parameter model must scale to
  this without becoming a paged "edit menu".
- Voice-stacking is not a side feature — it is the differentiator that makes
  the operator architecture competitive with hardware-cost wavetable
  hypersaws. Stacking mechanics must be designed in, not retrofitted.

## Decision

### 1. Operator-based engine, 6 operators per voice

Each voice contains 6 operators. Each operator is a phase accumulator + sine
generator + EG + per-op level + per-op feedback + key scaling. Operators are
combined per a *patch's* algorithm: a fixed graph of which ops modulate which
others, which ops are carriers (sum to output), and which op has a self-feedback
loop. This mirrors the DX7/TX802 model exactly, both for sound-design familiarity
and because the algorithm space (32 standard graphs) is well-understood
territory.

**Oscillator core**: Bhaskara+Moser sine approximation (5 mul + 2 abs + 2 add,
branch-free, ~−59 dB THD). Q32 fixed-point phase accumulator (zero drift, free
wraparound). Float past the phase boundary: filters, envelopes, mixing stay
f32. See `vxn-2/README.md` for premises.

**Extension over DX7**: per-op feedback (DX7 had feedback on one op per algo;
VXN2 allows any op a feedback amount, costing modest extra DSP for the
flexibility). Cross-op PM stays algorithm-bound — arbitrary PM is the mod
matrix's job, not the algo graph's.

### 2. 32 DX7 algorithms as the patch topology library

The algorithm enum is the canonical DX7 set (1–32), each with a fixed
modulator→carrier graph and one designated feedback op. We do not allow
free-form algorithm editing in v1 — the 32 cover the useful topology space,
and a free editor is its own UX problem (loop detection, latency-cost
visualisation, presets that don't fit other algos). A free editor is a
post-v1 candidate.

The current algorithm is a single integer param (`algo`, 1..32). Picker UI
is a visual grid of 32 mini-diagrams (overlay panel), not a dropdown.

### 3. Voice stacking as first-class

A "voice" in VXN2 is a *stack*: when a note plays, up to N concurrent operator
voices are instantiated (N = stack density, 1..8). Each stacked instance gets
its own values for:

- `voice_idx` — integer 0..N−1 (discrete per-stack-instance position).
- `voice_spread` — float −1..+1 (symmetric position; 0 = centre instance,
  ±1 = outer instances).
- `voice_rand` — float [0,1), fixed at note-on per-instance (humanisation).

These three are first-class **mod matrix sources**, alongside LFOs, envelopes,
mod wheel etc. Stacking macros (density, detune, pan-spread, phase-spread,
distribution mode) are convenience knobs that *write into* matrix-style
routings; advanced patches can override or augment via the matrix.

CPU cost: stacking N voices ≈ N× single-voice DSP. A 16-note poly limit with
density 4 = 64 op-voices in flight. SIMD lane-packing (per VXN1 lessons —
SoA, no branch in hot path) is mandatory to make this viable.

### 4. Two LFOs — one global, one per-voice

- **LFO1 — global**: free-running OR host-sync (BPM), shared phase across all
  voices. Sin / Tri / Saw± / Pulse / S&H. Use cases: chorus-y detune locked
  across the patch, song-synced filter sweeps.
- **LFO2 — per-voice**: retriggered on note-on (or free-running, per-voice
  toggle), with delay + fade time (matches VXN1 LFO 1 idiom). Use cases:
  vibrato that breathes per-note, decorrelated tremolo across a stack.

LFO2 per-voice is what makes stacked patches feel alive — independent phase
per stack instance + `voice_rand → lfo2 phase` modulation = the supersaw
"shimmer" without identical-twin artefacts.

### 5. Two extra envelopes beyond per-op EGs

Per-op EG is a 4-rate / 4-level DX7-shape envelope, exclusively wired to that
op's level (the FM index for modulators, the amp env for carriers). Beyond
those, two patch-wide envelopes:

- **Pitch EG** — 4-rate / 4-level, signed (positive and negative pitch
  excursions). Routes to global pitch by default; matrix-routable elsewhere.
- **Mod Env** — ADSR with shape (lin/exp). General-purpose source for the
  matrix. Replaces the unstructured "extra env" gap DX7 patches sometimes
  needed scaling tricks to fake.

### 6. Mod matrix as the central source/dest engine

A fixed-slot matrix (16 slots in v1, expandable). Each slot:
`(source, destination, depth, curve, condition)`.

- Sources: LFO1, LFO2, Pitch EG, Mod Env, Mod Wheel, Aftertouch (channel),
  Velocity, Key (note number), `voice_idx`, `voice_spread`, `voice_rand`.
- Destinations: per-op ratio / level / detune / pan; global pitch; FX wet
  amounts; LFO rates / phase; per-op feedback; stacking macros.
- Curve: lin / exp / log / bipolar.
- Condition: optional gate by another source (e.g. "only when velocity > X")
  — v2 feature; v1 ships unconditional.

The matrix is the only mechanism for "this knob moves that thing dynamically".
Dedicated wiring (mod wheel directly to cutoff) is **not** added — every such
route is a matrix slot, with the panel UI hooking into specific slot indices
when one-knob macros are exposed (per VXN1 pattern: macros write into hidden
parameter cells).

### 7. Effects — clean delay + FDN reverb, no character

Serial chain: synth → delay → reverb → master. Both effects "clean": no
character emulation, no tape, no plate. The patch's character lives in the FM
synthesis; FX add space, not colour.

- **Delay**: stereo, BPM-syncable (subdivisions match VXN1 LFO sync table),
  feedback, mix. Ping-pong toggle.
- **Reverb**: FDN topology (chosen over Schroeder for tunability; over
  convolution for zero IR data + CPU predictability; over plate/spring for
  the "clean" requirement). Size, decay, damping, mix.

Toggle each effect on/off via header switch (VXN1 idiom: header colour change
+ body dim).

### 8. Voicing modes — Whole / Layer / Split

Inherited from VXN1's two-layer model but extended:

- **Whole**: one patch, whole keyboard.
- **Layer**: same as Whole but the second layer can have an independent set
  of per-op parameters (lower octave bass split, e.g.). Two parallel patches,
  same trigger.
- **Split**: keyboard split at a configurable point, upper and lower play
  different patches.

In v1, "Layer" and "Split" share infrastructure with the per-op parameter
layer-doubling already proven in VXN1. The Voice panel surfaces this
identically: Mode selector + (when not Whole) an Upper/Lower edit toggle
contextually appearing inside the op-detail panel.

### 9. UI: faceplate, three rows, op-tab layout

Inherits VXN1's faceplate idiom (1024px-wide hardware-style panel, palette
matches `vxn-ui-vizia` exports). Three rows below the preset bar:

- **Op row** — algorithm block (left, with always-visible diagram and op-num
  picker) + op-tab strip + per-op detail panel. Selecting an op (via tab or
  by clicking its node in the diagram) populates the detail panel: tuning
  (ratio/fixed/fine/detune), envelope graph (drag points), key-scaling level
  graph (BP + L/R depth + curve), sensitivity (vel/AMS/key-rate), output
  (out/pan/per-op fb).
- **Global mod row** — LFO1 | LFO2 | Pitch EG (graph) | Mod Env (ADSR).
- **Performance row** — Voice mode | Stacking macros | Delay | Reverb |
  Master.

The mod matrix lives in a button-triggered overlay (from the preset bar:
`Mod Matrix · N`). Not on the main faceplate — its summary is mostly
read-only and the per-source UIs (LFO panel etc.) already show their own
depths.

### 10. UI widget reuse

Wave-shape rotary, fader, button-group, segmented switch, preset bar with
browser overlay — all ported from VXN1's HTML faceplate (`vxn-ui-web`). The
ported wave-knob uses the same `WAVE_GLYPHS` polyline table and 270° glyph
arc layout. New widgets introduced for VXN2:

- **EG graph editor** — 4-point envelope with draggable handles, sustain
  segment dashed past the L3 point. Shared between per-op EG, Pitch EG, Mod
  Env (same widget, different state binding).
- **KS level graph** — breakpoint + L/R curve editor. Three drag handles
  (BP, L endpoint, R endpoint), curve-type selector beside it. Shared across
  all six op KS configurations.
- **Algorithm diagram + picker** — clickable diagram of current algorithm
  (carriers orange, modulators teal, feedback as labelled arc), + overlay
  grid of all 32 mini-diagrams.

### 11. Parameter model

Per-op parameters are repeated 6× with a per-op index suffix
(`op1_ratio`, `op2_ratio`, …). Globals are named directly. Total parameter
count ~155 (matches DX7 ballpark, slightly higher with per-op FB + extra
envelopes). See `vxn-2/PARAMETERS.md` for the full enumeration.

The CLAP `params` table follows VXN1's ADR 0007 pattern: stable IDs are *not*
a binding constraint (per memory: `vxn1-id-stability-dropped`). The patch
format is name-keyed TOML (per ADR 0005 in VXN1's tree).

### 12. Algorithm editor — NOT in v1

A user-editable algorithm (drag op nodes to wire them) is explicitly out of
v1. Justification: 32 algorithms cover most sound-design needs, a graph
editor with cycle detection / feedback budget visualisation is a substantial
UI problem in its own right, and existing patches won't survive arbitrary
re-routes. Revisit post-v1 if user demand surfaces.

## Consequences

- The DSP hot path will be dominated by op-voice processing: 16 poly × 8
  stack × 6 ops = up to 768 op-instances per sample. Vector packing is
  non-optional; per-voice scalar fallback for sustain/release tails only.
- The mod matrix is the single most CPU-heavy non-DSP per-block work. Source
  evaluation is per-block (control rate); destination application is
  per-sample for smoothing-critical params (pitch), per-block for others.
- The preset format diverges from VXN1's (different parameter set). VXN1 and
  VXN2 share no preset bytes, only the file-format conventions.
- Two synths in the same monorepo: `vxn-1/` and `vxn-2/` are sibling Cargo
  workspaces. Shared crates (CLAP shell helpers, preset I/O primitives,
  faceplate JS bridge) are candidates for a third top-level workspace once
  divergence stabilises — not yet, premature consolidation risk.
- The UI mockup at `vxn-2/ui-mockup/index.html` is the *living layout
  reference* until the production HTML faceplate ships. Treat it as
  source-of-truth for visual/interaction decisions during the kernel build.
