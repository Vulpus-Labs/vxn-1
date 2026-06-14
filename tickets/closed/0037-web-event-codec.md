---
id: "0037"
product: vxn-2
title: "Scaffold: binary event codec (Rust + JS, round-trip tested)"
priority: high
created: 2026-06-14
epic: E015
depends: ["0035", "0036"]
---

## Summary

Turn the 0035 spike's ad-hoc record framing into a real, shared binary
event codec — the wire format for the
[E015](../../epics/open/E015-web-event-driven-core.md) event ring. One
definition, two implementations (Rust for the worklet decoder + any
Rust writer, JS for the main-thread encoder), kept honest by round-trip
tests.

Mirrors the semantics of `vxn-core-clap`'s `dispatch_event`
([events.rs:43-89](../../crates/vxn-core-clap/src/events.rs#L43-L89)) so
the web path applies events to `Synth` identically to the plugin.

## Design

- **Event set** (the union the ring must carry):
  - `set_param { id: u16, plain: f32 }`
  - `set_param_norm { id: u16, norm: f32 }`
  - `gesture_begin { id: u16 }` / `gesture_end { id: u16 }`
  - `note_on { note: u8, velocity: f32 }` / `note_off { note: u8 }`
  - `pitch_bend { norm: f32 }`, `mod_wheel { norm: f32 }`,
    `sustain { on: bool }`
  - `key_mode { mode: u8 }`, `split_point { note: u8 }`
    (non-automatable shared state — set once per block, per the CLAP loop)
- **Framing**: fixed header (sample-offset timestamp + tag + length),
  little-endian, alloc-free, fixed max record size. Use the param-id
  layout fixed by 0036 (69×2 patch + 27 global = 165).
- **Rust decoder**: zero-copy read from a byte slice (the ring view); maps
  each record to the matching `Synth` call, reusing the dispatch semantics.
- **JS encoder**: writes records into the ring's `SharedArrayBuffer` (or
  the spike's buffer) with the same layout.
- **Round-trip tests**: encode in JS → decode in Rust (and vice versa
  where applicable) for every event kind, including boundary values
  (note 0/127, bend extremes, all key modes). A golden-bytes table guards
  the layout against drift.

## Acceptance criteria

- [ ] A single documented binary layout, implemented in both Rust and JS.
- [ ] Every event kind round-trips JS↔Rust with identical bytes
      (golden-table test) — including boundary values.
- [ ] The Rust decoder applies each record to `Synth` with semantics
      matching `vxn-core-clap::dispatch_event` (verified against the same
      inputs).
- [ ] Encoding/decoding is alloc-free on the hot path.
- [ ] Layout uses the 0036 param-id addressing; key-mode/split-point
      records carried as non-automatable state.

## Notes

- Depends on [0035](0035-web-sab-event-ring-spike.md) (framing shape) and
  [0036](0036-web-controller-placement-adr.md) (param-id addressing,
  controller placement — which determines whether the writer is JS, Rust,
  or both).
- Consumed by [0038](0038-web-worklet-audio-host.md) (decoder in the
  render loop) and the input adapters in E017 (encoders).
- Out of scope: the ring buffer itself (0035→0038), param-store atomics
  (0039).

## Close-out (2026-06-14)

- **One layout, two implementations** over the frozen 0035 16-byte LE
  fixed-slot framing (byte-for-byte, no new layout invented).
  - Rust: [codec.rs](../../vxn-1/crates/vxn-wasm/src/codec.rs) — typed
    `Event` enum, zero-copy `decode(&[u8])`, alloc-free
    `encode`/`encode_into([u8;16])`, `apply(event,&mut Synth)`,
    `decode_and_apply`; wired into [lib.rs](../../vxn-1/crates/vxn-wasm/src/lib.rs).
  - JS: [event-codec.mjs](../../vxn-1/crates/vxn-wasm/web/event-codec.mjs) —
    `encodeInto`/`encode`/`decode`, typed `ev.*` constructors, and the
    exported id-layout constants (now the single source of truth that
    [param-store.mjs](../../vxn-1/crates/vxn-wasm/web/param-store.mjs)
    imports).
- **Event set + tags:** note_on(1)/note_off(2)/param(3)/pitch_bend(4)/
  mod_wheel(5)/sustain(6)/key_mode(7)/split_point(8) (the 0035 reserved
  set) + gesture_begin(9)/gesture_end(10). **norm vs plain:** one EV_PARAM
  tag, `flag = PARAM_FLAG_NORM(1)` selects norm; decoder converts norm→plain
  via `ParamDesc::from_normalized` (engine + SAB carry plain, ADR 0009 §2).
  **gestures** round-trip but `apply()` no-ops them on the Synth (a
  controller/host-echo concern, not rendering). key_mode/split_point ride in
  `flag` as non-automatable shared state.
- **Dispatch parity:** `apply()` replicates `vxn_core_clap::dispatch_event`
  and the `SynthNotes` adapter
  ([events.rs](../../crates/vxn-core-clap/src/events.rs)). 14-bit-bend /
  CC1 / CC64 MIDI decoding stays encoder-side (Web MIDI adapter, E017); the
  codec carries already-normalized values. Test
  `codec::tests::dispatch_parity_with_clap_reference` decodes a 9-event
  stream, applies via codec, and asserts identical output across 5 quanta vs
  the hand-written CLAP-reference calls.
- **Addressing pulled from vxn-app** (not hardcoded): added `vxn-app` dep to
  the `vxn-wasm` crate; `id_layout_matches_vxn_app` asserts 165 = 69×2 + 27.
  Adding the dep kept the wasm build clean (confirms 0036's wasm-clean
  finding).
- **Tests:** `cargo test -p vxn-wasm` 9/9 (golden encode + decode,
  round-trip, unknown-tag/short-buffer→None, id-layout, gesture no-op,
  norm→plain, dispatch parity); `node web/event-codec.test.mjs` 6/6 (same
  golden byte table as the Rust side, non-zero slot base, forward-compat);
  `cargo build … --target wasm32-unknown-unknown --release` clean.
