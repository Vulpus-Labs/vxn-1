---
id: "0315"
product: vxn-1b
title: "War-story sweep: keep the constraint, drop the event"
priority: low
created: 2026-08-26
epic: E047
depends: ["0314"]
---

## Summary

VXN1b's comments narrate their own history. Six independent reviewers each
flagged it without prompting, and the volume is real: **~370 bare ticket-number
references** across the product (assets 157, `bank.rs`/`state.rs` 104, engine
core 56, clap/wasm 54), plus roughly **130 of the ~1,190 non-test lines** in
`vxn1b-web-controller/src/lib.rs` given over to narration.

Individually most are defensible — "don't re-add the thing I removed" is a real
service. Collectively the doc layer reads as a changelog, and [[0314]] documents
what that costs: prose that describes the code's past is prose nobody updates
when the code moves again.

### The rule

**Keep the constraint. Drop the event.**

> *"The wire name must stay `pwm` — presets and state blobs written before the
> split decode by name."* → keep. It tells you what you may not do.
>
> *"0261 relabelled this one; its wire name stays `pwm`, so presets and state
> blobs written before the split decode unchanged."* → trim to the above.

Bare ticket refs as provenance tags are fine. A ticket ref *plus a paragraph of
what the code used to be* is not.

### The clearest cases

**Told more than once:**

- The 0262 mono-fast-path removal is narrated **four separate times**:
  [output.rs:142-146](../../vxn-1b/crates/vxn1b-engine/src/output.rs#L142-L146),
  [output.rs:11-12](../../vxn-1b/crates/vxn1b-engine/src/output.rs#L11-L12),
  [output.rs:226](../../vxn-1b/crates/vxn1b-engine/src/output.rs#L226),
  [engine.rs:1654-1656](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L1654-L1656).
  One removal, four tellings.
- The empty-factory VST3 incident, **twice in adjacent blocks**:
  [CMakeLists.txt:133-150 and :151-163](../../vxn-1b/wrapper/CMakeLists.txt#L133-L163)
  — *"the link SUCCEEDED, producing a ~516 KB module of wrapper glue with an
  empty factory. Shipped that way in VXN1/VXN2 0.1.1 and VXN1b 0.0.1"* then
  *"a successful link, no diagnostics, and a ~516 KB module with an empty
  factory."* The flag constraints are load-bearing and stay; the postmortem
  belongs in the ticket.

**Describes code that isn't there:**

- [render.rs:22-28](../../vxn-1b/crates/vxn1b-engine/src/render.rs#L22-L28) —
  six lines on five functions 0273 deleted. The last sentence (the rule for
  adding a dest) is the part worth keeping.
- [web-controller/src/lib.rs:53-59](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L53-L59)
  — a whole doc section about vxn-1's readback diff, ending *"There is no
  `pump_readback` here and nothing for one to observe."*
- [matrix.rs:515-518](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L515-L518)
  — a pre-wired slot layout that stopped existing 40 tickets ago.
- [state.rs:52-64](../../vxn-1b/crates/vxn1b-engine/src/state.rs#L52-L64) — an
  11-entry changelog for versions the code rejects three lines later. The
  bump-and-reject rationale stays; the list goes.

**Reconstructs a debugging session:**

- [vxn1b-ui-web/src/lib.rs:37-48](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L37-L48)
  — *"= 554 nominal, **556 as laid out** … At 554 the last 2 px — the bottom
  row's panel border — were cut off"*.
- [faceplate.css:1560-1565](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.css#L1560-L1565)
  — *"a 36 px dial claiming 90 px of it pushed the mute and the whole split
  section out of the panel (they rendered **underneath** the FX row's panels,
  **which is why they looked deleted rather than overflowing**)"*.
- [faceplate.css:1661-1666](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.css#L1661-L1666)
  — *"a sharp (\"F#3\" vs \"F3\") widened the whole column and **shoved the
  mixer's strips sideways as the slider moved**"*.
- [dispatch.js:435-451](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L435-L451)
  — 17 lines on `freshenCell`: *"…so after visiting Layer 2, a click on the
  Voice rocker wrote Poly/Solo to layer 1 **and** layer 2"*.
- [telemetry.mjs:180-192](../../vxn-1b/crates/vxn1b-wasm/web/telemetry.mjs#L180-L192)
  — 12 lines narrating a bug that no longer exists, to justify `= 0`.
- [vxn1b-ui-web/src/lib.rs:672-679](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L672-L679)
  — the longest, inside a test: *"Nothing failed — every unit test still passed,
  because the breakage was purely visual…"*

**Gossip about the other synths:**

- [web-controller/src/lib.rs:16-51](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L16-L51)
  — 36 lines comparing against `vxn2-web-controller`.
- Four JS headers on what vxn-1 / vxn-2 do differently: `coordinator.mjs:22-25`,
  `event-ring.mjs:3-6,34-38`, `param-store.mjs:8-13`, `controller.mjs:18-24`.
- [faceplate-bridge.mjs:31-38](../../vxn-1b/crates/vxn1b-wasm/web/faceplate-bridge.mjs#L31-L38)
  — *"An earlier cut of this file ALSO pushed key/matrix ops onto the ring at
  route time, 'because the engine needs them too'…"*
- [faceplate-bridge.mjs:1-75](../../vxn-1b/crates/vxn1b-wasm/web/faceplate-bridge.mjs#L1-L75)
  — a 75-line prose header before the first import (33% of the file is comment),
  including a counterfactual proof and *"Getting the order right is free, so it
  is done; nothing more elaborate is warranted."* The ~6-line ordering invariant
  inside it is load-bearing and tested; the essay is not.
- [coordinator.mjs:39-40](../../vxn-1b/crates/vxn1b-wasm/web/coordinator.mjs#L39-L40)
  — *"…is not exotic, it is Tuesday."*

## Do not cut

Reviewed and explicitly cleared as concise statements of real constraints on
current behaviour:

- [coordinator.mjs:50-53](../../vxn-1b/crates/vxn1b-wasm/web/coordinator.mjs#L50-L53)
  — Safari has no render-thread slack and ignores `latencyHint`
  ([[vxn1-web-safari-audioworklet]]).
- [event-ring.mjs:247-249](../../vxn-1b/crates/vxn1b-wasm/web/event-ring.mjs#L247-L249)
  — byte loop instead of `subarray`, because JSC GC stalls the render thread.
- [event-ring.mjs:27-33](../../vxn-1b/crates/vxn1b-wasm/web/event-ring.mjs#L27-L33)
  — the block-writer overflow policy.
- [telemetry.mjs:14-19](../../vxn-1b/crates/vxn1b-wasm/web/telemetry.mjs#L14-L19)
  — why SAB and not `postMessage`.
- [vxn1b-processor.js:11-15](../../vxn-1b/crates/vxn1b-wasm/web/vxn1b-processor.js#L11-L15)
  — `performance.now()` historically absent from worklet scope, with the w3c
  issue link.
- [param-store.mjs:50-59](../../vxn-1b/crates/vxn1b-wasm/web/param-store.mjs#L50-L59)
  — the per-slot atomicity contract.
- The `/WHOLEARCHIVE` and `/OPT:REF` + `/INCLUDE:clap_entry` constraints in
  `CMakeLists.txt` (one sentence each, not the postmortem).
- The ring-before-store ordering invariant in `faceplate-bridge.mjs`.
- The `eval.rs` / `render.rs` / `bank.rs` factoring rationale, and the C1
  continuity argument in `mod_smoothing.rs:12-21`.
- [faceplate.css:1144-1146](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.css#L1144-L1146)
  — the specificity note. *"matches the row-major rule's specificity and then
  some, on purpose"* is exactly the kind of thing a later editor must not
  "simplify". Trim the tail, keep the warning.

## Acceptance criteria

- [ ] Every quoted block above is either trimmed to its constraint, moved to its
      ticket, or deleted.
- [ ] Nothing on the **Do not cut** list is shorter than it is today.
- [ ] `faceplate-bridge.mjs`'s header is under ~15 lines and starts with what
      the file does.
- [ ] Ticket-reference count across `vxn-1b/**` is materially down — record
      before/after in the close-out. This is a volume target, not a zero target:
      a ref beside a live ADR earns its place.
- [ ] No behaviour change whatsoever. This ticket touches only comments; a diff
      that changes a line of code has gone wrong.

## Notes

- Sequenced after [[0314]] on purpose. Correcting the *wrong* docs is a
  correctness fix and should land first; this one is taste, and taste can wait.
- The reviewers were unanimous on the rule and split on the volume. Some ticket
  refs are genuinely useful provenance — do not mechanise this into a regex
  sweep.
- Related: `util/drag.js:94-97` self-declares its mount form dead but argues to
  keep it (*"the next composite will want it and it is three lines"*). That is a
  legitimate keep. If the argument no longer convinces, delete the code — but do
  not leave the comment describing code that isn't there.
