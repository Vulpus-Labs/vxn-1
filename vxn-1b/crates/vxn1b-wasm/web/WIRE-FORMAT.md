# VXN1b browser wire format

The main→worklet transport, defined once. Two implementations must agree
byte-for-byte:

| half | file |
|---|---|
| Rust (worklet, decodes + applies) | [`../src/codec.rs`](../src/codec.rs) |
| JS (main thread, encodes) | [`event-codec.mjs`](event-codec.mjs) |

Each has a hand-written golden byte table, and each asserts its own encoder
against it. They are transcriptions of one another — that duplication is
deliberate, because two independent tables that must match will catch a layout
change that one shared table would silently absorb.

## Slot layout (16 bytes, little-endian)

```text
off 0  u8   type      EV_* tag
off 1  u8   offset    sample offset within the upcoming quantum, 0..Q-1
off 2  u16  paramIdx  CLAP param id (params, gestures), or the packed matrix
                      address (matrix_edit)
off 4  f32  value     velocity / param value / bend / wheel / pressure / BPM
off 8  u8   note      MIDI note number (note + poly-pressure events)
off 9  u8   flag      MIDI channel, key mode, split note, scope tap, matrix
                      value byte, OR the param-norm bit
off 10 u16  seq       producer sequence — owned by the RING, not the codec.
                      `encode` writes 0; `decode` ignores it.
off 12 f32  reserved  zero
```

Fixed 16-byte slots rather than packed variable records: no record straddles the
ring's wrap boundary, the write index advances by exactly one slot, and the whole
lock-free protocol is a single atomic store of a monotonic counter. The cost is
internal fragmentation — a 6-byte note-on still burns 16 — which at a few hundred
events per quantum is free.

## Tags

| tag | event | fields |
|---|---|---|
| 1 | `note_on` | `value` = velocity, `note` = key, `flag` = MIDI channel |
| 2 | `note_off` | `note` = key, `flag` = MIDI channel |
| 3 | `param` | `paramIdx` = clap id, `value` = plain or norm, `flag` = norm bit |
| 4 | `pitch_bend` | `value` ∈ [-1, 1] |
| 5 | `mod_wheel` | `value` ∈ [0, 1] |
| 6 | *reserved* | vxn-1's `sustain`; VXN1b has no CC64 path — decodes to nothing |
| 7 | `key_mode` | `flag` = mode (0 Single, 1 Dual, 2 Split) |
| 8 | `split_point` | `flag` = note |
| 9 | `gesture_begin` | `paramIdx` = id — controller concern, no-ops on the engine |
| 10 | `gesture_end` | `paramIdx` = id — ditto |
| 11 | `lfo2_link` | `flag` 0/1 |
| 12 | `matrix_edit` | `paramIdx` = packed address, `flag` = value byte |
| 13 | `scope_tap` | `flag` = tap code (0 Off, 1 Layer 1, 2 Layer 2) |
| 14 | `tempo` | `value` = BPM |
| 15 | `poly_pressure` | `note` = key, `value` ∈ [0, 1], `flag` = channel |
| 16 | `channel_pressure` | `value` ∈ [0, 1], `flag` = channel |

Unknown **and reserved** tags decode to nothing, in both languages. That is
forward-compat: a producer from a newer build can put a tag on the wire that this
worklet skips, rather than having it misread as whatever kind shares its layout.

### Tags are shared across the synths only up to 10

**Tags 1–10 mean the same thing in vxn-1, vxn-2 and VXN1b. That is the whole of
the guarantee.** Tags 11 and up are synth-local and already conflict:

| tag | vxn-2 | VXN1b |
|---|---|---|
| 11 | `matrix_row` | `lfo2_link` |
| 12 | `patch_swap` | `matrix_edit` |

Each synth has its own ring, its own codec and its own worklet; nothing crosses
between them, so this is not a bug. But **never port a tag by number** — read
the target synth's codec.

## What VXN1b deliberately does not carry

Checked against the other ports rather than assumed:

- **No `sustain`.** `vxn1b-clap`'s bespoke `dispatch` has no CC64 path at all;
  the plugin ignores sustain. Adding one on this wire would make the browser
  build behave differently from the plugin.
- **No `patch_swap` pulse.** vxn-2 needs one because its native host bumps a
  `load_epoch` on preset load to silence the outgoing patch's ringing voices, and
  the worklet's separate `SharedParams` never sees it. VXN1b has no epoch
  mechanism — its native preset load swaps params and topology and lets voices
  ring — so a pulse would make the browser quieter than the plugin.
- **No bulk-state event.** `Engine::load_state` beyond params is the keyboard
  record (tags 7/8/11) plus `apply_envelopes()`, and `Synth::set_param` already
  re-cooks envelopes via `recooks_envelopes(id)`. A param stream is therefore
  equivalent to a state load, which is what makes the store-fold path faithful.
- **No layer-copy event.** `copy_layer` rewrites params and topology in the
  *model*, which on the web lives in the controller; a copy reaches the worklet
  as ordinary param writes plus `matrix_edit` records.

## What travels where

Two channels, and the split is load-bearing:

- **The param store** (`param-store.mjs`) carries everything with a CLAP id,
  block-granular, latest-value-wins. That includes each matrix slot's **depth**.
- **The ring** carries notes, sample-accurate param automation, and every piece
  of non-automatable domain state: key mode, split point, LFO 2 link, matrix
  **topology**, scope tap, tempo.

Slot depth on the store and slot topology on the ring is the 0219 split, and it
is why a slot can be automated without its routing changing underneath the
automation. A `matrix_edit` must never touch depth.

Topology is applied **at its sample offset**, in the same slice loop as params —
never hoisted to block start. A preset load moves params and topology together;
hoisting would land the new routing ahead of the depths travelling with it, and a
slot would briefly route its new source at its old depth. On a matrix-heavy patch
that is audible.

## Matrix address packing

`paramIdx` on tag 12 packs the target:

```text
layer << 12 | slot << 8 | field
```

- `layer` — 0 = Layer 1, 1 = Layer 2
- `slot` — 0..15
- `field` — 0 Source, 1 Dest, 2 Curve, 3 ScaleSrc

An out-of-range layer, slot or field **unpacks to nothing and the record is
dropped**, never clamped onto a valid slot. Clamping would land the edit on a
real slot the sender never aimed at, silently rewiring a patch.

## Param id layout

VXN1b's space is two-layer, like vxn-1's (vxn-2 flattened its own and has no
equivalent):

```text
counts:  PATCH_COUNT  = 75   per-layer patch params
         GLOBAL_COUNT = 35   globals, shared by both layers
         LAYER_COUNT  = 2
         TOTAL_PARAMS = 2*75 + 35 = 185

[  0 ..  75 )   Layer 1 patch params   clap_id = patch_index
[ 75 .. 150 )   Layer 2 patch params   clap_id = 75 + patch_index
[150 .. 185 )   global params          clap_id = 150 + global_index
```

A Layer 1 control and its Layer 2 twin are separate automation targets.

### Changing the param count

The JS counts are a **hand-declared mirror** — the browser has no build step that
could read the Rust table. [Ticket 0285](../../../../tickets/open/0285-web-param-mirror-drift.md)
is what that costs when it rots: vxn-1 and vxn-2 both shipped browser builds that
could not boot, because their declared counts had drifted behind engines that had
grown params. The runtime handshake caught it immediately; nobody ran it.

When a param is added or removed:

1. update `PATCH_COUNT` / `GLOBAL_COUNT` in [`event-codec.mjs`](event-codec.mjs);
2. update the counts in this file;
3. run `node --test vxn-1b/crates/vxn1b-wasm/web/*.test.mjs`.

Nothing else in JS hard-codes them — `param-store.mjs` imports from the codec,
and Rust re-exports from `vxn1b-engine` rather than declaring anything.

`wasm-agreement.test.mjs` reads `vxn1b_total_params()` out of the built artifact
and fails if the mirror disagrees. It **fails rather than skips** when the wasm
is missing, by design: vxn-2's equivalent tests skip on a missing artifact, and
since both ports' `xtask web` write and wipe the same `target/web-dist`, a normal
run reported "89 pass" with 13 tests silently skipped — including every one that
would have caught 0285.

## The return channel (audio -> view)

Everything above travels main -> worklet. Meter and scope frames travel back, over
a **second SAB** with its own layout ([`telemetry.mjs`](telemetry.mjs), ticket
0288):

```text
i32[0] meterSeq        seqlock counter: even = stable, odd = mid-write
i32[1] scopeSeq        ditto
i32[2] scopeLen        samples valid in the scope region (0 = nothing yet)
i32[3] reserved
f32[4 ..)              meter frame, MeterTap order, linear peak magnitudes
f32[..)                scope window, oldest -> newest
```

Region sizes come from `vxn1b_meter_len()` / `vxn1b_scope_window()`, never from
literals — adding a meter tap must not silently truncate the frame.

A **seqlock**, not the param store's plain per-slot atomics: a frame is not a set
of independently meaningful slots, and a scope window stitched from two captures
shows a discontinuity that reads as a glitch rather than as stale data. The
writer never blocks (two atomic stores, no CAS); the reader retries a bounded
number of times and otherwise keeps the frame it had.

The writer publishes on a **rate division**, not every quantum. `MeterFrame::drain`
is read-and-clear, so each frame reports the extreme since the last drain;
draining every quantum at 48 kHz would be ~375 drains against a 60 Hz reader and
would discard five quanta of peaks unseen. It divides to ~60 Hz, and publishes
the scope every second time (~30 Hz), matching the native `SCOPE_TICK_DIVISOR`.

Silence suppression — deliver one silent frame, then stop until audio returns —
lives on the **main** thread. The write is nearly free and the reader polls
anyway, so the render thread stays unconditional and the policy sits where it
costs nothing.

## Running the tests

```sh
RUSTFLAGS="-C target-feature=+simd128" cargo build -p vxn1b-wasm \
    --target wasm32-unknown-unknown --release
node --test vxn-1b/crates/vxn1b-wasm/web/*.test.mjs   # expect: 0 skipped
cargo test -p vxn1b-wasm                              # the Rust half
```
