e# ADR 0001 — VXN1 overall design approach (first draft)

- **Status:** Accepted
- **Date:** 2026-05-24
- **Scope:** The architecture of the first draft of VXN1, a Jupiter-8-style
  analogue polysynth shipped as a Rust CLAP plugin (Vulpus Labs).

This is an overarching ADR. It records the design *approach* of the first
draft as a whole; later, narrower ADRs can supersede individual decisions
without rewriting this one.

## Context

We are building a polyphonic subtractive synthesizer ("VXN1") in the spirit of
the Roland Jupiter-8, distributed as an audio plugin. Constraints and goals:

- **Real-time audio**: the process callback must be allocation-free and
  predictable; no panics across the FFI boundary.
- **Permissive licensing**: the shipping artifact must be free of copyleft
  obligations so it can be distributed under MIT/Apache terms.
- **DSP heritage**: proven DSP exists in sibling projects (`../patches`,
  `../patches-bundles`); we want to reuse its *sound*, not necessarily its code
  structure.
- **A real editor**: a hardware-style faceplate UI, not just host-generic
  parameter sliders, that interoperates cleanly with DAW automation.
- **macOS / Apple Silicon first**, with nothing that structurally precludes
  Windows/Linux later.

## Decision

### 1. Rust + CLAP via `clack`

The plugin is written in Rust and targets the **CLAP** format through the
**`clack`** bindings (`github.com/prokopyl/clack`, MIT OR Apache-2.0). `clack`
was chosen over `nih-plug` specifically because `nih-plug`'s VST3 export pulls
in GPLv3; CLAP-only via `clack` keeps the whole tree permissively licensed. The
cost is that we hand-build the parameter model, the GUI embedding, and the
bundler ourselves rather than getting them from a framework.

### 2. Crate layout — framework-free core, thin shells

```
crates/vxn-dsp     framework-free DSP kernels (no plugin/UI deps)
crates/vxn-engine  parameters, voice allocation, block render, smoothing,
                   the thread-safe shared parameter store
crates/vxn-clap    clack cdylib: CLAP shell, params, state, gui extension
crates/vxn-ui      Vizia editor, embedded via baseview
xtask              `cargo xtask bundle` — builds/installs the .clap bundle
```

The audio engine knows nothing about CLAP or the GUI; the CLAP and UI layers
depend *down* onto it. `SharedParams` lives in `vxn-engine` (it is just atomics
over the parameter table) so `vxn-clap` and `vxn-ui` share one definition
without depending on each other.

### 3. Processing model — per-sample kernels, block-rate control

DSP kernels run **per sample** and are kept bit-faithful to the reference DSP
(recurrences such as the ladder filter are inherently serial). The *engine*
drives them in **fixed 32-sample control blocks** (`CONTROL_BLOCK`):
modulation, filter coefficients, and the one global LFO are recomputed once per
block (control rate ≈ sr/32). This is the central performance/quality trade:
expensive per-block work, cheap per-sample work.

- **Oversampling**: the synthesis path (osc → mixer → ladder → VCA) runs at
  1×/2×/4× and is decimated by a half-band FIR before the effects; effects stay
  at base rate. Default 2×.
- **Vectorisation**: voices are processed structure-of-arrays
  (`PolyOscillator` / `PolyNoise` / `PolyLadder`, `[f32; 16]` lanes, branchless)
  so the hot path auto-vectorises (NEON on Apple Silicon). Envelopes stay scalar
  per voice.

### 4. Voice & signal architecture

16-voice polyphony with oldest-note stealing. Per voice: 2 oscillators + noise
→ mixer → 4-pole ZDF transistor-ladder lowpass → VCA. Two assignable ADSR
envelopes. One global LFO.

### 5. Modulation — a generic 5×4 matrix

Rather than the Jupiter-8's fixed routing, modulation is a generic matrix of
**5 sources** (ENV-1, ENV-2, LFO, Velocity, Key-follow) × **4 destinations**
(Pitch, Cutoff, Amp, PWM). `dest = Σ_source (depth[source][dest] × source)`.
The VCA gain *is* the Amp column (ENV-2→Amp defaults to 1.0). The generalisation
was chosen over the strict JP-8 subset for reuse across future instruments.

The **UI surfaces this economically**, not as a raw grid: only musically useful
routes get dedicated faders, placed in context (filter envelope/key/LFO/velocity
in the Filter panel; vibrato/pitch-env/PWM in a VCO Mod panel; velocity/tremolo
in an Amp panel). The remaining cells stay engine-only but host-automatable.

### 6. Parameter model & the UI ↔ automation contract

Parameters are a **flat, index-addressed table**: `ParamId` discriminant = CLAP
parameter id = table index, values stored as `f32` in *plain* units. One table
serves the engine (reads), the CLAP layer (stable ids, automation, state), and
the UI (labels, ranges, formatting via `ParamDesc::display`).

Three writers (host automation, UI edits, preset load) and two readers (engine,
UI) coordinate through one thread-safe **`SharedParams`** store (atomics) as the
single source of truth, plus a per-thread **`LocalParams`** mirror in `vxn-clap`
following `clack`'s pattern:

- Host input events fold into the mirror (never the shared store directly);
  `publish` writes mirror → shared. Because the shared store only changes via
  the UI or via `publish`, diffing it surfaces *only* UI edits — host automation
  is never echoed back (no feedback loop).
- UI edits are emitted to the host as parameter events bracketed by CLAP
  gestures (begin/end), so automation recording and undo coalesce.
- The editor reflects host automation by polling the shared store on idle.

### 7. Parameter smoothing — granularity matched to consumption

Zipper-free parameter changes use **different mechanisms per consumption point**:

- **Per-sample** for master volume (the final gain multiply).
- **Block-rate one-pole** for gain-like params (osc/noise levels, PW, mod
  depths) read once per control block.
- **Per-sample coefficient interpolation** in the ladder filter: the modulators
  (automation, LFO, envelope) are *sampled* at block rate, but the resulting
  filter coefficients are *interpolated* per sample across the block. This makes
  block-stepped cutoff a smooth piecewise-linear trajectory and handles all
  cutoff modulation sources uniformly.
- **Snap** for discrete params (enums, bools, ADSR times).

### 8. Editor — Vizia embedded via the CLAP `gui` extension

The editor is built with **Vizia** (skia-safe renderer) and embedded into the
host window via **baseview** through `clack`'s `gui` extension. raw-window-handle
0.5 is used to match the pinned baseview. The look is a Jupiter-8 faceplate:
bordered, header-labelled panels of small vertical faders, rotary selectors for
waveform/colour/shape, switches for two-option choices, a button group for
oversample. The faceplate **font is bundled** (`add_font_mem`) so it renders
identically regardless of the user's installed fonts.

### 9. DSP reuse policy

DSP is **copy-paste-and-adapted** from the sibling `patches` projects, not
imported as a dependency. This keeps `vxn-dsp` self-contained and lets us
simplify/specialise kernels for this synth without coupling to another project's
release cadence.

## Consequences

**Positive**

- Permissively licensed end-to-end; the plugin can ship under MIT/Apache.
- The framework-free engine is unit-testable without a host or GUI and is
  reusable for future instruments.
- The flat parameter table gives stable CLAP ids, trivial state save/restore,
  and one formatting path shared by host and editor.
- The smoothing strategy is cheap (most work at block rate) yet click-free where
  it matters.

**Negative / costs**

- We own code a framework would normally provide: the parameter model, the
  Vizia↔baseview embedding, gesture/automation plumbing, and the bundler. The
  GUI embedding in particular is bespoke.
- Hand-built parameter plumbing has subtle correctness requirements (the
  no-echo mirror, gesture bracketing) that must be maintained by hand.

**Deferred (known, intentional)**

- `request_flush` for the deactivated-plugin-with-open-UI edge case — UI edits
  currently reach the host via the (continuous, for an active instrument)
  process callback.
- Pitch-bend / MIDI-CC routing (the engine hook exists; no event maps to it).
- An authentic bucket-brigade chorus (v1 uses a clean modulated delay).
- A full 5×4 matrix "advanced" view for power users (the engine matrix is
  complete; only the economical surface is built).
- Windows/Linux GUI embedding (engine and CLAP layer are platform-neutral; only
  the nsview parenting path is macOS-specific so far).

## References

- Engine/architecture notes and build status are tracked in the project memory.
- Reference DSP: `../patches`, `../patches-bundles`.
- `clack`: https://github.com/prokopyl/clack — Vizia: https://github.com/vizia/vizia
</content>
