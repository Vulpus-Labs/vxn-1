---
id: "0286"
product: vxn-1b
title: "vxn1b-wasm: engine cdylib — C-ABI worklet render loop + binary event codec"
priority: medium
created: 2026-08-25
epic: E045
depends: ["0284"]
---

## Summary

Second ticket of [E045](../../epics/open/E045-vxn1b-web-wasm-browser-port.md):
the half of VXN1b that runs **in the AudioWorklet**. A raw C-ABI `cdylib` — no
wasm-bindgen — so the module instantiates cleanly inside a worklet scope. Ports
[vxn-wasm/src/host.rs](../../vxn-1/crates/vxn-wasm/src/host.rs) and
[codec.rs](../../vxn-1/crates/vxn-wasm/src/codec.rs), retargeted from
`vxn_engine::Synth` to [`vxn1b_engine::Engine`](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L225).

The 2026-08-24 spike confirmed `vxn1b-engine` and its whole tree build for
`wasm32-unknown-unknown --release` with zero source changes, so this is glue.

`Engine` is already the facade the CLAP shell drives — it owns both synths, the
global block, the demux and the FX chain behind `process_block(l, r)` — so the
worklet host wraps **one** `Engine`, not two synths plus a mixer. The render loop
is the same shape as
[vxn1b-clap's `process`](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L586): apply
non-automatable state, then slice the block at event offsets.

## Design

### Wire format

The 16-byte slot framing is **unchanged** from vxn-1's 0035/0037 — one framing
across all three synths — and tags 1–10 keep their meaning byte-for-byte. VXN1b
reclaims the two vxn-2 left reserved and adds six:

| tag | event | fields |
|---|---|---|
| 1 | `note_on` | `value` = velocity, `note` = key, **`flag` = MIDI channel** |
| 2 | `note_off` | `note` = key, **`flag` = MIDI channel** |
| 3 | `param` | `paramIdx` = clap id, `value` = plain/norm, `flag` = norm bit |
| 4 | `pitch_bend` | `value` ∈ [-1, 1] |
| 5 | `mod_wheel` | `value` ∈ [0, 1] |
| 6 | *reserved* | vxn-1's `sustain` — see below |
| 7 | `key_mode` | `flag` = mode (0 Single, 1 Dual, 2 Split) → `KeyOp::SetKeyMode` |
| 8 | `split_point` | `flag` = note → `KeyOp::SetSplitPoint` |
| 9 | `gesture_begin` | `paramIdx` = id (controller concern; no-ops on the engine) |
| 10 | `gesture_end` | `paramIdx` = id (ditto) |
| 11 | `lfo2_link` | `flag` 0/1 → `KeyOp::SetLfo2Link` |
| 12 | `matrix_edit` | `paramIdx` = `layer<<12 \| slot<<8 \| field`, `flag` = value byte |
| 13 | `scope_tap` | `flag` = `ScopeTap` code (0 Off, 1 Layer1, 2 Layer2) |
| 14 | `tempo` | `value` = BPM |
| 15 | `poly_pressure` | `note` = key, `value` ∈ [0, 1], `flag` = channel |
| 16 | `channel_pressure` | `value` ∈ [0, 1], `flag` = channel |

**Two corrections to the plan, from reading the engine before writing the wire.**

- *There is no sustain event.* This ticket was scoped assuming vxn-1's tag 6
  ported across. It does not: `vxn1b-clap`'s bespoke `dispatch` has **no CC64
  path at all** — the plugin ignores sustain. Adding one on the web wire would
  make the browser build behave differently from the plugin, so tag 6 stays
  reserved-unused and decodes to `None`, the way vxn-2 reserved 7/8.
- *There is no layer-copy event.* `copy_layer` is a `SharedParams` operation,
  not an `Engine` one — it rewrites params and topology in the **model**. On the
  web that model lives in the controller (0287), so a copy reaches the worklet as
  ordinary param writes plus `matrix_edit` records. A dedicated tag would have
  had nothing to call.

Three of these are VXN1b-shaped rather than mechanical ports:

- **Channel on note events.** VXN1b's CLAP dispatch is deliberately MPE-aware
  (per-note pitch and pressure); vxn-1's codec has no channel field at all
  because its dispatch is channel-agnostic. `flag` is unused on tags 1/2 in the
  existing format, so the channel goes there — no framing change, and a producer
  that writes 0 gets channel 0, which is what a non-MPE source wants.
- **Matrix topology on the ring.** Native, topology lives in `SharedParams`
  behind a `Mutex<[MatrixTable; 2]>` shared with the audio thread. The worklet
  has its own linear memory and cannot see that, so a `MatrixEdit` has to travel.
  Depth stays a normal CLAP param on tag 3 — that is the whole point of the
  0219 split, and it must not regress here.
- **Tempo.** `sync.rs` resolves LFO/delay subdivisions against host BPM. The
  browser has no host, so BPM arrives as an event from a UI control (0294),
  defaulting to `DEFAULT_TEMPO_BPM`.

### Render loop

Per quantum, mirroring the plugin:

1. Fold the param store (block-start, from the SAB) — the `LocalParams` analogue.
2. Apply pending non-automatable state once, before event ingestion.
3. Slice at each record's sample offset: apply every event at offset `k`, render
   `[prev..k)`, advance; render the tail.

**Ordering rule.** A preset load changes params *and* topology together. Topology
events therefore apply at their offset in the same slice loop as params, never
hoisted to block start — otherwise a slot briefly routes the new source at the
old depth (or the reverse), which is audible on a matrix-heavy patch.

### Out of scope

The JS twin of the codec (0287, keeps the golden table byte-identical), the
meter/scope **return** channel (0288 — this ticket only accepts `scope_tap`
pointing the ring; reading frames back out is the new transport), and any
`vxn1b-engine` change.

## Acceptance criteria

- [ ] `vxn-1b/crates/vxn1b-wasm` exists as a `cdylib`, in the workspace, with
      `vxn1b-engine` as its only dependency.
- [ ] Builds for `wasm32-unknown-unknown --release` with
      `RUSTFLAGS=-C target-feature=+simd128`; every `vxn1b_host_*` export present
      in the artifact.
- [ ] `codec.rs` encodes/decodes all 16 live tags; a golden byte table pins the
      wire format (the contract 0287's JS half is written against).
- [ ] Unknown **and reserved** tags decode to `None` (forward-compat).
- [ ] Id-layout constants come from `vxn1b_engine::params`, never literals — a
      test asserts `TOTAL_PARAMS` agrees with the engine (this is exactly the
      drift [0285](0285-web-param-mirror-drift.md) was).
- [ ] `host.rs` renders a quantum with sample-accurate slicing; a test proves an
      event at offset `k` is inaudible before `k` and audible after.
- [ ] A test proves a `matrix_edit` retargets a slot, leaves the slot's **depth
      param untouched**, and does not touch the other layer's same-numbered slot.
- [ ] A note on channel 3 with per-note pressure reaches the engine as channel 3.
- [ ] `cargo test -p vxn1b-wasm` green; `cargo test --workspace` green.

## Notes

- Reference: `vxn-1/crates/vxn-wasm/src/{host,codec}.rs`; vxn-2's port
  (`vxn-2/crates/vxn2-wasm/src/`) is the closer relative for crate shape, but
  vxn-1's is the closer relative for the *event set* (vxn-2 dropped tags 7/8).
- `Engine::process_block` takes the whole slice; check whether it has a
  max-frames or control-block constraint like vxn-2's `CONTROL_BLOCK` assert
  before assuming an arbitrary slice length is legal.
- No `cargo fmt` — [[vxn-no-cargo-fmt]]. Stage explicit paths —
  [[vxn-concurrent-vxn2-work-no-git-add-all]]. One `cargo test` at a time —
  [[vxn-no-parallel-cargo-test]].
- Blocks 0287 (JS codec twin) and 0289 (worklet + coordinator).
