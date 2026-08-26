---
id: "0314"
product: vxn-1b
title: "Module docs that describe architectures VXN1b no longer has"
priority: high
created: 2026-08-26
epic: E047
depends: []
---

## Summary

Distinct from the war-story volume in [[0315]]: these comments are **factually
wrong about the current code**, and a reader trusting any of them will make a
bad change. Several actively contradict code within a few lines of themselves.

### Contradicted by adjacent code

- [host-runner.mjs:8-12](../../vxn-1b/crates/vxn1b-wasm/web/host-runner.mjs#L8-L12)
  — *"The runner owns the wasm bytes and the SABs so it can re-instantiate after
  a trap … a fresh host over the same SABs resumes exactly where the dead one
  left off."* The 0297 block **six lines below** says the opposite; the runner
  stays down ([:104-113](../../vxn-1b/crates/vxn1b-wasm/web/host-runner.mjs#L104-L113)).
- [coordinator.mjs:251-258](../../vxn-1b/crates/vxn1b-wasm/web/coordinator.mjs#L251-L258)
  — the `"trap"` comment says the runner *"already caught it and kicked async
  recovery"*, that `ready` *"flips back true on the next ready after re-init"*,
  and that *"the faceplate bridge (0290) is what listens for this"*. There is no
  re-init, and the bridge does not listen — `boot()`'s `onTrap` only
  `console.error`s.
- [vxn1b-clap/src/lib.rs:5-7](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L5-L7)
  — *"There is no faceplate yet — the HTML editor + its controller land in
  E038"*, with `mod gui;` on line 23 and `PluginGui` registered on line 156.
  Same staleness at [:205-207](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L205)
  (*"so the (future) editor can hold a clone too"*).

### Wrong about the engine's shape

- [bank.rs:12](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L12) — *"the engine
  runs **two** for 16-voice poly"*. `Synth::BANKS = 4`, `MAX_VOICES = 32` since
  0264. Same error at [:114](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L114).
- [bank.rs:26](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L26) — *"All four
  assign modes are live: the `Voices` coordinator resolves Poly/Unison/Solo/Twin"*.
  `AssignMode` was split into `StackWidth` × `VoiceMode` by 0266 / ADR 0003 and
  greps to zero outside stale test names. Also
  [:925](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L925).
- Five more pre-0264 voice counts: `voice.rs:235` (*"The 16-voice bank's
  allocation"*), `voice.rs:809` (*"Arrays are the full 16-voice width; the engine
  slices each 8-lane bank out"*), `synth.rs:198`, `synth.rs:242`, `matrix.rs:8`
  (*"VXN1b is a **flat 16-voice** instrument"*), `mod_smoothing.rs:20`.

### Misattached docs

- [synth.rs:237-246](../../vxn-1b/crates/vxn1b-engine/src/synth.rs#L237-L246) —
  `lfo2_phase`'s doc block **and its `#[inline]`** both land on `is_silent`,
  because a second doc block was inserted between them. So `lfo2_phase` (called
  once per control block on the LFO-link path) is undocumented and un-inlined,
  and `is_silent` is documented as something it isn't.
- [vxn1b-ui-web/src/lib.rs:669-680](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L669-L680)
  — the doc for `css_covers_every_control_primitive` (at :718) is glued above
  `rule_heads` (at :685), so rustdoc attaches all of it to the helper.

### Smaller, same class

- [faceplate-bridge.mjs:351](../../vxn-1b/crates/vxn1b-wasm/web/faceplate-bridge.mjs#L351)
  — `resyncEngine`'s doc says *"Called when the worklet reports ready"*; the only
  call site is `attachGestureGate`'s `onGesture` after `await host.start()`,
  which resolves *before* the worklet posts `ready`. The behaviour is fine; the
  comment is wrong.
- [web-controller/src/lib.rs:579](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L579)
  — *"Clone out of `pending` to satisfy the borrow checker"* sits above a
  `std::mem::replace` that exists specifically to avoid cloning.
- [controller.mjs:24](../../vxn-1b/crates/vxn1b-wasm/web/controller.mjs#L24) —
  header says `patchCount` *"is exposed for the id split"*; nothing in JS reads
  it (the page gets `__PATCH_COUNT__` baked in from Rust).
- [xtask/src/main.rs:47-51](../../vxn-1b/xtask/src/main.rs#L47-L51) —
  `VST3_NAME` has two stacked doc comments that contradict each other on what
  the constant is.
- [factory.rs:9-11](../../vxn-1b/crates/vxn1b-engine/src/factory.rs#L9-L11)
  asserts the directory name *is* the browser's grouping category; the browser
  actually groups on `meta.category` from the TOML, and the directory-derived
  field is read only for a sort. All shipped presets agree, so nothing is
  visibly broken — the doc describes a coupling that is not enforced.
- [faceplate.css:93](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.css#L93)
  and [:1348](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.css#L1348) point
  at a `.fader-scale-lg` selector that does not exist;
  [:606-607](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.css#L606-L607)
  claims uniformity across a Cross Mod panel that was folded into Voice.
- [discrete.js:2](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/discrete.js#L2)
  claims the file holds the FX tab strip (it is `dispatch.js:522 wireTabs`);
  [:61](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/discrete.js#L61)
  documents `data-no-label` as *"used inside `.route-col`"*, a class that
  appears nowhere.

## Design

Mechanical, but two rules make it not-mechanical:

1. **Prefer naming the constant over restating the number.** The "16-voice"
   errors happened because a count was written into prose once and widened in
   code later. Write `Voices::CAPACITY` / `MAX_VOICES` / `Synth::BANKS`, or say
   "the lane pool" — do not write `32` and create the same bug for 0264's
   successor.
2. **Fix the doc to match the code, not the code to match the doc** — except for
   `synth.rs:237`, where the misplaced `#[inline]` is a real (if minor) codegen
   change that should go where it was intended.

`factory.rs` is the one item needing a judgement: either enforce the coupling
the doc claims (a test asserting directory == `meta.category`) or correct the
doc to say the directory only drives the sort. Enforcing is better — it is one
test and it makes a moved file fail loudly instead of sorting under one heading
and grouping under another.

## Acceptance criteria

- [ ] No module doc in `vxn-1b/` states a voice, lane or bank count that
      disagrees with the consts.
- [ ] The three self-contradicting comments (host-runner, coordinator, clap
      lib.rs) describe what the code does now.
- [ ] `lfo2_phase` has its own doc and its `#[inline]`; `is_silent` has its own.
- [ ] `css_covers_every_control_primitive`'s doc sits on the test.
- [ ] The `factory.rs` category coupling is either enforced by a test or the doc
      no longer claims it.
- [ ] The remaining smaller items are corrected or deleted.
- [ ] A grep for `AssignMode`, `Poly/Unison/Solo/Twin` and `16-voice` across
      `vxn-1b/**` returns nothing outside genuinely historical files.

## Notes

- This is deliberately split from [[0315]] so the corrections are not held up
  behind the stylistic judgement calls, and so this one can be reviewed by
  checking each claim against the code rather than by taste.
- Several stale test *names* still say `assign_mode`. Renaming them is in scope;
  changing what they assert is not.
