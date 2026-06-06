# ADR 0001 — vxn-core-* shared crate split

- **Status:** Proposed
- **Date:** 2026-06-06
- **Scope:** Boundary between per-synth code (vxn-1, vxn-2, future
  Vulpus Labs synths) and shared infrastructure crates under
  `crates/vxn-core-*`. Companion to epic [E001](../epics/open/E001-vxn-core-extraction.md).

## Context

Vulpus Labs ships two Rust CLAP synths from a shared repo:

- **vxn-1** — Jupiter-8-idiom analogue polysynth. Shipped: full DSP
  kernel, voice bank, CLAP shell, HTML faceplate via `wry` WebView,
  preset system.
- **vxn-2** — DX7-lineage 6-operator FM synth with first-class voice
  stacking. Shipped: DSP kernel, voice allocator, mod matrix, CLAP
  shell with default patch. Editor (HTML faceplate + Controller layer)
  is the open epic `E003-faceplate`.

The two synths are deliberately separate instruments — different
signal models (subtractive analogue vs operator FM), different param
tables (~60 vs ~380 params), different voicing topologies (8-voice
dual-layer vs 16-voice SoA + stacking). That divergence is desirable
and protected: each synth's identity lives in its DSP and its
parameter shape.

What is *not* divergent is everything around the DSP: the CLAP plugin
shell, the parameter-model trait surface, the UI-↔-engine event loop,
the WebView lifecycle, denormal handling, MIDI utilities. Those
exist today in two copies — one in vxn-1 (mature, shipping) and one
either started or scheduled in vxn-2 (`E003` will recreate the entire
Controller + WebView layer if we let it).

The trigger for this ADR: `E003-faceplate` is open and its scope says
"Mirrors `vxn-1/crates/vxn-app` in shape". A second copy of the
Controller + editor scaffolding would lock in the duplication at the
moment we still have the option to extract.

A naive impulse here is to share *everything* possible — pull the
voice allocator into a generic, pull the SharedParams atomic store
out, share the DSP primitives via traits. That impulse is wrong.
Forced abstraction over divergent synth models (FM operator vs analog
osc, 8-voice dual vs 16-voice stack) buys nothing and costs every
future change.

The ADR records where we drew the line, and the rule for where to
draw it for future synths.

## Decision

### 1. Four shared crates under `crates/vxn-core-*`

We promote the repo root to a Cargo workspace owning:

- **`vxn-core-utils`** — denormal/FTZ guard, one-pole parameter
  smoother, MIDI `note_to_hz`, host-tempo subdivision table.
  Zero-dep, `no_std`-compatible. The trivially-shared layer.
- **`vxn-core-app`** — `ParamModel` trait, `ParamDesc` schema,
  `UiEvent`/`ViewEvent` event types, gesture-bracket lifecycle,
  `Controller` event loop, `EditorBackend` trait, `PresetStore`
  trait, `PresetCorpus` model. The synth-agnostic Model+Controller
  surface a faceplate editor talks to.
- **`vxn-core-ui-web`** — `wry` WebView lifecycle, JS↔Rust IPC
  bridge, batched `evaluate_script` view-event sink with
  `batch_chunks` dedup, native text-input popup. Implements
  `EditorBackend` from `vxn-core-app`. HTML/CSS/JS assets stay
  per-synth.
- **`vxn-core-clap`** — `clack` plugin scaffold with the
  Shared/MainThread/AudioProcessor split, CLAP event dispatch
  (notes, bend, CC, mod-wheel, aftertouch), `LocalParams`
  audio-thread mirror pattern, state-blob save/load via
  `ParamModel`, outbound gesture-bracket emit. Generic over a
  small set of engine traits.

vxn-1 and vxn-2 each become a *plug* that satisfies these traits and
supplies its own DSP, voice allocator, param table, and HTML faceplate.

### 2. Explicit not-extracted list

The following stay synth-local and the decision is recorded so we
don't relitigate it:

- **DSP primitives** (oscillators, filters, envelopes, LFOs). vxn-1's
  analog-osc + OTA ladder + ADSR primitives and vxn-2's FM operator +
  exponential 4R/4L EG + voice-stack LFOs are different signal models
  with different numerical contracts (Q32 phase vs f32 phase, SoA
  stack vs scalar voice). A shared "DSP toolbox" would force one of
  them to compromise.
- **Voice allocator**. vxn-1's 8-voice dual-layer `VoiceBank` and
  vxn-2's 16-voice SoA `PolyAlloc` with first-class stacking have
  fundamentally different shapes. Shared interfaces would either be
  too thin to share code or too thick to fit either.
- **`SharedParams` atomic store.** Both synths implement a
  ~750-line lock-free atomic-f32 store keyed by CLAP id. The shapes
  are similar but not identical (vxn-2 has snapshot-to-`EngineParams`
  semantics vxn-1 does not). Duplication cost is low; extraction
  saves 0 wall-clock today. Revisit if a third synth shows up.
- **Param tables.** Per-synth by definition. Only the `ParamModel`
  *machinery* is shared; each synth declares its own descriptor
  registry and implements `ParamModel` over its own storage.
- **Modulation matrix.** vxn-1 = fixed routes (ADR 0004), vxn-2 = 8×9
  generic matrix-only routing. Different topology, different
  source/destination universes. No shared abstraction earns its keep.
- **xtask CLAP bundling.** Both work today and the bundle format is
  tiny (~30 lines per synth). Share if a third synth needs the same
  bundling and the divergence is still nil.

### 3. Per-synth events ride a `Custom` escape hatch

`vxn-core-app::UiEvent` and `ViewEvent` carry the events common to
every keyboard synth (param set, gesture brackets, preset load/save,
text input, param echo, status text). Synth-specific events
(vxn-1's `KeyModeChanged(Whole | Dual | Split { point })`, vxn-2's
mod-matrix row edits) ride a `Custom(Box<dyn Any + Send>)` variant.

Trade-off: the per-synth payload loses exhaustiveness and pays one
dynamic dispatch per event. We chose dynamic over an associated-type
extension (`EventExt::Ui`, `EventExt::View`) because:

- The existing vxn-1 match sites are written assuming closed enums.
  An assoc-type extension forces every `match` to know `Ext`, which
  ripples through the whole crate.
- UI events are control-rate (≤ kHz under user gesture); dynamic
  dispatch cost is irrelevant.
- The `Custom` payload de/serialises via a per-synth `serde` impl
  the synth supplies at controller construction time, so the JS-IPC
  bridge remains symmetric.

If a future synth needs static guarantees we revisit. The assoc-type
alternative is captured here so the reasoning isn't lost.

### 4. State blob wire-compat

vxn-1's current state blob format (versioned header + flat f32
array, indexed by CLAP id) is the shared format. vxn-2's existing
state impl already follows the same shape; the migration in ticket
0006 confirms it byte-for-byte. Existing vxn-1 patches saved in any
host must load cleanly after the extraction — this is a hard
acceptance criterion on the migration ticket.

### 5. Workspace layout: flat, single Cargo.lock

The repo root becomes a single Cargo workspace. `vxn-1/*` and
`vxn-2/*` crates fold in as path members. One `Cargo.lock`,
one `target/`, one `cargo test --workspace`. The `clack` git rev is
pinned once in root `[workspace.dependencies]`.

The alternative — three workspaces (root + vxn-1 + vxn-2) with
relative paths between them — was rejected because pinning the same
`clack` rev in three places risks drift. The flat layout costs a
one-shot disruption (every path-dep re-pointed); the workspace-of-
workspaces alternative costs ongoing vigilance.

### 6. Rule for future shared crates

Extract upward (UI, Controller, plugin shell) before downward (DSP,
voice models). The rule: if two synths' versions of a piece of code
are *substantially the same shape and they'd both improve together*,
share it. If sharing would force either side to compromise its
signal model or its voicing topology, don't.

A concrete test: write the shared trait. If implementing it for
either synth requires unsafe coercion, a `Box<dyn>` boundary that
allocates per-block, or a parameter the synth doesn't actually have,
the abstraction is wrong — leave the duplication.

## Consequences

**Positive:**

- vxn-2's `E003-faceplate` writes a `ParamModel` impl and an HTML
  asset bundle instead of recreating the Controller + WebView +
  IPC + text-input layers. Net work avoided: ~3 kLOC.
- One place to fix bugs in the WebView IPC bridge, the gesture
  bracket lifecycle, the state blob format, the CLAP event dispatch.
- A third Vulpus Labs synth becomes structurally cheaper: implement
  DSP, declare params, implement `ParamModel`, write HTML. The
  shell is free.
- The `Custom` escape hatch lets per-synth events stay per-synth
  without polluting shared types with every variant any synth has
  ever needed.

**Negative:**

- One-shot disruption: every path-dep in vxn-1 and vxn-2 re-pointed
  to root. `Cargo.lock` regenerates. CI scripts assuming
  `vxn-1/cargo build` rather than `cargo build -p` need updates.
- Per-synth events lose exhaustiveness at compile time. A typo'd
  `Custom` payload variant on the synth side is a runtime error,
  not a compile error.
- vxn-1 audio output must remain bit-identical (or within 1e-6 RMS)
  across the migration. Floating-point order is sensitive to
  inlining; an extracted smoother that gets inlined differently
  could perturb LFO modulation by an LSB. Ticket 0006 has the
  golden-render diff as its load-bearing acceptance check.
- Three crates depend on `vxn-core-app`'s event schema. A
  schema-breaking change to `UiEvent` / `ViewEvent` ripples into
  every consumer. Versioning discipline matters more than it does
  today.

**Neutral:**

- The not-extracted list (DSP, voice allocator, SharedParams, mod
  matrix, param tables) is the durable boundary. Future shared
  crates have to clear the rule in §6 — duplication is the default.

## Out of scope

- Sharing DSP primitives via traits. The signal models diverge by
  design; see §2.
- Generic voice allocator. See §2.
- Sharing `SharedParams` storage. The shapes are close but not
  identical; the win is too small to justify a third re-derivation.
  Revisit if a third synth shows up with the same shape.
- xtask bundling. Both bundle scripts work; extract when there's a
  third caller.
- Publishing the `vxn-core-*` crates to crates.io. They're shaped
  to be publishable (no path-only deps in their final form), but
  publishing them as a public synth-building toolkit is a separate
  decision past this ADR.

## References

- Epic [E001 — vxn-core-* shared crate extraction](../epics/open/E001-vxn-core-extraction.md)
- vxn-1 ADR 0007 — VXN1 MVC architecture (the Controller + ParamModel
  surface this ADR shares is vxn-1's mature implementation)
- vxn-2 ADR 0001 — VXN2 overall design (the divergent DSP/voicing
  decisions that justify the not-extracted list)
- vxn-2 epic E003 — VXN2 HTML faceplate editor (the consumer that
  triggered this extraction)
