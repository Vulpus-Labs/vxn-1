---
id: "0215"
product: vxn-1b
title: "MIDI demux + KeyMode — single/dual/split, route-on / broadcast-off"
priority: high
created: 2026-07-31
epic: E039
depends: ["0214"]
---

## Summary

Add the MIDI demux in front of the two `Synth`s ([[0214]]) and derive `KeyMode`.
Per [ADR 0002](../../vxn-1b/adrs/0002-vxn1b-dual-layer.md) §2–§3.

## Design

- **KeyMode** ∈ {Single, Dual, Split}, **derived** from UI state:
  - Layer 2 **off** → `Single`.
  - Layer 2 **on**, split disabled → `Dual`.
  - Layer 2 **on**, split enabled → `Split` at the split point.
  - `KeyMode` + split point (MIDI note, default 60) are **non-automatable blob
    state** (as VXN1
    [domain.rs:26-61](../../vxn-1/crates/vxn-app/src/domain.rs#L26-L61)).
- **Routing:**
  - **Single**: all events → synth 1; synth 2 bypassed.
  - **Dual**: every event fanned to **both** synths.
  - **Split**: note-**on** routed by pitch vs split point (below → Lower / synth
    2, at/above → Upper / synth 1 — match VXN1's convention); CC / wheels /
    pressure fanned to both.
- **Note-offs are ALWAYS broadcast to both synths**, in every mode. Owning synth
  releases; other synth no-ops on unmatched pitch. This is the fix for the
  split-move stuck-note bug — note-on routed at press time, split point moves,
  note-off would else route to the wrong synth. No per-note owner map, no cut
  held notes.

## Acceptance

- Demux routes single/dual/split per the rules above; KeyMode derived from
  L2-on + split-enable; KeyMode + split point persist as blob state.
- **Note-offs broadcast in all modes.**
- Test — split-move stuck-note: hold a note above the split, move split above it,
  send note-off → note releases (no stuck voice), and other held notes ring out
  (no range-kill).
- Test: dual fans note-ons to both; split routes by pitch; single leaves synth 2
  silent.
- `cargo test -p vxn1b-engine` green; allocation-free callback.
