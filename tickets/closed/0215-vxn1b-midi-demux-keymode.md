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

## Close-out (2026-07-31)

- **Demux + derived KeyMode.** `KeyMode {Single,Dual,Split}` + `KeyState
  {layer2_on, split_enabled, split_point}` (non-automatable domain state) added to
  [engine.rs](../../vxn-1b/crates/vxn1b-engine/src/engine.rs). `Engine::note_on`
  routes by `KeyState::key_mode()`: Single→synth 1; Dual→both; Split→below point =
  Lower (synth 2), at/above = Upper (synth 1). Setters `set_layer2_on` /
  `set_split_enabled` / `set_split_point` / `set_key_state`; getters `key_mode` /
  `key_state`. Exported from [lib.rs](../../vxn-1b/crates/vxn1b-engine/src/lib.rs).
  Verified `engine::tests::key_mode_is_derived_from_toggles`,
  `dual_fans_note_on_to_both_synths`, `split_routes_note_on_by_pitch` (incl.
  at-split boundary = Upper), `single_mode_leaves_synth2_silent`.
- **Note-offs broadcast in all modes.** `Engine::note_off` unconditionally calls
  both synths — the split-move stuck-note fix, no owner map. Verified
  `engine::tests::split_move_does_not_strand_a_held_note` (release after moving
  split above a held note releases it *and* leaves the other held note ringing —
  no stuck voice, no range-kill) and `note_off_broadcasts_in_single_mode`. Test
  helpers `Voices::is_holding` / `Synth::voices_holding` (`#[cfg(test)]`).
- **KeyMode + split point as blob state.** `KeyState` is a fixed 3-byte record
  with `write`/`read`; `key_mode` derived, never stored. Verified
  `engine::tests::key_state_round_trips_through_blob` (3-byte, short read = error)
  and `default_key_state_is_single_middle_c` (split point 60). Serialisation into
  the two-layer `clap.state` blob is 0221; this crate owns the record shape.
- **Green + alloc-free.** `cargo test -p vxn1b-engine` → 113 + 1 + 2 + 4 pass,
  incl. `tests/alloc_free.rs::hot_path_is_allocation_free` and
  `tests/parity.rs::default_patch_render_matches_vxn1` (single mode still
  byte-exact — demux changes are additive, existing signatures unchanged).
