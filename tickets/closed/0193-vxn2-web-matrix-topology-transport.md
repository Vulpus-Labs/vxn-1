---
id: "0193"
product: vxn-2
title: "Web: matrix topology never reaches the audio worklet (source/dest/curve/active frozen at default)"
priority: high
created: 2026-07-13
epic: null
depends: []
---

## Summary

In the browser build the mod matrix is non-functional for **routing**: editing a
slot's source / dest / curve / active in the web faceplate has no audible effect.
Only slot **depth** responds. The engine keeps whatever topology the compiled
default patch (`default_patch::default_matrix`) baked in — slot 0 =
`Lfo2 → GlobalPitch` — regardless of what the user picks, so every route sounds
like LFO2 into pitch.

Root cause: the web build runs two wasm modules — the main-thread **controller**
(`vxn2-web-controller`, authoritative param state + UI) and the audio-worklet
**engine** (`vxn2-wasm`). They sync through the flat 209-slot CLAP-param store
SAB. `controller.mjs::_mirrorToStore` copies only `vxnc_values_ptr` (the
`TOTAL_PARAMS` float values). Matrix **depth** (slots 1-8) is a CLAP param so it
crosses; matrix **topology** lives in a separate `SharedParams::matrix_meta`
`[AtomicU32]` array with no CLAP id, so it is never mirrored and never reaches
the worklet. The input-event ring codec (`event-codec.mjs` / `codec.rs`) carries
note / param / gesture / bend / wheel / sustain only — no matrix event. So there
is simply no transport for topology in the web build.

Native CLAP is unaffected: host + engine share one `SharedParams`, so
`set_matrix_row_raw` is immediately visible to the audio thread.

Symptoms that trace to this one gap: source edits inaudible; reopening the
overlay shows the correct (controller-side) source while the wrong source sounds;
depth is the only field that bites; "mod matrix very broken in web".

## Acceptance criteria

- [ ] A matrix topology edit in the web faceplate (source / dest / curve /
      active) changes the audible route within one control tick.
- [ ] Depth edits keep working (no regression on the existing CLAP-param path).
- [ ] `EV_MATRIX_ROW` round-trips: JS `encodeInto` → Rust `decode_and_apply`
      applies `set_matrix_row_raw(slot, row)` with all five fields intact.
- [ ] Reopening the overlay after an edit shows a source that matches what is
      audible (controller and worklet agree).
- [ ] Slots 9-16 (non-CLAP depth) route correctly too — depth rides the row.
- [ ] Codec drift test (JS encoder vs Rust decoder) covers the new event, matching
      the existing param/gesture codec test.
- [ ] Switching presets updates the audible routing, not just the UI + depth
      (preset restore surfaces only a `matrix_snapshot`, never `setMatrixRow`).
- [ ] Switching presets silences the outgoing patch's still-ringing voices
      (`load_epoch` is not a value param, so the patch-swap silence signal must
      cross via an `EV_PATCH_SWAP` ring pulse, not the store).

## Notes

Chosen fix: add a fixed-slot `EV_MATRIX_ROW` to the 16-byte ring codec — it fits
(slot u8 + source u8 + dest u8 + curve u8 + active u8 + depth f32 = 9 bytes). The
ring already crosses main→worklet, so topology travels the same proven path as
notes/params instead of inventing a second SAB region.

Wiring: the faceplate bridge's `set_matrix_row` opcode must reach the worklet in
addition to the controller (which stays authoritative for UI snapshots). Push an
`EV_MATRIX_ROW` onto the producer ring from the bridge / coordinator producer
surface, and keep the existing `ctrl.setMatrixRow` call so the overlay's snapshot
echo is unchanged. Depth for CLAP slots 1-8 still also rides `set_param`; the row
event carries depth too so slots 9-16 (no CLAP id) get theirs.

Touch points:

- `crates/vxn2-wasm/src/codec.rs` — `EV_MATRIX_ROW` const, `Event::SetMatrixRow`,
  encode `row(...)`, `decode_and_apply` → `shared.set_matrix_row_raw`.
- `crates/vxn2-wasm/web/event-codec.mjs` — `EV_MATRIX_ROW`, `encodeInto` / `decode`
  cases, typed constructor.
- `crates/vxn2-wasm/web/coordinator.mjs` — producer-surface `setMatrixRow` that
  pushes the ring event.
- `crates/vxn2-wasm/web/faceplate-bridge.mjs` — route `set_matrix_row` to the
  producer ring as well as the controller.
- `crates/vxn2-wasm/web/controller.mjs` — `setMatrixRow` dual-writes (controller
  wasm + ring); `_mirrorMatrixToRing` fans `matrix_snapshot` ViewEvents to the
  ring each tick so preset loads / reset (which never call `setMatrixRow`) reach
  the worklet.
- `crates/vxn2-wasm/web/event-ring.mjs` — `pushMatrixRow` / `pushPatchSwap`.
- `EV_PATCH_SWAP` (tag 12): same root cause for `load_epoch`. Preset load bumps
  the epoch on native (shared `SharedParams`) so `snapshot_params` silences the
  outgoing voices; the web worklet has its own `SharedParams` and the epoch isn't
  a value param, so a preset switch left the previous patch ringing. The
  controller pushes `EV_PATCH_SWAP` on the `preset_loaded` ViewEvent →
  `SharedParams::bump_load_epoch` on the worklet → existing silence path fires.
- Tests: extend the codec round-trip test (`event-codec.test.mjs` +
  `codec.rs` unit test) with `EV_MATRIX_ROW`; ring `pushMatrixRow` decode test;
  controller `_mirrorMatrixToRing` snapshot-fan test.
