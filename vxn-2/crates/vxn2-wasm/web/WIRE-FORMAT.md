# vxn-2 web event wire format (epic E030)

One page, two implementations. The Rust half is [`../src/codec.rs`](../src/codec.rs);
the JS half is [`event-codec.mjs`](event-codec.mjs). Both encode/decode the same
fixed 16-byte slot, and both ship an identical **golden byte table**
(`codec.rs tests::golden` ⇄ `event-codec.test.mjs GOLDEN`) that is the drift
tripwire — change one, the other's test fails.

The framing is carried verbatim from vxn-1's spike-0035 layout so vxn-1 and
vxn-2 share a wire format; the JS ring/store port across with only the param-count
and the dropped key-mode/split events changed.

## Slot (16 bytes, little-endian)

| off | type | field    | meaning                                             |
|----:|------|----------|-----------------------------------------------------|
| 0   | u8   | type     | `EV_*` tag (below)                                  |
| 1   | u8   | offset   | sample offset within the upcoming quantum, `0..128` |
| 2   | u16  | paramIdx | CLAP param id — `EV_PARAM` / gestures; else 0       |
| 4   | f32  | value    | velocity / param value / bend / wheel               |
| 8   | u8   | note     | MIDI note — `EV_NOTE_ON` / `EV_NOTE_OFF`            |
| 9   | u8   | flag     | sustain 0/1, **or** the param-norm bit on `EV_PARAM`|
| 10  | u16  | seq      | producer sequence (low 16 bits) — **owned by the ring**, not the codec; encode writes 0 |
| 12  | f32  | reserved | zero                                                |

## Event tags

| tag | name             | payload                                  |
|----:|------------------|------------------------------------------|
| 1   | NOTE_ON          | `value`=velocity `[0,1]`, `note`         |
| 2   | NOTE_OFF         | `note`                                   |
| 3   | PARAM            | `paramIdx`=id, `value`=plain or norm, `flag`=`PARAM_FLAG_NORM`(1) selects norm |
| 4   | PITCH_BEND       | `value` in `[-1,1]`                      |
| 5   | MOD_WHEEL        | `value` in `[0,1]`                       |
| 6   | SUSTAIN          | `flag` 0/1                               |
| 7   | *(reserved)*     | was vxn-1 `KEY_MODE` — unused in vxn-2   |
| 8   | *(reserved)*     | was vxn-1 `SPLIT_POINT` — unused in vxn-2|
| 9   | GESTURE_BEGIN    | `paramIdx`=id; no-op on the engine       |
| 10  | GESTURE_END      | `paramIdx`=id; no-op on the engine       |

An unknown tag decodes to `null` / `None` (forward-compat).

## vxn-2 specifics

- **Params are flat.** CLAP id == param index, `0 .. TOTAL_PARAMS`
  (`vxn2_engine::TOTAL_PARAMS` = 209 today). No Upper/Lower layer split, so no
  `patchClapId` / `globalClapId` (vxn-1 had those). `u16 paramIdx` fits with
  vast headroom.
- **No key-mode / split-point.** The FM engine is a single voice pool. Tags 7/8
  stay reserved so notes/params/gestures keep vxn-1's byte numbering, but no
  producer emits them and the decoder treats them as unknown.
- **Param apply is block-granular.** `PARAM` writes the atomic store
  (`SharedParams` analogue); the wasm host folds it into the engine once per
  quantum via `Engine::snapshot_params`. Notes / bend / wheel / sustain act on
  the engine immediately (sample-accurate within the quantum).
