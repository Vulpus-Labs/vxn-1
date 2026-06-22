---
id: "0092"
product: vxn-2
title: "CI build + static-host deploy"
priority: high
created: 2026-06-22
epic: E020
depends: ["0090", "0091"]
---

## Summary

Sixth ticket of [E020](../../epics/open/E020-web-perf-crossbrowser-ship.md).
Wires CI to build the web bundle as an artifact and deploy it to a static host.
`cargo xtask web` already produces a fully self-contained `target/web-dist/`
(both wasm modules, transport JS, faceplate page, baked factory bank, and a
Netlify/Cloudflare `_headers` file with the COOP/COEP the SAB transport needs —
[xtask main.rs:168-246](../../vxn-1/xtask/src/main.rs#L168)). This ticket makes
that happen on every push and serves the result.

## Design

- **Build job.** A GitHub Actions workflow that installs the
  `wasm32-unknown-unknown` target, runs the headless test gates
  (`cargo test`, the node web suites, the 0087 bench unit tests, the
  `vxn-ui-web/assets` vitest suite), then `cargo xtask web` (release + SIMD128 +
  the 0090 `wasm-opt` pass), and uploads `target/web-dist/` as a build artifact.
- **Deploy.** Push `web-dist/` to a static host that honours `_headers`
  (Netlify or Cloudflare Pages — both read the file the bundle already emits at
  [main.rs:241-246](../../vxn-1/xtask/src/main.rs#L241), so COOP/COEP/CORP ride
  along with no host-specific config). Cross-origin isolation is mandatory:
  without it `SharedArrayBuffer` is not constructible and the worklet transport
  can't boot.
- **Headers verification.** A deploy-time / smoke check that the served document
  actually returns `Cross-Origin-Opener-Policy: same-origin` +
  `Cross-Origin-Embedder-Policy: require-corp` (the three headers
  `web_dist_headers` writes, [main.rs:320-325](../../vxn-1/xtask/src/main.rs#L320));
  a curl-for-headers step catches a host that silently drops `_headers`.
- The xtask `web` build runs `gen-web-page` and `bake-factory` as subprocesses
  ([main.rs:264-315](../../vxn-1/xtask/src/main.rs#L264)) — CI must allow those
  cargo `run` invocations (they need the full workspace, not just the wasm
  target).

## Acceptance criteria

- [ ] (headless) A CI workflow file builds `target/web-dist/` from a clean
      checkout (target install + `cargo xtask web`) and uploads it as an
      artifact; the run is green.
- [ ] (headless) CI runs the full headless gate before bundling: `cargo test`,
      node web suites, vitest — a failure blocks the deploy.
- [ ] (MANUAL / CI) The bundle deploys to the chosen static host and the page
      loads; `curl -I` on the deployed URL shows the COOP + COEP headers.
- [ ] (MANUAL) The deployed page plays end-to-end: boot, play a note via
      keyboard, load a factory preset (cross-ref 0062), hear audio.

## Notes

- Depends on 0090 (the `wasm-opt` pass CI should run) and 0091 (the support
  matrix that bounds what "plays end-to-end" is verified against).
- Secrets (deploy token) are a manual one-time setup; document the required
  secret name in the workflow.
- Out of scope: PWA/offline (0093).
