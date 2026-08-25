---
id: "0285"
product: monorepo
title: "Web ports' JS param mirrors drifted behind the engines — vxn-1 and vxn-2 browser builds are both broken"
priority: high
created: 2026-08-25
epic: null
depends: []
---

## Summary

Both browser ports fail to boot. Each web port declares its param-space size once
in JS and reconciles it against the wasm at controller-instantiate time; both
declarations are stale, so that reconciliation throws and the page never reaches
audio:

| port | wasm says | JS mirror said | throws at |
|---|---|---|---|
| vxn-1 | `TOTAL_PARAMS` 167 | 165 | [controller.mjs:188](../../vxn-1/crates/vxn-wasm/web/controller.mjs#L188) |
| vxn-2 | `TOTAL_PARAMS` 209 | 208 | [controller.mjs:183](../../vxn-2/crates/vxn2-wasm/web/controller.mjs#L183) |

The handshake did its job — it refuses to run on a layout it can't trust, rather
than silently addressing the wrong slots. What failed is that nobody re-ran the
web suites after the commits that moved the counts.

**Cause, both ports: the same feature pair.**

- vxn-1 — [`9b5d222`](#) *phaser stereo spread + delay ping-pong (0277-0279)*
  added `PhaserStereo` and `DelayPingPong` to `GlobalParam`, taking
  `GLOBAL_COUNT` 27 → 29. Both were **inserted mid-enum**, so every global clap
  id from `PhaserStereo` on shifted up. The same commit's 0278 bumped
  `vxn_app::state::VERSION` 1 → 2 for the longer global block.
- vxn-2 — [`3630407`](#) *phaser stereo spread + faceplate ping-pong toggle
  (0280)* added one param, `TOTAL_PARAMS` 208 → 209, **appended**, so no
  existing id moved. `BLOB_VERSION` stayed 1.

Found while extracting the shared browser glue in
[0284](0284-vxn-core-web-shared-browser-glue.md); confirmed pre-existing on a
clean tree in both ports, so it is not fallout from that work.

## Design

The JS constant is a *declared mirror*, deliberately hand-maintained: the browser
has no build step that could read the Rust table, and the runtime handshake is
what makes the mirror safe. So the fix is to correct the mirrors, not to
re-engineer the reconciliation — it already worked.

vxn-1's blob-version bump has a second, quieter victim: `faceplate-bridge.test.mjs`
hand-builds a state blob with `u32le(1)`. `restore` rejects any version but the
current one, so a stale value there surfaces as "the factory preset silently
failed to apply" — four assertions deep in the factory round-trip, not as a
header error. That masking is why it was worth chasing rather than deleting.

## Acceptance criteria

- [x] vxn-1 `event-codec.mjs` declares `GLOBAL_COUNT = 29` / `TOTAL_PARAMS = 167`.
- [x] vxn-2 `event-codec.mjs` declares `TOTAL_PARAMS = 209`.
- [x] `faceplate-bridge.test.mjs`'s hand-built state blob carries
      `vxn_app::state::VERSION` = 2.
- [x] Every hardcoded count in the suites tracks the new values (`param-store`,
      `event-codec`, `controller` id-layout and last-id assertions).
- [x] Comments quoting the old totals corrected in `param-store.mjs`,
      `coordinator.mjs`, `controller.mjs` (both ports).
- [x] vxn-1 web suite **29/29, 0 skipped**.
- [x] vxn-2 web suite **89/89, 0 skipped** — see the note below on skips.
- [x] Golden wire tables untouched: both ports' JS golden rows still match their
      Rust twin in `codec.rs` (vxn-2's id-208 sample is still a valid id).

## Notes

- **The vxn-2 suite hides failures by default.** Its wasm-backed tests are
  `{ skip: !HAVE }` on `target/web-dist/vxn2_web_controller.wasm` existing. A
  plain `node --test` on a tree that has never run `xtask web` — or that last ran
  **vxn-1's** `xtask web`, since both ports write the same `target/web-dist` and
  wipe it first — reports "89 pass" with 13 silently skipped, including every
  test that would have caught this. Run `cargo run -p vxn2-xtask -- web` first
  and check the suite reports `skipped 0`. Worth its own follow-up: a green run
  that skips its only integration coverage is a bad default.
- No `cargo fmt` — [[vxn-no-cargo-fmt]]. No `git add -A` —
  [[vxn-concurrent-vxn2-work-no-git-add-all]].
- Out of scope: making the mirror generated rather than declared; the shared-dist
  collision between the two ports' `xtask web` (noted above).
