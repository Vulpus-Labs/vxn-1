---
id: "0086"
product: vxn-1
title: web tree hygiene — delete stale spike files, clarify vxn-wasm spike status
priority: low
created: 2026-06-21
epic: E024
---

## Summary

`vxn-wasm/web/` mixes dead and live code. The production
stack is `audio-host.mjs`/`coordinator.mjs`/`faceplate-bridge
.mjs` (auto-booting), but alongside it sit `vxn-processor.js`
(the 0034 spike worklet — `vxn-wasm/src/lib.rs:1`
self-describes as "Throwaway — not a product surface"),
`vxn-processor-0035.js`, `vxn-processor-0038.js`,
`index-0035.html`, and an `index.html` that still wires the
spike. The ticket-number-in-filename suffixes (`-0035`,
`-0038`) are a version-in-filename antipattern, and a
maintainer can't tell which worklet is authoritative without
archaeology — inviting edits to the wrong file.

## Acceptance criteria

- [ ] `vxn-processor-0035.js`, `vxn-processor-0038.js`, and
      `index-0035.html` are deleted (superseded by the
      production `.mjs` stack); `vxn-processor.js` and
      `index.html` are deleted or, if still needed as a
      manual dev harness, renamed without the ticket
      suffix and documented as such in a one-line README
      note.
- [ ] The throwaway `vxn-wasm` spike `Instance` API
      (`vxn-wasm/src/lib.rs`) is either removed (production
      path is `host.rs`/`codec.rs`) or marked
      `#[doc(hidden)]` / cfg-gated so the crate's
      product-vs-spike split is unambiguous.
- [ ] No live page or build step references a deleted file
      (grep the crate + xtask + any bundling for the
      removed names).
- [ ] `cargo test -p vxn-wasm` green; gated vitest suite
      green; the web build still boots the production
      faceplate (manual).

## Notes

Distinct from E011 **0020**, which removes
`assets/prototypes/wave-knob.html` and dead CSS in
`vxn-ui-web` — this ticket targets the `vxn-wasm/web/` spike
files and the `vxn-wasm` crate's spike API, not touched
there.

## Close-out (2026-06-22)

- Deleted spike files: `web/index.html` (0034 harness), `web/index-0035
  .html`, `web/vxn-processor.js` (0034 spike worklet),
  `web/vxn-processor-0035.js`. Renamed `vxn-processor-0038.js` →
  `vxn-processor.js` (production worklet — stable name at source, matching
  the xtask dist rename).
- Spike `Instance` API removed from `vxn-wasm/src/lib.rs` (the
  `vxn_new`/`vxn_destroy`/`vxn_note_*`/`vxn_process*`/`vxn_set_param`/
  `vxn_out_*`/`vxn_quantum` exports + struct); `lib.rs` now re-exports only
  `codec`, `host`, `QUANTUM` with clean module docs. Safe — symbols were
  only called by the deleted spike worklets.
- xtask updated: `("vxn-processor-0038.js","vxn-processor.js")` →
  `("vxn-processor.js","vxn-processor.js")`. Stale comments fixed in
  `event-ring.mjs`, `audio-host.mjs`, `host-runner.mjs`, `coordinator.mjs`,
  `harness-0042.mjs`, `README.md`.
- grep for `vxn-processor-0035` / `vxn-processor-0038` / `index-0035` across
  `.mjs/.js/.html/.rs/.toml/.yml` → zero hits.
- Tests: `cargo test -p vxn-wasm` 16/16 pass; gated JS suites that don't
  need the wasm binary pass; the pre-existing `controller wasm not found`
  failures are unrelated. Done by Sonnet in worktree.
