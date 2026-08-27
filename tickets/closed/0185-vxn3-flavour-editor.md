---
id: "0185"
product: vxn-3
title: "vxn-3 flavour/voice editor + save-as — faceplate voice library, assign-to-lane, flavour-aware value_to_text"
priority: high
created: 2026-07-05
epic: E034
---

## Summary

The "playable-with" payoff of E034: a faceplate for editing **voices** (engine + flavour)
and assigning them to lanes, plus the backend that makes the edits audible and persistent.
Reworked from the ticket's original per-lane sketch into a **voice-library** model
(user direction): lanes *reference* named voices; voices are edited as presets in their
own tab; editing a voice updates every lane using it.

Design: [ADR 0005](../../vxn-3/adrs/0005-vxn3-voice-families-flavours-macros.md) (base +
bindings the faceplate edits; a kit = lanes × voices). Builds on the 0180–0184 flavour
runtime and the 0172 host param surface.

## Design

- **Faceplate (voice-library model).** Two tabs — **Pattern** and **Voices**.
  - Pattern lane: an engine-coloured **voice box** (click → voice **browser**), the step
    grid, 3 performance macro knobs (auto-labelled from the voice's macro names), gain /
    pan / send / length.
  - Voices tab: a voice **library** (grouped by engine; new / duplicate / delete) + a
    **voice editor** — name, engine type, **base sliders** (one per family param), and
    **macro bindings** (per slot: rename + one-to-many target/depth/curve rows).
  - Voice browser: assign a voice to a lane.
- **Backend.**
  - `Flavour` gains `macro_names[K]` (serialised; `FLAVOUR_VERSION` 1→2, v1 blobs still
    load with empty names).
  - Main-thread **flavour store** ([`FlavourStore`] in `EngineIo`) — per-lane deep-patch
    source of truth; uncontended mutex (main-thread-only access).
  - **`assign_voice`** opcode (JS → `parse_custom_ui` → `Vxn3UiCustom::AssignVoice` →
    `tick_vxn3`): stores the flavour, mirrors the kind, builds the engine with the
    flavour applied, swaps it in. The single edit path — a voice edit re-sends it for
    every lane using the voice.
  - **`value_to_text` flavour-aware** (0172): a macro slot renders `"<name> <pct>%"`
    (name = user override → first-bound-param → `M<n>`), round-tripped by
    `flavour_macro_parse`.
  - **`clap.state`** persists each lane's live flavour (base + bindings + macro names);
    load restores them into the store and applies to the engines.
- **Live apply** is via the existing engine **swap** (rebuild + `apply_flavour`), reusing
  `SetEngine`'s machinery. Editing while a long voice rings restarts it — acceptable for
  a drum editor; a no-glitch flavour channel is a later option.

## Acceptance criteria

- [ ] Faceplate: base sliders + macro-binding assignment (target param, depth, curve per
      slot, multi-binding, renameable macros); voice library with assign-to-lane.
- [ ] `assign_voice` applies engine + flavour to a lane (store + kind + swap); a voice
      edit updates every lane using it.
- [ ] `value_to_text` reads the flavour's macro name + knob %, round-tripping cleanly.
- [ ] Per-lane flavour persists through `clap.state` save/load (base + bindings + names);
      `clap-validator` state-reproducibility passes.
- [ ] `cargo test -p vxn3-engine -p vxn3-clap -p vxn3-app -p vxn3-ui-web` green; clippy
      clean; `clap-validator` 0 failures.

## Notes / follow-ups

- **Faceplate load-sync (deferred):** on `clap.state` load the *audio* restores, but the
  web UI still shows its default voice assignments — vxn-3 has no view-event announcing
  loaded state (patterns don't sync on load either). A dedicated "state → UI restore"
  ticket covers this.
- **Persistent user voice bank (deferred):** the voice *library* is session-local in JS;
  voices assigned to lanes persist (as the lane's flavour), but unassigned named voices
  don't survive a reload. The factory/user bank ticket (0188+; 0187 is taken by vxn-2)
  moves flavours to `include_dir!` TOML and adds user-bank storage.
- Mind [[vxn3-flavour-runtime]].

## Close-out (2026-08-27)

- **Faceplate** ([app.js](../../vxn-3/crates/vxn3-ui-web/assets/app.js)):
  Pattern + Voices tabs, a voice library (`voices[]`, add / duplicate / delete),
  the voice browser overlay (`#voice-browser`, "Assign voice → Track N"), base
  sliders, and per-slot macro bindings with target / depth / curve rows —
  multi-binding per slot, additive-from-base
  ([:439-486](../../vxn-3/crates/vxn3-ui-web/assets/app.js#L439-L486)) — plus
  renameable macros (`flav.macro_names[slot]`, `""` = derive).
- **`assign_voice`** parses base + bindings + macro defaults + names
  ([vxn3-ui-web/src/lib.rs:140](../../vxn-3/crates/vxn3-ui-web/src/lib.rs#L140)),
  routes as `Vxn3UiCustom::AssignVoice { track, kind, flavour }`
  ([vxn3-app/src/lib.rs:115](../../vxn-3/crates/vxn3-app/src/lib.rs#L115)), and a
  voice edit re-sends it to every lane referencing that voice
  ([app.js:160](../../vxn-3/crates/vxn3-ui-web/assets/app.js#L160)).
  Unit-tested by `parses_assign_voice`.
- **`value_to_text`** renders a macro slot flavour-aware — override name /
  first-bound-param / `M<n>`, plus knob percent — reading the main-thread
  `FlavourStore` and falling back to the fixed 0172 engine map only if
  unavailable ([vxn3-clap/src/lib.rs:509-535](../../vxn-3/crates/vxn3-clap/src/lib.rs#L509-L535)).
  `text_to_value` inverts it (`flavour_macro_parse`), so the transform
  round-trips.
- **Per-lane flavour through `clap.state`**: `FlavourStore` is saved and loaded
  with the param cache and track kinds
  ([vxn3-clap/src/state.rs:46](../../vxn-3/crates/vxn3-clap/src/state.rs#L46)).
- **clap-validator on `target/release/vxn3.clap`: 20 tests run, 17 passed,
  0 failed, 3 skipped** — including `state-reproducibility-basic`, `-flush`,
  `-null-cookies` and `state-buffered-streams`. The 3 skips are the unimplemented
  preset-discovery factory; the 1 warning is a 334 ms scan time.
- `cargo test --workspace` (covers vxn3-engine / vxn3-clap / vxn3-app /
  vxn3-ui-web): 1622 passed, 0 failed. `cargo clippy -p vxn3-engine -p vxn3-clap
  -p vxn3-app -p vxn3-ui-web --all-targets`: 0 diagnostics.
