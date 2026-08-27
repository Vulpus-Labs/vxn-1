---
id: E044
product: monorepo
title: "Envelope lifecycle trait + runtime-swap readiness pattern (additive, no numerics move)"
status: open
created: 2026-08-02
---

> **vxn-1 retired, 2026-08-27.** The original vxn-1 is archived under
> `archive/vxn-1/`, out of the workspace and not expected to compile.
> **vxn-1b is now the canonical virtual-analogue synth**, and it carries what
> was vxn-1's DSP: `vxn-dsp` moved to `vxn-1b/crates/vxn-dsp` with its name
> intact. Where this epic says "vxn-1" as an *adopter* of shared code, read
> **vxn-1b** — the kernels are the same ones. Where it names vxn-1's shells,
> engine or web port, that work is gone.

> Four envelope families share one lifecycle — `cook / note_on / note_off /
> scale_rates / tick(dt) -> level` — with different param shapes: vxn-2
> `EgState` (4R/4L unsigned, `EgCurve`), `PitchEgState` (4R/4L signed +
> depth), `ModEnvState` (ADSR + shape), vxn-1 `AdsrCore` (ADSR,
> `tick(triggered, gate_high)`). This epic names the lifecycle as a trait
> (implemented **in place** — no numerics move, the marchers stay per-synth)
> and documents the block-edge kernel-selection pattern that makes runtime
> swapping of oscillators/envelope curves/filters wireable later without
> re-architecture (locked decision: design-for, wire later).

## Goal

- `vxn-core-dsp::env::EnvLifecycle` trait; impls for `EgState`,
  `PitchEgState`, `ModEnvState`; adapter impl for `AdsrCore` (note_on latches
  `triggered`, note_off drops `gate_high`, `tick(dt)` ignores dt —
  semantic-preserving, unit-tested bit-exact against direct calls).
- vxn-core-dsp docs describe the swap dispatch pattern: runtime enum resolved
  once per block edge to a marker type (`with_wave!`/`with_mix!` template) or
  fn-ptr table (`LANE_ROUTE_FNS` style), state structs worst-case-sized, no
  boxing, no per-sample dispatch. `KernelSelectFn` alias + one compile-tested
  example. No user-facing swap params.

## Planned tickets

Chain: **0238 → 0239**.

- [ ] **0238** — `EnvLifecycle` trait + in-place impls + `AdsrCore` adapter.
- [ ] **0239** — Swap-readiness pattern docs + `KernelSelectFn` + example.

## Acceptance

- All goldens untouched; adapter-equivalence unit tests green.
- A future "filter model" or "env curve" param has a documented, compile-
  tested dispatch recipe that provably keeps enum matches out of lane loops.
