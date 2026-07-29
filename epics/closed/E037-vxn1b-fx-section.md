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
</content>
