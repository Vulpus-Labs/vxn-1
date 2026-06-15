---
id: E016
product: vxn-1
title: "vxn-1 web port — host shell & build pipeline"
status: closed
created: 2026-06-14
depends-on: E015
---

> **Depends on E015.** This epic builds the main-thread shell around the
> event core: the AudioContext bootstrap, the wasm build/bundle pipeline,
> and the cross-origin-isolated serving that E015's `SharedArrayBuffer`
> requires. It is the web analogue of `vxn-clap` minus the audio-thread
> event loop (which lives in E015). Some tickets firm up once E015's
> controller-placement ADR (0036) lands — they are listed below but their
> detail is deliberately thin until then (spike-scaffold rhythm).

## Goal

Make the web synth launchable and buildable: a reproducible
`cargo xtask web` that compiles and bundles the wasm + JS + assets into a
servable directory, an AudioContext/worklet bootstrap on the main thread,
and a dev server that sets the COOP/COEP headers so `SharedArrayBuffer`
works. The "host" that wires the controller to the E015 transport.

When this epic closes:

- `cargo xtask web` produces a self-contained `dist/` (wasm, JS glue,
  worklet, faceplate assets) with one command.
- Opening the served page boots an AudioContext, instantiates the worklet
  from E015, and reaches the "audio live" state (autoplay-unlock handled).
- The build emits with SIMD128 enabled (perf measurement is E020; the
  flag belongs to the pipeline).
- A documented production-hosting recipe sets COOP/COEP correctly.

## Why a dedicated build epic

The spike copied a `.wasm` by hand and used `python3 -m http.server`.
A real port needs: deterministic builds, asset bundling, the isolation
headers baked into both dev and prod, and a single entrypoint that the
other epics (UI, input, persistence) plug into. Decoupling this from the
event core keeps E015 focused on the hard transport problem.

## Scope

**In:**

- `xtask web` subcommand: build the wasm crate(s) for `wasm32-unknown-unknown`
  (release, SIMD128), run any post-processing, assemble `dist/`.
- Main-thread coordinator module: create AudioContext, add the worklet
  module, instantiate the `AudioWorkletNode`, hand it the wasm bytes
  (per the spike pattern), wire it to the E015 transport + controller.
- AudioContext lifecycle: user-gesture autoplay unlock, suspend/resume,
  device-change handling, teardown.
- Dev server (or static-server config) emitting `Cross-Origin-Opener-Policy:
  same-origin` + `Cross-Origin-Embedder-Policy: require-corp`.
- Production hosting doc: the same headers on a static host / CDN, and the
  CORP implications for any cross-origin assets.

**Out:**

- The event ring / param store / audio-host (E015).
- wasm size optimisation beyond enabling release+SIMD (E020).
- CI artifact builds + deploy automation (E020).
- The faceplate itself (E018).

## Planned tickets

> Ids assigned at scaffold time (after E015's 0036 ADR). Provisional set:

- [ ] `xtask web` build + bundle to `dist/` (release, SIMD128).
- [ ] Main-thread coordinator: AudioContext + worklet bootstrap, wired to
      the E015 transport.
- [ ] AudioContext lifecycle: autoplay unlock, suspend/resume, device
      change, teardown.
- [ ] COOP/COEP dev server + production hosting doc.
- [ ] (conditional on 0036) main-thread controller wasm module + JS glue,
      *or* the JS controller shell — whichever 0036 selects.

## Risks

- **Autoplay policy.** AudioContext starts suspended until a user gesture;
  the bootstrap must gate on a click and resume cleanly.
- **Isolation headers break embedding.** COOP/COEP can break iframing and
  third-party assets; the hosting doc must call this out.
- **Build reproducibility.** wasm + JS bundling across dev machines and CI
  needs pinned tool versions; avoid a bespoke chain that rots.

## Acceptance

- `cargo xtask web` yields a servable `dist/` in one command.
- The served page boots audio to "live" after a user gesture, with the
  E015 worklet running.
- `SharedArrayBuffer` is available on the served page (isolation headers
  verified).
- Production hosting recipe documented and validated on at least one
  static host.
