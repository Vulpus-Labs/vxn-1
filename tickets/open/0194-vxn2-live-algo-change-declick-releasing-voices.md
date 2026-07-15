---
id: "0194"
product: vxn-2
title: "Live algo change: declick releasing voices so a promoted modulator can't ring out"
priority: medium
created: 2026-07-15
epic: null
depends: []
---

## Summary

Switching the FM algorithm live (picker move, not a preset load) re-routes voices
in place via `Stack::set_algo_live`. For a voice that is still *releasing* after
note-off, an op that was a **modulator** under the old algo can become a
**carrier** under the new one — dumping its (potentially very long) release tail
straight onto the audio bus as a surprise sustained tone that rings until the
release finishes.

Fix (design choice: *cut the whole releasing voice*): in
`Engine::apply_block_params`, detect a change in the patch algorithm against a new
`last_algo` field and, on the change, `start_declick()` every voice that is
releasing (`!meta.gate && !is_idle()`). Held (gated) voices are left to re-route
and morph as intended — live timbre morphing on held notes is desirable. Preset
loads take the separate `load_epoch → silence_all` path, so those voices are
already idle by this point and skipped. `start_declick` fast-releases all op EGs
proportionally over `DECLICK_SECS`, so the cut is click-free.

This is engine behaviour (applies to native CLAP and the web/wasm build alike),
not a web-transport concern — distinct from ticket 0193.

## Acceptance criteria

- [x] A live algo change declick-kills voices that are mid-release.
- [x] Held (gated) voices are NOT cut — they re-route and morph.
- [x] Preset loads still silence via `load_epoch`, not this path (no double-cut).
- [x] Test: releasing voice → `VoicePhase::Declick` after the change; held voice
      stays gated and non-declick.

## Notes

- `AlgoSpec.carriers` (6-bit mask) exposes which ops are on the bus, so a more
  surgical variant (silence only ops newly promoted to carrier, keep the rest of
  the release) is possible — deferred; the whole-voice cut was the chosen
  behaviour.
- Touch points: `engine.rs` (`last_algo` field + declick loop in
  `apply_block_params`); test `live_algo_change_declicks_releasing_voice_not_held`.
