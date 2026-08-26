---
id: E047
product: vxn-1b
title: "VXN1b post-ship quality sweep: dead code, duplicated wire, comment archaeology"
status: open
created: 2026-08-26
---

> VXN1b went from fork to shipped plugin to shipped web build in about ten weeks
> (E038 → E039 → E045), and the debt that accumulates from moving that fast is
> now legible. A six-agent read of the whole product (23.4k Rust, 16.7k
> first-party JS/CSS/HTML) found the core in good shape and the periphery
> carrying **a dead 249-line module that still ships in the bundle**, two of the
> wire's four halves unused, ~370 ticket-number references in comments, and a
> set of module docs that describe architectures the code no longer has.

## Why now

Three findings are the argument; the rest is tidying that rides along.

**1. The bundle ships dead code and CI would not tell you.**
[`panels/keys.js`](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/keys.js) opens
with `document.querySelector('.panel[data-name="Keys"] .panel-body')` and
`faceplate.html` has no Keys panel — the CSS for it was already deleted. Every
export is a no-op stub, six `dispatch.js` call sites call into `function(){}`,
and the file is still `include_str!`'d at
[vxn1b-ui-web/src/lib.rs:596](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L596)
and spliced into the shipped page. Separately, the worklet's CPU meter still
accumulates per-quantum **on the render thread** for a readout that has no
consumer.

**2. Nothing runs VXN1b's JS suites.** ~2,600 lines of `node --test` under
[`vxn1b-wasm/web/`](../../vxn-1b/crates/vxn1b-wasm/web/) execute nowhere in CI;
[test.yml](../../.github/workflows/test.yml) installs npm and runs Vitest for
vxn-1 only. That includes `wasm-agreement.test.mjs`, whose own header records
that the drift it guards against *"was caught immediately by the runtime
handshake — nobody ran it."* Still nobody-run. And
[bundle.yml](../../.github/workflows/bundle.yml) builds vxn-1 only, so VXN1b's
CMake / `force_load` / `/INCLUDE:clap_entry` path is exercised **only on a
`vxn-1b-*` release tag** — precisely the configuration that shipped hollow VST3s
before ([[vxn-windows-vst3-optref-strip]]).

**3. The doc layer has drifted into being wrong, not just verbose.**
[`bank.rs:12`](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L12) describes a
two-bank/16-voice engine (`BANKS = 4`, `MAX_VOICES = 32` since 0264) and
documents `AssignMode`, which 0266 split into `StackWidth` × `VoiceMode` and
which now greps to zero.
[`host-runner.mjs:8-12`](../../vxn-1b/crates/vxn1b-wasm/web/host-runner.mjs#L8-L12)
says the runner re-instantiates after a trap; the 0297 block **six lines below
it** says it does not.
[`vxn1b-clap/src/lib.rs:5`](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L5) says
*"There is no faceplate yet"* with `mod gui;` eighteen lines down. These are not
style complaints — a reader trusting any of them will make a wrong change.

## What the review found good

Worth writing down so the sweep does not "fix" it:

- `preset.rs` / `state.rs` / `preset_io.rs` are genuinely well-factored —
  table-driven, sparse, the depth-authority rule stated once, no copy-pasted
  per-field lists.
- The DSP hot-path / pure-maths split (`eval.rs` ↔ `render.rs` ↔ `bank.rs`) is
  correct factoring, not redundancy, and the alloc-free story is real and tested.
- `controller.mjs` / `coordinator.mjs` / `faceplate-bridge.mjs` divide duties
  cleanly (model wasm / audio transport / router + pump).
- Zero `console.log` scaffolding, essentially zero commented-out code, no
  `#[allow(dead_code)]` outside one test.
- Parameter *metadata* has exactly one source: everything flows from
  `vxn1b_engine::params::desc_for_clap_id`. No restated descriptor table.
- Several comments are load-bearing and must survive the sweep: Safari's
  `latencyHint`/one-quantum limit, the JSC-GC byte loop, SAB-not-`postMessage`,
  the worklet `performance.now()` gap, `/WHOLEARCHIVE` + `/INCLUDE:clap_entry`,
  and the ring-before-store ordering invariant.

## Goal

VXN1b's shipped bundle contains only code that runs; its wire has one encoder
and one decoder; its comments state constraints rather than narrate tickets; and
CI proves all of that on every push rather than on a release tag.

## Scope

**In:** all of `vxn-1b/`; the two CI workflows; one new shared xtask crate
(0317) that vxn-1 and vxn-2 also adopt.

**Out:** any change to audio output. This epic is behaviour-preserving by
construction — the one exception is 0310's dead Reset button, which is
[0307](../../tickets/open/0307-vxn1b-reset-button-dead.md)'s call, not this
epic's. No DSP algorithm changes. Not vxn-3. The vxn-2 equivalents of these
smells are [0298](../../tickets/open/0298-vxn2-web-controller-smells.md).

## Planned tickets

Do **0321 first** — it is the cheapest ticket here and every subsequent deletion
wants CI watching it. After that the three deletion tickets (0310/0311/0312) are
independent of each other and of the refactors.

- [ ] **0321** *(monorepo, high)* — CI: run VXN1b's node suites; add a VXN1b
      bundle job with the non-hollow `strings | grep` check.
- [ ] **0310** *(vxn-1b, high)* — Delete the dead web surface: `panels/keys.js`
      and its splice slot, the CPU meter, `makeDropdown`, the `src-off` dim
      rule, ~22 unreferenced exports. **Depends on [[0307]]** — the Reset button
      lives in the file being deleted.
- [ ] **0311** *(vxn-1b, medium)* — Delete the dead Rust surface: the single-lane
      allocation path, `_PARAM_COUNT`, `max_frames`, two `set_sample_rate`
      chains, `is_sync_flag`, `last_width`, the `--release` no-op flag.
- [ ] **0312** *(vxn-1b, high)* — One encoder: collapse the wire's two dead
      halves and point the golden table at the encoder that ships. Adds the
      patch/global count handshake.
- [ ] **0313** *(vxn-1b, high)* — Pass `RenderView` into `RenderBank::render`,
      then split the 452-line body at its own phase banners; derive `BlockCtx`'s
      four cross-mod fields from the one enum.
- [ ] **0314** *(vxn-1b, high)* — Correct the module docs that are factually
      wrong. Correctness, not style.
- [ ] **0315** *(vxn-1b, low)* — War-story sweep: keep the constraint, drop the
      event. ~370 ticket refs, 12 quoted narrative blocks.
- [ ] **0316** *(vxn-1b, medium)* — Bind the cross-language tables: the custom-op
      vocabulary (3 transcriptions) and the telemetry payload shape (2), neither
      pinned by a test.
- [ ] **0317** *(monorepo, medium)* — `vxn-xtask-common`: 3,015 lines of
      triplicated bundler across three products, plus `gui.rs` ×3.
- [ ] **0318** *(vxn-1b, medium)* — Extract the five remaining long functions.
      Every one has its seams already written in as banner comments.
- [ ] **0319** *(vxn-1b, low)* — Collapse intra-file boilerplate: the FX `run_*`
      quintuplet, the ×4 smoother pattern, the hand-synced matrix enum tables,
      `.lock()` ×8, `*_buf_reserve` ×3, the arg-pair decode ×5.
- [ ] **0320** *(vxn-1b, medium)* — Close the test gaps the sweep exposed:
      `preset_io`'s untested filesystem half (including the path-escape guard),
      hydrate opcodes whose tests exercise copies, and `WEB_BOOT_HEAD`'s
      un-linted inline JS.

## Risks

- **VXN1b ships.** Every ticket here touches a released product
  ([[vxn-release-process]]). Nothing in the epic should change audio, so a
  `cargo test` + node-suite pass is *usually* sufficient proof — but 0313 edits
  the render bank's signature and 0311 deletes an allocation path, and those two
  want a manual DAW pass ([[verify-audio-in-reaper]]) before close.
- **Deleting dead code is where the surprises live.** 0310's `keys.js` is
  provably unmounted, but it is `include_str!`'d into a splice order that
  `css_covers_every_control_primitive` and the orchestration suite both depend
  on. Land the splice-slot removal in its own commit.
- **0312 changes what the golden table proves.** Today it validates an encoder
  that never runs. Retargeting it is the point of the ticket, but it means the
  test that goes green afterwards is testing something different — say so in the
  close-out rather than letting a future reader assume continuity.
- **The comment sweep can delete load-bearing text.** 0314 and 0315 are separate
  tickets precisely so the correctness fixes are not held up behind a stylistic
  judgement call, and so the "do not cut" list above gets applied once, carefully.
- **0317 touches three products' build tooling** and can break bundling for all
  of them at once. It is deliberately last-ish and deliberately depends on 0321
  landing first, so CI is watching before the bundler moves.

## Acceptance

- The shipped web bundle and the shipped plugin contain no module, export,
  branch or CSS rule that the review identified as unreachable — or the survivors
  are listed here with a reason.
- CI runs VXN1b's Rust tests, its node suites and a VXN1b bundle on every push;
  a hollow VST3 fails the build.
- Exactly one encoder and one decoder on the event wire, with the golden table
  pointed at the encoder that ships.
- No module-level doc comment in `vxn-1b/` describes an architecture, a voice
  count, a feature or a recovery path that the code does not have.
- Every cross-language duplicated table is either collapsed to one source or
  pinned by a test that fails on drift.
- Both products' suites green, 0 skipped ([[0295]]); one manual DAW pass for
  0311 and 0313.
