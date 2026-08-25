---
id: "0284"
product: monorepo
title: "vxn-core-web: extract the shared browser-glue JS out of the vxn-1 / vxn-2 web ports"
priority: medium
created: 2026-08-24
epic: E045
depends: []
---

## Summary

First ticket of [E045](../../epics/open/E045-vxn1b-web-wasm-browser-port.md), and
the only one that touches shipped products. VXN1b is the **third** browser port;
six of the fourteen glue modules under
[vxn-1/crates/vxn-wasm/web/](../../vxn-1/crates/vxn-wasm/web/) and
[vxn-2/crates/vxn2-wasm/web/](../../vxn-2/crates/vxn2-wasm/web/) are already
duplicates of each other. Measured diff between the two ports:

| module | vxn-1 | vxn-2 | changed lines | real deltas |
|---|---|---|---|---|
| `midi-input.mjs` | 299 | 299 | **0** | none |
| `keyboard-input.mjs` | 236 | 236 | **0** | none |
| `preset-persistence.mjs` | 148 | 148 | 14 | comments only |
| `state-autosave.mjs` | 160 | 160 | 22 | comments only |
| `patch-io.mjs` | 210 | 210 | 36 | comments + `"VXN1 Patch"` ×3 |
| `preset-storage.mjs` | 152 | 148 | 48 | comments + `DB_NAME` + `DB_VERSION` |

Two are byte-identical; the other four differ only in comments plus **two**
pieces of configuration — the IndexedDB identity and the product patch name.
Forking them a third time for VXN1b is not defensible in a repo that already
runs `vxn-core-app`, `vxn-core-clap`, `vxn-core-ui-web` and a shared `vxn-dsp`.

Extract the six to `crates/vxn-core-web/assets/`, one physical copy, and repoint
both existing ports at it with their suites still green. No VXN1b code lands
here — 0285 onwards consumes the result.

The remaining eight modules (`event-ring`, `param-store`, `event-codec`,
`coordinator`, `controller`, `audio-host`, `host-runner`, `faceplate-bridge`)
stay forked: they encode per-synth model shape and diverge by 50–1296 lines.
Forcing them into one file would cost more than it saves.

## Design

### Resolution: injected seams

`dist/` is flat (xtask copies every `.mjs` side by side), so
[faceplate-bridge.mjs:31-38](../../vxn-1/crates/vxn-wasm/web/faceplate-bridge.mjs#L31-L38)
imports `./preset-persistence.mjs`. Once that module lives in
`crates/vxn-core-web/assets/`, the specifier that resolves in `dist/` no longer
resolves in the source tree where `node --test` runs. The repo has **no root
`package.json`** — the JS suites run on raw `.mjs` with no install step — so
bare specifiers / import maps are out, and checked-in symlinks are out (E013
Windows parity is open; a checkout without `core.symlinks` silently yields a
text file).

Resolution instead follows the seam idiom these modules already use
everywhere ([preset-persistence.mjs:29-45](../../vxn-1/crates/vxn-wasm/web/preset-persistence.mjs#L29-L45)
injects `openDB` / `getAllPresets` / `applyWrites`; `state-autosave` injects its
timers; `bootFaceplate` already injects `WebHostClass`):

- the bridges take the shared classes as **options**, defaulting to a dynamic
  `import("./x.mjs")` — the flat-dist path, reached only in the browser;
- the **generated page** and the **test files** are the only places that name a
  path, and each names its own correct one.

vxn-1's bridge already relies on this (its `coordinator.mjs` + input-adapter
imports are dynamic behind a `typeof document === "undefined"` guard). vxn-2's
`FaceplateBridge` is constructed against a fake document in its test, so for
that port the options must be real injection, not just a guarded default.

### Configuration

Two values, threaded explicitly rather than baked in:

- **IndexedDB identity** — `DB_NAME` (`vxn1-presets` / `vxn2-presets`) and
  `DB_VERSION` (2 / 1). These are per-DB migration history and **must not be
  unified**: vxn-1 users have live data under `vxn1-presets` at v2, and
  renumbering a shipped store is a needless eviction risk.
- **Product patch name** — `"VXN1 Patch"` / `"VXN2 Patch"`, in `patch-io`'s
  export default, its option default, and its rejection message.

`openPresetDB` gains an explicit DB descriptor; `PresetPersistence` /
`StateAutosave` gain `dbName` / `dbVersion` options they forward to their
`openDB` seam.

### Crate shape

A real workspace crate, mirroring `vxn-core-ui-web` (a crate whose substance is
`assets/*.js`): `Cargo.toml` + `src/lib.rs` exposing the module list and
`include_str!` sources, plus a test asserting no shared module contains a
product-specific string (`VXN1`, `VXN2`, `vxn1-presets`, …). That test is the
standing guard against the extraction being quietly undone.

Both `xtask web` module tables gain a source-root discriminator so shared
modules copy from `crates/vxn-core-web/assets/` into the same flat `dist/`.

## Acceptance criteria

- [ ] `crates/vxn-core-web/` exists, is a workspace member, and holds exactly one
      copy of `midi-input.mjs`, `keyboard-input.mjs`, `preset-storage.mjs`,
      `preset-persistence.mjs`, `state-autosave.mjs`, `patch-io.mjs`.
- [ ] Those six files are **deleted** from `vxn-1/crates/vxn-wasm/web/` and
      `vxn-2/crates/vxn2-wasm/web/` — `git ls-files` shows no duplicate.
- [ ] `cargo test -p vxn-core-web` passes, including a case that fails if a
      shared module contains a product-specific literal.
- [ ] The IndexedDB name/version and the patch product name are passed in by each
      port; no `vxn1`/`vxn2` literal survives in the shared sources.
- [ ] vxn-1 web suite: pass/fail set unchanged from a clean tree, verified by
      diffing against a stashed run (24/29 either side at the time; 29/29 once
      [0285](0285-web-param-mirror-drift.md) lands).
- [ ] vxn-2 web suite green, unchanged pass count — run it with vxn-2's
      `xtask web` output present so it reports `skipped 0`, per 0285's note.
- [ ] `cargo run -p vxn-xtask -- web` and `cargo run -p vxn2-xtask -- web` each
      still produce a complete `target/web-dist/` with the same file list as
      before (shared modules land flat, from the new source root).
- [ ] `cargo test --workspace` green.

## Found while doing this

vxn-1's web suite was **already red on `main`** before any of this work: five of
its 29 tests died on `controller TOTAL_PARAMS 167 != JS mirror 165`. vxn-2 had
the same class of drift (209 vs 208), hidden because its wasm-backed tests skip
unless `target/web-dist` happens to hold *that port's* bundle.

Both are split out as **[0285](0285-web-param-mirror-drift.md)** and fixed there,
not here — they are vxn-1/vxn-2 product bugs from `9b5d222` / `3630407`, and
burying them in an extraction diff would hide two broken browser builds. With
0285 applied, both suites are fully green (29/29 and 89/89, zero skipped), which
is what makes this ticket's "pass/fail set unchanged" gate meaningful rather than
a comparison of two red runs.

## Notes

- **Land alone.** This modifies two shipped browser ports; it should not share a
  commit with VXN1b work. Both node suites are the regression gate.
- Do not run concurrent `cargo test` invocations here —
  [[vxn-no-parallel-cargo-test]]. One run, captured to a file, then grep.
- No `cargo fmt` — [[vxn-no-cargo-fmt]]; hand-format the new Rust.
- Stage explicit paths, never `git add -A` — [[vxn-concurrent-vxn2-work-no-git-add-all]].
- Out of scope: the eight divergent modules; any behaviour change; VXN1b's own
  `web/` directory (0285+); the `key-mode.mjs` module (vxn-1-only, no vxn-2 twin
  to share with, and VXN1b's key state is a different shape).

## Close-out (2026-08-25)

- `crates/vxn-core-web/` created and added to the workspace members + path
  dependencies. Holds exactly the six shared modules in
  [assets/](../../crates/vxn-core-web/assets); `git ls-files` shows one copy of
  each — the vxn-1 and vxn-2 duplicates are gone.
- Config threaded rather than baked: `openPresetDB(indexedDB, db)` now REQUIRES a
  `{ name, version }` identity and rejects a missing one;
  `PresetPersistence` / `StateAutosave` forward it via a new `dbId` option;
  `patch-io` takes `product` for its default filename and rejection message.
  Each bridge exports its own `DB_ID` / `PRODUCT`, and the port tests import
  those so they open the database the browser actually opens.
- Resolution by injected seams: both bridges dropped their static imports for a
  lazy `loadGlue()` over the flat-dist siblings, overridable per call. vxn-2's
  `_attachInputs` became async as a result and `boot()` awaits it.
- `cargo test -p vxn-core-web` — 4 pass, incl.
  `tests::no_shared_module_hardcodes_a_per_port_config_value` (a grep sweep for
  `vxn1-presets` / `VXN2 Patch` / … over every shared source) and
  `tests::open_preset_db_refuses_to_guess_a_database`.
- Both xtask module tables split into local + `CORE_MODULES`; verified
  `cargo run -p vxn1-xtask -- web` → 24 files and
  `cargo run -p vxn2-xtask -- web` → 20 files, 6/6 shared modules present in each.
- Suites: vxn-1 web 29/29, vxn-2 web 89/89, both 0 skipped;
  `cargo test --workspace` 1543 pass / 0 fail at the time of landing.
- Turned up two pre-existing product bugs, both split out rather than bundled:
  [[0285]] (param mirror drift, both ports dead) and, later, [[0296]].
