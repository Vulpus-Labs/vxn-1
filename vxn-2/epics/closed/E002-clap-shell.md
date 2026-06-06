---
id: E002
title: VXN2 CLAP shell + UI-less playable plugin
status: closed
created: 2026-06-05
closed: 2026-06-06
---

## Goal

Wrap the E001 kernel in a `clack`-based CLAP plugin so a host (Bitwig,
Reaper, the bundled `clack-host` test) can load `vxn2.clap`, send MIDI, and
hear the synth respond to an illustrative default patch. No editor —
parameters are driven from the host's generic controls plus matrix routes
defined in the default patch.

When this epic closes:

- `vxn2-clap` builds as a `cdylib` and bundles into a `vxn2.clap` host-
  loadable plugin.
- An xtask installs the bundle to `~/Library/Audio/Plug-Ins/CLAP/`.
- Bitwig / Reaper / `clack-host` enumerate the full parameter table (174
  automatable params from 0012), receive notes via the CLAP note port and
  raw MIDI, and render audio.
- Loading the plugin produces an immediately useful sound on its first
  note — the default patch is not "all params at defaults"; it is a
  hand-tuned illustrative patch that exercises FM, stacking, both LFOs,
  delay, and reverb.
- Plugin state survives host save/reload.

UI is **out**. The HTML faceplate ships as a separate epic that bolts onto
the `SharedParams` surface this epic establishes.

## Scope

**In:**

- `vxn2-clap` crate: `cdylib` + `rlib`, `clack-plugin` + `clack-extensions`
  dependencies, plugin descriptor, audio/note port declarations, extension
  registration.
- `SharedParams` atomic param store (Arc-shared between main and audio
  threads, lockless). `LocalParams` audio-thread mirror for engine input +
  UI-echo (UI-echo path is stubbed but wired so the UI epic only adds the
  consumer).
- CLAP `params` extension wired to `vxn2-engine::params::ParamTable` from
  0012. `get_info` / `get_value` / `value_to_text` / `text_to_value` /
  `flush` mirror the VXN1 shape; module strings group params by
  layer/section for the host's automation list.
- Audio thread `process()`: note-on / note-off (CLAP + raw MIDI 1.0),
  pitch bend, CC1 (mod wheel), channel aftertouch, host transport →
  engine tempo, parameter event handling, scratch stereo render copied
  to host channels.
- State save/restore via the `state` extension, using the param-table
  snapshot blob produced in 0012.
- Default patch baked into engine init: an illustrative DX-EP-style sound
  (algo 5, stacking density 4, LFO2 vibrato with delay/fade, modest
  delay + reverb, two matrix slots — velocity → carrier level, voice_rand
  → lfo2 phase for stack decorrelation).
- xtask: `cargo xtask bundle` (build cdylib, assemble `.clap` bundle
  directory with `Info.plist`), `cargo xtask install` (copy bundle to
  the user CLAP search path).
- Integration smoke test via `clack-host`: instantiate the plugin, send a
  note-on burst, render N seconds, assert non-silent + finite output for
  the default patch and for a parameter-sweep over every CLAP id.

**Out (later epics):**

- HTML faceplate (`vxn2-ui-web`), `WebView`-backed editor, GUI extension.
- `vxn-app`-style `Controller` + `ViewEvent` / `HostEvent` plumbing — UI
  epic introduces these on top of `SharedParams`.
- Preset format + factory bank beyond the single default patch.
- Mod matrix CLAP exposure beyond slots 1–8 depth per layer (already
  defined in 0012 / 0008; this epic only honours that exposure).
- MPE / per-note expression.
- Windows / Linux bundle plumbing (the xtask targets macOS first; the
  cdylib itself is cross-platform).

## Tickets

- [ ] [0013 — vxn2-clap crate scaffold + plugin entry + ports](../../tickets/open/0013-clap-scaffold.md)
- [ ] [0014 — SharedParams + LocalParams (atomic store, mirror)](../../tickets/open/0014-shared-params.md)
- [ ] [0015 — CLAP params extension wired to ParamTable](../../tickets/open/0015-clap-params.md)
- [ ] [0016 — Audio process: notes, MIDI, transport, render](../../tickets/open/0016-process-loop.md)
- [ ] [0017 — State save/restore](../../tickets/open/0017-state-extension.md)
- [ ] [0018 — Default illustrative patch](../../tickets/open/0018-default-patch.md)
- [ ] [0019 — Plugin bundling xtask](../../tickets/open/0019-bundle-xtask.md)
- [ ] [0020 — End-to-end smoke test via clack-host](../../tickets/open/0020-clack-host-smoke.md)

## Dependency order

```text
E001 (kernel + ParamTable from 0012)
  │
  ├─> 0013 (crate scaffold) ──> 0014 (SharedParams) ──> 0015 (params ext) ──> 0016 (process) ──> 0017 (state)
  │                                                                                                  │
  │                                                                                                  └─> 0020 (smoke test)
  │
  ├─> 0018 (default patch) ──────────────────────────────────────────────────────────────────────────┘
  │
  └─> 0019 (bundle xtask)  — depends on 0013 only; runs in parallel with the params/process chain
```

- 0013 stands up the crate, descriptor, port declarations, and an empty
  `process()` that returns silence. Everything downstream attaches to it.
- 0014 introduces the lockless param store. Audio thread reads it; later
  the UI epic will write it. For E002 only the audio thread reads and the
  CLAP param events write.
- 0015 wires the host's parameter list to `ParamTable`. After this, the
  host shows every CLAP-automatable param with correct ranges + display
  strings, but moving them has no audible effect until 0016.
- 0016 implements `process()` proper: events drive engine state, scratch
  buffers render, host transport feeds tempo. After this the plugin makes
  sound under generic host controls.
- 0017 plugs in the `state` extension so the host can save/reload patches.
- 0018 supplies the default-patch values so the plugin sounds *useful*
  on first load instead of "all defaults". Can land any time after 0012.
- 0019 produces a `.clap` bundle + install command. Can land alongside any
  of the above — needed only when a real host is the test target.
- 0020 closes the loop: a `clack-host` integration test that loads the
  built plugin, sends notes, and asserts non-silent finite output.

## Acceptance

- `cargo xtask bundle` produces `target/release/vxn2.clap/` containing the
  cdylib at the standard CLAP bundle path; `cargo xtask install` copies it
  to `~/Library/Audio/Plug-Ins/CLAP/vxn2.clap`.
- Bitwig (or another CLAP host on the dev machine) loads the bundle,
  enumerates 174 automatable parameters grouped by section, and plays
  audible notes from a MIDI keyboard against the default patch.
- The default patch sounds like an intentional, hand-tuned sound — not
  a single sine carrier — on the first note after load. It uses every
  major engine block (FM, stacking, both LFOs, both extra envelopes,
  matrix, delay, reverb).
- `clack-host` integration test renders ≥ 1 second of stereo audio for the
  default patch, asserts no NaN / Inf, asserts RMS > a small threshold
  (audible), and asserts silence after the last note's release tail.
- Parameter sweep test: for each of the 174 CLAP ids, set the value to
  min / mid / max in turn over a held note, render, assert no NaN / Inf
  and no panic.
- Host transport tempo changes are reflected within one block in LFO1
  rate (when synced) and delay time (when synced).
- State save → reload restores the param table bit-identically (snapshot
  blob round-trip).
- No RT allocations, no `unwrap` / `expect` in the audio path, no panics
  across the `process` boundary.
- Build with `panic = "unwind"`; a panic inside `process` does not abort
  the host process (caught at the FFI boundary by clack).

## Notes

- `vxn-1/crates/vxn-clap` is the structural template — copy the **shape**
  (extensions registered, `SharedParams` Arc pattern, `LocalParams`
  publish/fetch idiom, `ScopedFlushToZero`, scratch buffer sizing from
  `max_frames_count`) but not the `Controller` / `ViewEvent` / GUI /
  timer code, which all belong to the UI epic.
- `clack`, `clack-plugin`, `clack-extensions` should be added to the
  workspace `Cargo.toml` as workspace deps so any future crate that
  needs them (xtask host tests) picks up the same revision. Pin to the
  same `rev` VXN1 uses to avoid version skew across the two workspaces.
- The default patch values live in the engine (`vxn2-engine::default_patch`)
  so they are reachable from both the CLAP shell and any future preset
  loader / test harness — do not bake them into `vxn2-clap`.
- macOS-only bundle for now. Linux `.so` + Windows `.dll` bundle layouts
  are mechanically similar; defer until a request surfaces.
- `vxn-1` keeps a `dev-dependencies` line on `clack-host` at the same
  pinned rev for its own tests; mirror that in `vxn2-clap` and again in
  the bundling xtask if it grows host-side integration tests.
