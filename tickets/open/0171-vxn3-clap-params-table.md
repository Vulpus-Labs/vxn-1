---
id: "0171"
product: vxn-3
title: "vxn-3 clap.params fixed table — declaration + host→engine automation writes"
priority: medium
created: 2026-07-02
epic: E032
---

## Summary

Add the fixed, engine-independent `clap.params` table to the VXN3 clap shell and
route incoming host param automation into the engine. This is the host-facing
surface itself — mix + master + 3 macro slots per track — with a deterministic
positional id scheme, **never rescanned**. Engine-aware value-text is 0172; host
echo is 0173.

Design: ADR 0003 §1 (fixed host-param table) + §4 (no rescan-on-swap). Depends on
0170 (`set_macro`).

## Design

- **Extension.** Register `PluginParams` (clack `PluginAudioProcessorParams` +
  `PluginMainThreadParams`) alongside the existing extensions in
  [vxn3-clap/src/lib.rs:48](vxn-3/crates/vxn3-clap/src/lib.rs#L48). This is vxn3's
  first flat-param extension — the shell doc comment (there is deliberately "no
  params/state extension") gets updated.
- **Fixed layout / stable ids.** A single `const` positional scheme maps
  `(track, slot)` → `clap_id`, computed not accreted, stable across sessions:
  - Per track (× `N_TRACKS`): `level`, `pan`, `mute`, `send`, `macro1..3` — a fixed
    stride so track `t` slot `s` = `BASE + t * STRIDE + s`.
  - Master block above the per-track range: `volume`, `limiter`, delay `feedback`,
    delay `time`, delay `return`.
  Centralize the id↔(track,slot) mapping + ranges/flags in one module so 0172–0174
  reuse it.
- **`count` / `get_info` / `get_value`.** `get_info` reports name (generic
  `T{t} · {slot}` for macros; real names for mix/master), range, default, flags
  (`AUTOMATABLE`; `mute` stepped/boolean). `get_value` reads the current value
  cache (see below).
- **Host→engine.** In `process()`, walk the input event list for `ParamValue`
  events and translate each `clap_id` → the matching `EngineCommand`
  (`SetGain/SetPan/SetSend/SetMacro/...` from
  [io.rs:23](vxn-3/crates/vxn3-engine/src/io.rs#L23) — already the engine's command
  surface). Mute maps to gain-gate or a dedicated command. Keep a plain value cache
  (array) updated on write so `get_value` + 0174 state can read it without touching
  the audio-thread engine.
- **`value_to_text` stub.** Generic formatting only (raw normalized / dB / pan);
  engine-aware macro text lands in 0172.

## Acceptance criteria

- [ ] VXN3 declares a fixed `clap.params` table; `clap_id`s are computed from the
      positional scheme and identical across re-instantiation (unit-tested: the
      id↔(track,slot) map round-trips for all tracks/slots).
- [ ] Host automation of a mix param (level/pan/send/mute) and a macro slot reaches
      the engine via the existing `EngineCommand` queue and is audible.
- [ ] `count` matches the declared layout; `get_info` returns stable names/ranges;
      no `params-rescan` is ever requested.
- [ ] `get_value` reflects the last host write (value cache); param routing in
      `process()` is allocation-free.
- [ ] `cargo test -p vxn3-clap` green; `clap-validator` params checks pass (full
      validator sweep is 0174).

## Notes

- Do **not** expose the variable per-engine set — only mix + master + the 3 generic
  macros (ADR 0003 rejects the union table).
- The value cache is the seed of 0173's echo (dirty bitset) and 0174's state blob —
  design it as the single owner of host-facing param values.
- `mute` semantics: pick gain-gate vs explicit `SetMute` command and note it for
  0174 (state must restore it).
