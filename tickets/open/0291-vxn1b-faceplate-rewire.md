---
id: "0291"
product: vxn-1b
title: "Faceplate rewire: controller.mjs + faceplate-bridge.mjs replacing wry IPC"
priority: medium
created: 2026-08-25
epic: E045
depends: ["0290"]
---

## Summary

Seventh ticket of [E045](../../epics/open/E045-vxn1b-web-wasm-browser-port.md),
and the one that makes the port a playable instrument: the page and the two wasm
modules currently have no path between them.

Everything either side of the gap now exists. The page
(`build_web_faceplate_html()` + `gen-web-page`) speaks one narrow protocol to
its host, defined in
[bridge.js](../../vxn-1b/crates/vxn1b-ui-web/assets/bridge.js):

- **Out:** `window.ipc.postMessage(JSON.stringify({op, …}))` — one string `op`
  per intent, from a typed `window.vxn.send.*` façade. There is exactly one
  `postMessage` call site ([bridge.js:29](../../vxn-1b/crates/vxn1b-ui-web/assets/bridge.js#L29)).
- **In:** `window.__vxn.applyViewEvents(arr)` once per tick, and
  `window.__vxn.applyPresetCorpus(snap)` on a corpus change. Both buffer until
  `init()` swaps in the real dispatcher, so early events are not lost.

Natively wry provides the first and `evaluate_script` the second. This ticket
provides both from JS: `controller.mjs` (a thin wrapper over
[0290](../closed/0290-vxn1b-web-controller-cdylib.md)'s 48 wasm exports) and
`faceplate-bridge.mjs` (the router + pump). Ports vxn-2's pair (549 + 879
lines), which do the same job for a different model shape.

## Design

### The event-shape contract is fixed, and it is not ours to choose

The page's dispatcher already switches on `kind`
([dispatch.js:926-1080](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L926-L1080)).
The bridge must reproduce, byte for byte in field names, what the native
serialiser emits — the shared core's
([vxn-core-ui-web lib.rs:685-730](../../crates/vxn-core-ui-web/src/lib.rs#L685-L730))
for `param_changed` / `preset_loaded` / `preset_corpus_changed` / `status` /
`text_input_result`, and VXN1b's own
([vxn1b-ui-web lib.rs:181](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L181)) for
`matrix` / `keys` / `meters` / `scope`. Decoding 0290's binary batch into those
objects is most of `faceplate-bridge.mjs`.

Note `preset_loaded.source` is a nested object (`{kind:"factory",index}` /
`{kind:"user",path}` / null), not the flat u32 the wire carries — the browser
panel's highlight and the preset bar's overwrite button both read it.

### Opcode routing: three destinations, and the copy/load cases are the work

**Revised while building (2026-08-25).** 0290's ticket described key/matrix ops
as going to "both" the controller and the ring. Implementing that double-pushed
every UI topology edit — once at route time, once from the echo resend below —
and bought only one frame of latency, on a path where param edits already wait a
frame (they reach the SAB on the pump, not on the click). The rule that actually
holds is simpler:

**Does the opcode have a presence in the model?**

| opcode | destination | reaches the engine via |
|---|---|---|
| `set_param`, `set_param_norm` | controller only | the store-SAB mirror, on the pump |
| `begin_gesture`, `end_gesture` | controller only | nothing — the engine treats gestures as a no-op, and there is no host to bracket for |
| `set_key_mode`, `set_split_point`, `set_lfo2_link`, `set_matrix` | controller only | the echo resend, on the pump |
| `copy_layer`, preset/folder ops, `ready` | controller only | mirror + echo resend |
| `set_scope_source` (and tempo, 0294) | **ring only** | directly — no model presence, so no echo could carry it |
| `request_text_input` | neither | answered in-page |
| `reset_layer`, `set_edit_layer` | dropped | dead fork artifacts — see [[0307]] |

The hard part is not the table, it is that **three controller-only paths mutate
engine state the ring never heard about**: a preset load, a state restore, and
`copy_layer` all rewrite the matrix topology and the key record. Natively one
`SharedParams` is visible to both threads and this problem does not exist.

The clean answer is to let the echoes drive the resend: 0290 emits
`VE_MATRIX_SNAPSHOT` / `VE_KEY_STATE` exactly when those change, from any cause.
So the bridge treats an inbound matrix/key record as **both** a view event and an
engine-resync trigger, diffing it against what it last put on the ring and
pushing `EV_MATRIX_EDIT` / `EV_KEY_MODE` / `EV_SPLIT_POINT` / `EV_LFO2_LINK` for
the fields that moved. One mechanism covers load, restore, copy and undo, and
nothing has to enumerate the causes.

Worst case checked rather than assumed: a full topology resend is 2 layers × 16
slots × 4 fields = **128 slots**, plus 3 for the key record. The ring is 1024
slots ([event-ring.mjs:64](../../vxn-1b/crates/vxn1b-wasm/web/event-ring.mjs#L64))
and params ride the SAB rather than the ring, so 131 fits one block with room to
spare and no bulk tag is needed. The block-writer policy means a full ring would
stall the producer rather than drop musical events, so this is worth keeping true
as the resend grows.

### Coherence: depths and topology must land together

E045's named risk. Slot depths are CLAP params (param SAB); slot topology is a
ring event. A preset load moves both, and if they reach the worklet in different
blocks a slot briefly routes the **old source at the new depth** — an audible
wrong-modulation blip, not a cosmetic one. Sequence the mirror and the ring push
within one tick, and say in a comment which lands first and why.

### Text input has no native popup here

`request_text_input` exists because the native editor needs an NSWindow outside
the host's event monitor. In a browser the page can prompt itself. The bridge
should answer the opcode locally and synthesise the `text_input_result` event the
dispatcher already expects
([dispatch.js:1042](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L1042)),
rather than routing it to the controller (whose `OpenTextInput` /
`TextInputResult` variants 0290 deliberately does not pack).

### Checked: vxn-2's numeric-`op` collision does not recur

E045 flagged it. vxn-2's page sends `{op: <number>}` for the operator tab, so its
`routeOpcode` has to sniff the type before switching
([faceplate-bridge.mjs:99](../../vxn-2/crates/vxn2-wasm/web/faceplate-bridge.mjs#L99)).
VXN1b has no operators: a grep of the whole asset tree finds no non-string `op:`
posted, and one `postMessage` call site. Route on the string directly and add a
test that an unknown or non-string `op` is dropped rather than mis-routed.

### Corpus is available at boot

Unlike vxn-1 and vxn-2, there is no `factory.bin` to fetch: 0290 embeds the bank
and publishes the corpus JSON during `vxnc_new`. `applyPresetCorpus` can fire on
the first pump rather than waiting on a network round trip, and there is no
"factory list is empty until the asset lands" window to design around.

### Pump

One `requestAnimationFrame` loop: `vxnc_tick()` → decode the batch → append the
telemetry frames read from 0288's seqlock SAB → one `applyViewEvents` call →
mirror `vxnc_values_ptr` into the param SAB → resend any topology/key drift to
the ring. Meters and scope do **not** come through the controller (they are
audio-thread data on their own channel), so they are appended to the batch by the
bridge, matching what the native shell does when it pushes them into the same
`evaluate_script`.

## Acceptance criteria

- [ ] `controller.mjs` instantiates the 0290 wasm (no wasm-bindgen, plain
      `WebAssembly.instantiate`) and wraps every opcode it exposes; a test
      asserts the wrapper's method set matches the module's exports, so a new
      export cannot be added Rust-side and silently go unwired.
- [ ] String args round-trip through `vxnc_arg_buf_reserve` (TextEncoder →
      staging buffer → opcode lengths), including the `ARG_NONE` root-folder
      sentinel and a non-ASCII preset name.
- [ ] The binary view batch decodes to the exact objects the page's dispatcher
      expects — one golden test per `kind`, including `preset_loaded`'s nested
      `source` object and its `warnings` array.
- [ ] Every `window.vxn.send.*` opcode routes per the table above; a test pins
      it, **including that the non-param state ops reach the ring** (the half
      [0290](../closed/0290-vxn1b-web-controller-cdylib.md) could not test —
      here they arrive via the pump's resend, not at route time) and that
      `set_scope_source` never reaches the controller.
- [ ] A non-string or unknown `op` is dropped, not mis-routed.
- [ ] A preset load resends topology + key state to the ring: a test drives a
      factory load and asserts the ring received the slot edits, not just the
      param writes.
- [ ] `copy_layer` does the same — params via the mirror, topology via the
      resend — proving the controller-only op still reaches the engine.
- [ ] A test covers the full-table resend (128 slots + key), confirming it lands
      in one block against the 1024-slot ring.
- [ ] `request_text_input` is answered in-page and produces a
      `text_input_result` the dispatcher consumes; nothing is posted to the
      controller.
- [ ] Corpus renders from the embedded bank with no fetch.
- [ ] Meters and scope animate from the telemetry SAB through the same batch.
- [ ] VXN1b web node suite green, **0 skipped** ([[0295]]'s posture);
      `cargo test --workspace` still green.
- [ ] Browser pass (the one 0290 could not do): boot, play from the on-screen
      keyboard, move a fader, flip a sync toggle and see the label change, load a
      factory preset, copy Layer 1 → 2, save and reload a user preset.

## Notes

- Reference: `vxn-2/crates/vxn2-wasm/web/{controller.mjs,faceplate-bridge.mjs}`.
  Both are per-synth by design ([E045](../../epics/open/E045-vxn1b-web-wasm-browser-port.md)'s
  third-fork table puts them in the "genuinely divergent, stay forked" rows) —
  the shared modules from 0284 are the input adapters and persistence, not these.
- Demo posture ([[0297]]): the faceplate needs to work, not to survive eight
  hours. No reconnection logic, no state recovery beyond what 0293 gives.
- 0289's deliberate gap — "a render trap rebuilds the engine and the controller
  re-broadcasts" — is **dropped**: [[0297]] removed the rebuild, so a trap is a
  reload. Do not build a re-broadcast-on-trap path.
- Persistence (IndexedDB, autosave, patch export/import) is [0293], not this
  ticket. The bridge should drain `vxnc_take_journal` and hand the ops over, but
  wiring them to storage is 0293's.
- No `cargo fmt` — [[vxn-no-cargo-fmt]]. One `cargo test` at a time —
  [[vxn-no-parallel-cargo-test]]. Stage explicit paths —
  [[vxn-concurrent-vxn2-work-no-git-add-all]].
- Blocks 0292 (which bundles these modules into `dist/`).
