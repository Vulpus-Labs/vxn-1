---
id: "0093"
product: vxn-2
title: "(optional) PWA manifest + offline service worker"
priority: low
created: 2026-06-22
epic: E020
depends: ["0092"]
---

## Summary

Optional closing ticket of
[E020](../../epics/open/E020-web-perf-crossbrowser-ship.md). Adds a PWA manifest
+ a service worker so the deployed app is installable and works offline. The
bundle is already fully static and self-contained
([xtask main.rs:168-246](../../vxn-1/xtask/src/main.rs#L168)) — every asset is a
plain file under `web-dist/`, which is exactly what a cache-first service worker
needs. This is a nice-to-have; the epic closes with or without it.

## Design

- **Manifest.** A `manifest.webmanifest` (name, icons, `display: standalone`,
  theme) emitted into `web-dist/` by the `web` xtask step alongside `_headers`
  ([main.rs:246](../../vxn-1/xtask/src/main.rs#L246)), and linked from the
  generated `index.html` (the page comes from `gen-web-page`,
  [main.rs:294-315](../../vxn-1/xtask/src/main.rs#L294) — either the manifest link
  is injected there or appended by xtask).
- **Service worker.** A cache-first `sw.js` that precaches the known bundle file
  list — the same curated module set xtask already enumerates
  ([MODULES array, main.rs:183-214](../../vxn-1/xtask/src/main.rs#L183)) plus both
  `.wasm`, `index.html`, `factory.bin`. Single source the file list so it can't
  drift from what the bundle ships.
- **COOP/COEP interaction.** The service worker must not strip the isolation
  headers on cached responses — cross-origin isolation must survive an offline
  load or SAB breaks. Verify the cached document still reports COOP/COEP (the
  SW serves from cache but the headers must persist; on some hosts the SW has to
  re-add them).
- **Scope.** Register the SW only on a secure origin (https / localhost). Keep it
  behind a feature so a deploy can ship without it if the isolation-vs-SW
  interaction proves fragile.

## Acceptance criteria

- [ ] (headless) `cargo xtask web` emits `manifest.webmanifest` + `sw.js` into
      `web-dist/`, and the precache list matches the actual bundled files
      (a test/assert that the SW list == the MODULES set + wasm + page + factory).
- [ ] (MANUAL) The deployed app passes an "installable PWA" check (Chrome
      devtools / Lighthouse PWA audit: manifest valid, SW registers, installable).
- [ ] (MANUAL) Load once online, go offline, reload: the app boots and plays from
      cache.
- [ ] (MANUAL) Offline-loaded document still reports COOP/COEP and constructs a
      `SharedArrayBuffer` (isolation survived the SW).

## Notes

- Optional per the epic Scope; skip if the SW/isolation interaction is fragile.
- Depends on 0092 (a real deployed origin to install/serve the SW from).
- Out of scope: push notifications, background sync — not needed for a synth.
