---
id: "0053"
product: vxn-3
title: "vxn-3 host param table — fixed mix + per-track macro slots (engine reinterpretation)"
priority: medium
created: 2026-06-15
---

## Summary

Give VXN3 a `clap.params` table so the host can automate / modulate / save
parameters, **without** trying to expose the variable per-engine param set
(whose cardinality and semantics change on engine swap). Implements the model in
ADR 0003: a fixed engine-independent mix + master table plus a small fixed budget
of generic per-track **macro slots** the active engine reinterprets.

Post-MVP breadth — not part of E021. Groundwork for the param/preset epic; pairs
with presets (which persist engine + patch + macro values) and with 0051's send
amount (a mix param).

## Design

See ADR 0003 (`vxn-3/adrs/0003-vxn3-host-param-model.md`) for the rationale and
the rejected alternatives (union table, rescan-on-swap, no-params).

- **Fixed table, deterministic layout.** Stable `clap_id`s by positional scheme
  (`track t · slot s`), same every session, never rescanned. Contents:
  - Per track (× `N_TRACKS`): `level`, `pan`, `mute`, send amount(s), and
    `K` macro slots (`macro 1..K`, `K ≈ 3–4` — pin the number here).
  - Master: master volume + limiter (with 0051) + global send-FX params (ADR
    0002, as they land).
- **Macro slots.** Generic fixed id/name (`T3 · M1`); the active engine maps the
  normalized slot value onto its patch. Extend `TrackEngine` with a declared
  macro mapping — `macro_count()`, `set_macro(i, v)`, and `macro_display(i, v)`
  for engine-aware `value_to_text` — superseding the MVP `set_knob`.
- **Wire-up.** Bridge the existing `EngineIo` command path + p-lock resolver to
  the new host params: host param writes → engine; UI/p-lock writes → host
  echo (the dirty-bitset pump pattern from vxn-2, adapted). p-locks may target
  macro + mix params (already continuous-param p-lockable via 0050).
- **No rescan on swap.** Names/info fixed; only macro values + value-text change.
- **State.** `clap.state` save/restore of the fixed table; per-track engine kind
  + patch saved alongside (preset epic territory, but the blob format is decided
  here so state round-trips through an engine swap).

## Acceptance criteria

- [ ] VXN3 declares a fixed `clap.params` table; ids are stable across re-instantiation
      and a project reload (no rescan on engine swap).
- [ ] Host automation of a per-track macro slot drives the active engine's mapped
      param; swapping the engine changes what the slot does **without** changing
      its id (existing automation keeps targeting the same slot).
- [ ] `value_to_text` renders the macro engine-aware (e.g. "Decay 0.42 s" vs
      "Ring 1.8 s"); mix params render normally.
- [ ] Mix params (level/pan/mute/send) + master are automatable and engine-independent.
- [ ] A p-lock on a macro / mix param round-trips and is reflected to the host.
- [ ] Param flush + value→engine routing is allocation-free on the audio thread.

## Notes

- The faceplate keeps the **full** per-engine control set on the custom-event
  channel (0052); only the mix + macro layer is host-facing. Deep per-engine
  automation stays internal via p-locks (ADR 0001 §3a, 0050).
- The MVP knob surface (`Knob { Decay, Tone, Pitch }`, 0052) is the seed of the
  macro slots — generalise it here; also fix its current dead mappings (`Tone`
  no-op on Kick/Metal, `Pitch` absent from the faceplate) as part of the macro
  mapping work.
- Should anchor a future "vxn-3 host param + preset" epic; open that epic when
  this is scheduled.
- Design: ADR 0003; ADR 0001 §3a/§4/§5; ADR 0002 (master/send params).
