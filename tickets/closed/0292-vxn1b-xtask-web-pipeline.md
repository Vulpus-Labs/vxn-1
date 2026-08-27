---
id: "0292"
product: vxn-1b
title: "vxn1b-xtask web: one command to a servable dist/"
priority: medium
created: 2026-08-25
epic: E045
depends: ["0286", "0287", "0288", "0289", "0290", "0291"]
---

## Summary

Eighth ticket of [E045](../../epics/open/E045-vxn1b-web-wasm-browser-port.md).
Every piece of the browser port now exists as source; nothing assembles them.
This is the build target that turns the tree into a directory a static host can
serve:

```
cargo run -p vxn1b-xtask -- web [--serve] [--port N]
```

Ports vxn-2's [`web()`](../../vxn-2/xtask/src/main.rs#L114), which is the closest
relative and already handles the two-wasm + shared-modules shape.

## Design

`target/web-dist/`, rebuilt from scratch each run:

- **Two wasm modules**, `wasm32-unknown-unknown`, release + `-C target-feature=+simd128`
  (appended to any caller `RUSTFLAGS`, not clobbering it): `vxn1b_wasm.wasm`
  (the worklet engine) and `vxn1b_web_controller.wasm` (the main-thread model).
- **The production JS**, curated by hand so the `*.test.mjs` suites stay out of
  the bundle: `event-ring`, `event-codec`, `param-store`, `telemetry`,
  `audio-host`, `host-runner`, `coordinator`, `controller`, `faceplate-bridge`,
  and the `vxn1b-processor.js` worklet.
- **`index.html`** from `cargo run -p vxn1b-ui-web --bin gen-web-page`, so the
  param-descriptor JSON stays single-sourced and byte-identical to the plugin's
  faceplate — xtask carries no wry dependency and no copy of the splice.
- **`_headers`** with COOP/COEP (+CORP) on every path, so dropping `dist/` on a
  static host gives the cross-origin isolation `SharedArrayBuffer` needs with no
  extra config.
- **`--serve`** hands the bundle to `serve-coep.mjs` with the same two headers
  locally.

### No `factory.bin`

vxn-1 and vxn-2 bake one; VXN1b does not.
[0290](../closed/0290-vxn1b-web-controller-cdylib.md) embeds the factory bank in
the controller wasm via `include_dir!` and publishes the corpus during
`vxnc_new()`, because the reason vxn-1 baked an asset (ticket 0062: keep the DSP
engine out of a lean controller wasm) does not apply — this controller links
`vxn1b-engine` for `SharedParams` regardless. Verified: the preset names are
present in the release wasm, which is *smaller* than vxn-2's controller
(772 762 vs 831 258 bytes) while carrying its bank inline.

So this ticket has no bake step and `dist/` has no `factory.bin`, and there is
no boot fetch to fail.

### The shared modules are not needed yet

vxn-2's bundle copies six shared modules from
[`crates/vxn-core-web/assets`](../../crates/vxn-core-web/assets) (persistence ×4,
input ×2). Nothing in VXN1b's tree imports them yet — a grep of every production
module's imports resolves entirely within `vxn1b-wasm/web/`. They arrive with
[0293](0293-vxn1b-browser-persistence.md) (persistence) and
[0294](0294-vxn1b-input-adapters-ship.md) (MIDI + keyboard), and each should add
its own to the copy list rather than this ticket shipping files nothing loads.

### `serve-coep.mjs` — a third copy, knowingly

vxn-1's and vxn-2's differ only in comments and one MIME entry. This adds a
third. That is against [E045](../../epics/open/E045-vxn1b-web-wasm-browser-port.md)'s
"don't fork a third time" instinct, but the epic's rule was scoped to the ~14
transport/glue modules that encode model shape; this is a 61-line dev-only static
server with no product knowledge in it. Hoisting all three into
`crates/vxn-core-web/` is the right end state and is a follow-up, not this
ticket — it would touch two shipped ports for a file that never reaches a user.

## Acceptance criteria

- [ ] `cargo run -p vxn1b-xtask -- web` produces `target/web-dist/` containing
      both wasm modules, the nine production JS modules, `index.html` and
      `_headers` — and nothing else (no test files, no `factory.bin`).
- [ ] A test or the command itself fails loudly if a listed module is missing,
      rather than emitting a bundle that 404s in the browser.
- [ ] The two wasm modules are built with `simd128`, and a caller's existing
      `RUSTFLAGS` survives.
- [ ] `--serve` serves the bundle cross-origin-isolated; `crossOriginIsolated`
      is `true` in the page and `SharedArrayBuffer` is constructible.
- [ ] `index.html` is generated, not copied — its param JSON matches the
      plugin's (`web_page_params_are_byte_identical_to_native` already pins the
      generator).
- [ ] `--help` documents the target.
- [ ] The bundle is self-contained: served from a clean directory with no
      network, the page boots to "audio live" after a gesture.

## Notes

- [[vxn2-xtask-flat-workspace]]: `vxn-1b/xtask`'s `workspace_root()` needs two
  `.parent()` calls, same as vxn-2's — the xtask lives at `vxn-1b/xtask`, not at
  the workspace root.
- The last acceptance criterion depends on [0291](0291-vxn1b-faceplate-rewire.md)
  finishing its boot entry: the generated page loads `faceplate-bridge.mjs` as a
  module and expects it to self-boot and drain `window.__VXN_UI_QUEUE__` (the
  queuing `window.ipc` stub in
  [`WEB_BOOT_HEAD`](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L367)). The
  bundle can be built and inspected before that lands; it will not *play* until
  it does.
- No `cargo fmt` — [[vxn-no-cargo-fmt]]. Stage explicit paths —
  [[vxn-concurrent-vxn2-work-no-git-add-all]].

## Close-out (2026-08-27)

- `cargo run -p vxn1b-xtask -- web` produces `target/web-dist-vxn1b/`: both wasm
  modules, the nine `WEB_MODULES`, the worklet, the eight `CORE_MODULES`,
  `index.html` and `_headers`. No test files, no `factory.bin` (VXN1b's bank is
  embedded in the controller wasm).
- Loud failure on a missing module, three ways
  ([main.rs:990+](../../vxn-1b/xtask/src/main.rs#L990)):
  `every_bundled_module_exists`, `the_bundle_is_closed_under_its_own_references`
  (scans `"./x.mjs"` literals; guarded against a vacuous pass by a `found >= 6`
  floor), `no_test_files_are_bundled`, `no_factory_asset_is_expected`.
  The `web` fn itself also errors before copying if a source path is absent.
- `simd128` is **appended** to a caller's `RUSTFLAGS`, never assigned
  ([main.rs:924-932](../../vxn-1b/xtask/src/main.rs#L924-L932)).
- `_headers` carries COOP + COEP + CORP, asserted by
  `the_headers_carry_both_isolation_directives`.
- `index.html` is generated by `gen-web-page`, not copied, so the param JSON is
  the same splice the plugin's editor does — pinned by
  `web_page_params_are_byte_identical_to_native`.
- `--help` documents the target; corrected here — it still named
  `target/web-dist/`, which is vxn-2's directory, not VXN1b's.
- **Not verified here:** the browser half — `crossOriginIsolated === true`,
  `SharedArrayBuffer` constructible under `--serve`, and the clean-directory
  boot to "audio live". The headers and bundle closure that make those work are
  pinned above.
