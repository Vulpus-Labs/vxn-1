---
id: E020
product: vxn-2
title: "vxn-1 web port — perf, cross-browser, ship"
status: open
created: 2026-06-14
depends-on: E016
---

> **Depends on E016** (build pipeline) and effectively gates on the whole
> stack being playable (E015/E017/E018). The closing epic: prove the port
> performs at full polyphony in real browsers, on a cross-browser matrix,
> and ship it with CI + deploy. Detail firms up once there is an
> end-to-end playable build to measure.

## Goal

Take the working port to shippable: full 16-voice polyphony glitch-free in
target browsers (incl. mobile), SIMD128 perf measured and tuned, the one
denormal case the 0034 spike didn't isolate confirmed safe, a cross-browser
support matrix, and CI that builds + deploys the wasm bundle.

When this epic closes:

- Full polyphony renders without xruns at the chosen block size on the
  target browser baseline; measured headroom documented.
- SIMD128 perf is measured vs scalar; build flags settled.
- The held-quiet-sustain-into-reverb denormal case is verified (manual FTZ
  flush added only if measurement demands it).
- A cross-browser/-device support matrix is published (Chrome/Firefox/
  Safari, desktop + mobile).
- CI builds the bundle and deploys to a static host; optional PWA/offline.

## Why last

Perf and cross-browser truth only mean something against a complete,
playable build. The 0034 spike measured Node throughput (~55× realtime,
1 voice) — indicative, not the browser truth at 16 voices with WASM
SIMD128 (which is weaker than NEON). This epic measures reality and
closes the gaps.

## Scope

**In:**

- SIMD128 build verification + perf measurement (16-voice poly, worst-case
  patches) on desktop and mobile; tune block size / lookahead / latency.
- Glitch/xrun stress: sustained chords + automation + FX tails under load.
- Denormal: stress held-quiet-sustain into reverb feedback (FTZ absent on
  wasm); add a targeted manual flush only if a CPU cliff appears
  (`vxn1-silent-skip-filter-state` covers the release case already).
- wasm size optimisation (release, `wasm-opt`, feature trimming) to the
  extent it affects load time.
- Cross-browser/-device matrix: AudioWorklet, Atomics/SAB, Web MIDI,
  storage behaviour per browser; document support + fallbacks.
- CI: build the web bundle as an artifact; deploy to a static host.
- Optional: PWA manifest + service worker for offline/install.

**Out:**

- New DSP features or engine changes beyond a denormal flush if required.
- Native plugin/standalone work (separate epics).

## Planned tickets

> Ids assigned at scaffold time, against a playable build. Provisional set:

- [ ] SIMD128 build + perf measurement (16-voice, desktop + mobile).
- [ ] Glitch/xrun stress + latency/block-size tuning.
- [ ] Denormal stress (held-quiet-sustain → reverb) + flush if needed.
- [ ] wasm size optimisation (wasm-opt, trimming).
- [ ] Cross-browser/-device support matrix + fallbacks doc.
- [ ] CI build + static-host deploy.
- [ ] (optional) PWA manifest + offline service worker.

## Risks

- **WASM SIMD128 ≠ NEON.** Auto-vectorisation to SIMD128 is weaker; full
  poly may need headroom work or voice-count scaling on weak devices.
- **Mobile.** Battery/thermal throttling and smaller core budgets; mobile
  may need a reduced default polyphony.
- **Safari.** Worklet, Atomics, Web MIDI, and storage quirks concentrate
  here; the matrix may force fallbacks (e.g. keyboard-only input on
  Safari if Web MIDI is absent).
- **Denormals at scale.** The spike's release-path result is clean; a
  held quiet signal into reverb feedback is the untested case.

## Acceptance

- Full 16-voice polyphony renders glitch-free at the documented baseline,
  with measured headroom.
- SIMD128 perf measured and build flags settled.
- The held-quiet denormal case is verified safe (with a flush if needed).
- A published cross-browser/-device matrix with fallbacks.
- CI builds and deploys the bundle; the deployed page plays end-to-end.
