---
id: E037
product: vxn-1b
title: "VXN1b FX section — dynamics kernel + tab-switched serial chain (chorus/phaser/delay/reverb/dynamics)"
status: closed
created: 2026-07-25
---

## Goal

Consolidate VXN1b's effects into a single tab-switched FX section — the FX
engine work from [ADR 0001](../../vxn-1b/adrs/0001-vxn1b-overall-design.md) §8.
Four of the five kernels already ship in the shared `vxn-dsp`; the one new
kernel is **Dynamics**, copied from VXN2.

When this epic closes:

- The **Dynamics** block from VXN2 is copied into the shared `vxn-dsp` crate
  (additive; VXN1 unaffected), **minus its dedicated oversampling** — it runs at
  the single global 1×/2×/4× rate like the rest of the instrument.
- All five effects — Chorus, Phaser, Delay, Reverb, Dynamics — are wired into a
  serial FX chain in `vxn1b-engine` with per-effect on/off + wet params,
  governed by the global oversample rate. (The UI tab strip is [[E038]]; this
  epic is the engine wiring.)

Depends on [[E036]] (needs the param table + block-render loop to extend). All
modulation sources — incl. MPE aftertouch and note-on random — already land in
[[E036]]; this epic is FX only.

## Why now

Dynamics is the one new kernel and the tabbed chain is a fresh engine section
distinct from E036's synthesis core, so it groups cleanly on its own after the
spine is DAW-playable. Per-effect wet as a matrix *destination* (ADR §2) stays a
candidate — the blocks ship here regardless.

## Design (locked by ADR 0001)

- **Dynamics.** Copy `vxn-2/crates/vxn2-dsp/src/dynamics.rs` into `vxn-dsp`;
  delete its internal oversampling stage; it runs at whatever the global OS rate
  is. Additive to `vxn-dsp` (VXN1 does not route it). Unit tests for the block.
- **FX chain.** Serial: synth → chorus → phaser → delay → reverb → dynamics →
  master (order lockable during the ticket). Chorus/Phaser/Delay/Reverb kernels
  already exist in `vxn-dsp`. Per-effect on/off + wet/mix params. Off-path is a
  true skip, not a wet=0 multiply through the DSP.

## Planned tickets

Chain: **0206 → 0207**. (0205 was taken by an E036 ticket; real IDs shifted +1.)

- [ ] **0206** — Dynamics kernel into `vxn-dsp`. Copy VXN2's `dynamics.rs`;
      adapt to `vxn-dsp` conventions (the kernel is already rate-agnostic — no OS
      stage to remove; VXN2's oversampling lives in its *engine*, not the kernel,
      and is not copied). Additive — VXN1 build/tests unaffected. Unit tests
      (gain-reduction curve, attack/release, bypass identity). Run both VXN1 and
      VXN1b test suites (shared crate; `vxn-no-parallel-cargo-test`).
- [ ] **0207** — FX chain wiring in `vxn1b-engine`. Serial chorus → phaser →
      delay → reverb → dynamics at the global OS rate; per-effect on/off + wet
      params (added to the E036 table); default patch FX off/neutral. Tests: each
      effect bypasses to identity when off; chain runs alloc-free.

## Risks

- **Shared-crate blast radius.** Adding `dynamics.rs` to `vxn-dsp` touches a
  crate VXN1 depends on. Keep it purely additive (new module, no edits to shared
  paths) and run VXN1's suite to prove no regression.
- **Dynamics without its OS.** VXN2's dynamics may rely on internal oversampling
  for clean gain reduction; running at 1× could alias on fast transients. Accept
  per ADR (global OS is the lever); verify at the default 2× that it's clean
  enough, note if 1× needs a caveat.
- **FX chain CPU.** Five serial effects at oversampled rate adds cost; keep each
  block's off-path a true skip, not a wet=0 multiply through the DSP.

## Acceptance

- Dynamics lives in `vxn-dsp`, runs at the global OS rate, and VXN1's suite is
  green (no shared-crate regression).
- All five effects route in a serial chain with on/off + wet; each bypasses to
  identity when off; the chain is allocation-free.

## Close-out (2026-07-29)

Shipped in two tickets, landed on `main`.

- **0206 — Dynamics kernel into `vxn-dsp`.** VXN2's `DynamicsBlock` (peak comp →
  `tanh` saturator) copied verbatim into
  [dynamics.rs](../../vxn-1/crates/vxn-dsp/src/dynamics.rs) — additive module, no
  edits to existing kernels, VXN1 unaffected. The only real port work was dep
  adaptation (`crate::smoother` → `crate::smoothing`; two bit-exact test helpers
  inlined). **The epic's "delete its dedicated oversampling" turned out to be a
  no-op:** VXN2's oversampling lives in its *engine* (`run_dynamics_os`), not the
  kernel, which is already rate-agnostic — so it runs at the global OS rate for
  free. 7/7 dynamics tests, full vxn-dsp suite 90/90, VXN1 consumer builds clean.
  Commit `361fabe`.
- **0207 — Serial FX chain in `vxn1b-engine`.** New
  [fx.rs](../../vxn-1b/crates/vxn1b-engine/src/fx.rs) (`FxChain` + `FxParams`):
  chorus → phaser → delay → reverb → dynamics, between the bank sum and master
  volume. 26 params (per-effect on + wet + character), all default off/neutral →
  factory patch is FX-free. Off-path is a **true skip** via a per-slot 10 ms
  bypass fade that snaps to 0 (kernels held internally on; the fade owns
  click-free on/off — the split VXN1's `MasterFx` uses for reverb). vxn1b-engine
  95/95 (4 new fx tests + render parity intact), all vxn1b crates build, clippy
  clean. Commit `d3f7efe`.

**Acceptance met:** dynamics in `vxn-dsp` at the global OS rate with VXN1 green;
five effects serial with on/off + wet, each bypassing to identity when off; chain
allocation-free.

**Carried forward:**

- **Dynamics-at-1× aliasing** (epic risk) — the one item that can't close from
  code: needs an ear check in a DAW ([[verify-audio-in-reaper]]). OS plumbing is
  still 1×-hardwired, so verify at the eventual default 2× and note a 1× caveat
  if audible. Recorded in 0207's close-out.
- **UI** — the tab strip / per-effect panels are [[E038]]; params are
  host-automatable today without a faceplate.
- **Per-effect wet as a matrix destination** (ADR §2) stays a later candidate —
  this epic shipped host-automation params only.
</content>
