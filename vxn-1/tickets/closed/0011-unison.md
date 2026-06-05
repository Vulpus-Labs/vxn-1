---
id: "0011"
title: Unison assign mode
priority: medium
created: 2026-05-25
epic: E003
---

## Summary

Add **Unison** to the per-layer assign mode (0010): one logical note broadcast
across all 8 of a layer's channels with per-channel detune drift for thickness
(the JP-8 Unison idea, ADR 0003 §4). Builds entirely on the MIDI-processor seam;
no router or render changes.

## Acceptance criteria

- [x] `AssignMode::Unison` (the value reserved in 0010): a single held note
      drives all 8 channels of the layer; subsequent notes follow a defined
      priority (last-note, matching the existing note logic) — document the
      choice.
- [x] Per-channel **detune drift**: small, fixed per-channel pitch offsets
      (cents) spread the 8 channels for chorusing thickness; a per-patch
      `UnisonDetune` param (cents, 0 = all in tune) scales the spread.
- [x] Output level stays sensible as channel count engaged changes (avoid an
      8× level jump vs poly) — normalise the unison sum.
- [x] Tests: in unison, one note-on engages all 8 channels of the layer;
      detune > 0 produces beating/spread (adjacent-channel pitch differs); detune
      0 collapses to unison pitch; switching Poly↔Unison is clean (no stuck
      channels).

## Notes

- Detune spread can be a fixed symmetric pattern (e.g. ±n cents across the 8)
  scaled by `UnisonDetune`; per-channel constant, not random per note, so it is
  deterministic and testable.
- Interacts well with the global chorus but should stand on its own.
- Depends on 0010. Validation: `cargo test -p vxn-engine`.
