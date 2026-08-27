# ADR 0009 — Web controller placement & cross-thread param store

- **Status:** Accepted
- **Date:** 2026-06-14
- **Scope:** Decides, for the vxn-1 browser/WASM port (epic E015), where the
  MVC controller runs (main-thread Rust-wasm reusing `vxn-app` vs a JS
  reimplementation) and how parameters cross the main-thread ↔ AudioWorklet
  boundary (a `SharedArrayBuffer` atomic array vs param-set events on the
  0035 event ring). Pins the concrete 165-id CLAP param addressing the codec
  (0037) and store (0039) build against. Does not change `vxn-engine` DSP,
  the param table, or the native CLAP shell.

## Context

Spike [0034](../../tickets/closed/0034-vxn1-wasm-spike.md) proved
`vxn-engine` compiles to `wasm32-unknown-unknown` with zero source changes
and renders inside an `AudioWorkletProcessor`. The hard part it skipped is
the boundary: the browser splits the **controller (main thread)** from the
**renderer (AudioWorklet thread)**, where CLAP hands `process()` a
sample-accurate event list on the audio thread itself. E015 replicates that
boundary over a `SharedArrayBuffer` instead of the CLAP host ABI.

Two architecture forks gate every E015 scaffold (0037–0039) and ripple into
E016 (host shell) and E018 (UI bridge). This ADR decides both, grounded in
two throwaway probes under
[`crates/vxn-app-wasm-probe`](../crates/vxn-app-wasm-probe/) (ticket 0036).

### What exists today (the reference design)

- **The MVC controller** ([[vxn1-mvc-architecture]], ADR 0007) is the sole
  off-audio mutator of the model. `vxn_app::Controller` wraps
  `vxn_core_app::Controller<M>`; it owns bounded `std::sync::mpsc` channels
  (`UiEvent` in, `HostEvent` in, `ViewEvent` out) and an `Arc<Mutex>` corpus
  snapshot. On a single-threaded main thread the `Arc<Mutex<Controller>>` of
  the native shell collapses to a plain owned value (no second thread touches
  it; `Mutex` → effectively `RefCell` discipline).
- **Params are lock-free via `SharedParams`** (one `AtomicU32` per CLAP id,
  f32 bit-cast): audio reads, main writes. A timer-tick **param-diff pump**
  (`vxn-clap/src/lib.rs` `push_param_diffs`) scans the store against a
  main-thread mirror and emits `ViewEvent::ParamChanged` for any audio-thread
  write the controller never processed (host automation echo, meters).
- **165 CLAP param ids** address the whole model (see §3).

## Probe results

Both probes are reproducible from
[`crates/vxn-app-wasm-probe/README.md`](../crates/vxn-app-wasm-probe/README.md).
Measured on M1, rustc 1.95, Node 26.

### Controller-in-wasm

A probe crate (`cdylib`, depends on `vxn-app`) constructs a **real**
`vxn_app::Controller` with throwaway `ParamModel` + `PresetStore` impls,
posts `UiEvent`s through the bounded channel, `tick()`s, and drains
`ViewEvent`s — forcing the whole controller code path into the binary, not
just its type signatures.

- **It compiles to `wasm32-unknown-unknown` with no source changes** to
  `vxn-app` / `vxn-core-app`. The only edit was edition-2024 `#[no_mangle]`
  syntax in the probe's own C-ABI shims.
- The controller dependency tree is wasm-clean: `vxn-core-app` pulls only
  `serde`; `vxn-core-utils` has zero deps. `std::sync::mpsc`, `Arc<Mutex>`,
  `PathBuf` and `Box<dyn Any>` all compile on wasm. No threads are spawned;
  the channels are drained synchronously in `tick()`.
- **Size: 160 872 bytes raw, ~57 KB gzipped.** For comparison the 0034
  engine spike is 172 KB raw — the controller is the same order of magnitude,
  i.e. a *second* wasm module roughly doubles the wasm payload.

### Param store

A Node prototype (`param_store_bench.mjs`) compares the two mechanisms on the
two cases the boundary stresses:

| case | (a) SAB 165 atomics | (b) param-events on ring |
|---|---|---|
| bulk preset load (165 params at once) | **~1 µs** | ~84 µs |
| diff readback, 8 drifted params/tick | ~4 µs | ~4 µs |
| diff readback, all 165 drifted | ~84 µs | ~77 µs |

- **Bulk load: SAB is ~50–80× faster.** SAB is a flat 165-element atomic
  store (memcpy-class write the worklet reads lock-free, latest-value-wins).
  The ring must frame 165 length-prefixed records and the consumer must
  drain+decode each — overhead 165 records cannot escape.
- **Diff readback is a wash** either way: the dominant cost is the per-param
  work (one echo/scan per changed id), not the transport.
- Critically, **option (b) loses the "latest value wins / lock-free read"
  property the audio thread relies on**: a ring is a *stream*, not a
  *current-value store*. To answer "what is param N right now?" on the audio
  thread you must replay the stream into a local mirror; to observe
  audio-thread drift on the main thread (the diff pump's job) you need a
  *second* (audio→main) ring carrying echoes — there is no shared current
  value to scan. SAB gives both directions a single authoritative array.

## Decision

### 1. Controller placement — reuse `vxn-app` as a main-thread wasm module

The web host runs the **existing Rust MVC controller compiled to wasm on the
main thread**, not a JS reimplementation. It is a second wasm module
(alongside the engine wasm in the worklet), instantiated with a plain
`WebAssembly.instantiate` and a hand-written C-ABI glue layer (no
wasm-bindgen — same approach as 0034, which keeps the module worklet/scope
clean).

Rationale:

- **The controller is non-trivial, audio-correct logic we do not want to
  fork.** Gesture bracketing, the host-automation-during-gesture suppression
  rule, the EditorReady re-broadcast, the key-mode/split republish, the
  preset-step walker — all are subtle and already tested. A JS rewrite is a
  second source of truth for model mutation, exactly what ADR 0007 exists to
  prevent ([[vxn2-mvc-discipline]]).
- **It builds today, unchanged, at acceptable size** (57 KB gzipped). The
  cost feared in the ticket — "a second wasm module" — is real but small
  relative to the engine, and it is a one-time download, cached.
- **VXN-2 inherits the win.** `vxn_core_app::Controller` is engine-agnostic;
  the same web shell pattern serves vxn-2's eventual port for free.

**Rejected: reimplement the controller in JS, keep wasm to the engine.**
Cheaper in module count and removes JS↔wasm marshalling of `UiEvent` /
`ViewEvent`. Rejected because it duplicates the model-mutation arbiter in a
second language, drifts from the native shell over time, and discards tested
behaviour — a high-blast-radius, low-new-value move, the same trade we
rejected for `nih-plug` in ADR 0008 §Context.

#### Event marshalling across the JS boundary (consequence for E018)

`UiEvent` / `ViewEvent` (`vxn-core-app/src/events.rs`) do not cross the
JS↔wasm boundary as Rust enums. The glue layer marshals them as a small,
explicit C-ABI opcode surface — the bridge E018 builds against:

- **Hot path (per gesture / per knob):** flat extern-C calls, no allocation.
  `ui_set_param_norm(id: u32, norm: f32)`, `ui_begin_gesture(id)`,
  `ui_end_gesture(id)`, `ui_editor_ready()`. These map 1:1 to
  `UiEvent::{SetParamNorm, BeginGesture, EndGesture, EditorReady}`.
- **`ViewEvent` readback:** the controller drains its `view_rx` inside a
  `tick()` export and writes results into a wasm-memory scratch buffer the
  JS side reads — `ParamChanged{id, plain, norm, display}` becomes a packed
  record array (the `display` string as a length-prefixed UTF-8 slice). JS
  copies it out per tick; this is the web analogue of the native
  `flush_view_events` single-bridge-call discipline.
- **Per-synth `Custom` payloads** (`Vxn1UiCustom::SetKeyMode` / `SetSplitPoint`
  / `SetEditLayer` / `ResetLayer`; `Vxn1ViewCustom::*`) get dedicated
  narrow opcodes (`ui_set_key_mode(u8)`, `ui_set_split_point(u8)`, …) rather
  than crossing `Box<dyn Any>`. The downcast stays inside wasm.
- **Preset/file management `UiEvent` variants** (`LoadPreset`, `SavePreset`,
  `RenamePreset`, `PathBuf`-bearing ops) do **not** cross as-is — they assume
  a filesystem `PresetStore`. The web `PresetStore` impl is backed by
  IndexedDB/JS and is wired in E019; until then the controller takes a
  `NullStore` (as the probe does). This is out of scope for E015.

### 2. Cross-thread param store — `SharedArrayBuffer` of 165 atomics

Parameters cross threads as a **`SharedArrayBuffer` of 165 `Int32` atomics
indexed by CLAP id** (f32 bit-cast, `Atomics.load`/`store`) — the direct
analogue of today's `SharedParams`. Both wasm modules and the JS glue map
the *same* SAB; the worklet reads lock-free (latest-value-wins, no
`Atomics.wait`), the controller-side glue writes.

Rationale:

- **Bulk preset load is ~50–80× faster** and is the case that stresses the
  choice (E019 applies 165 params at once).
- **It preserves lock-free, latest-value-wins reads on the audio thread** —
  the property the renderer depends on; a ring cannot provide a current value
  without replay.
- **It is the smallest delta from the proven native design.** The param-diff
  pump ports almost verbatim: the controller-side glue keeps the
  `last_seen[165]` mirror and scans the SAB each tick, emitting
  `ParamChanged` for drift — exactly `push_param_diffs`, with `Atomics.load`
  in place of the native atomic read.

**This solves the "two wasm memories don't share by default" risk** (E015
Risks): the controller wasm and the engine wasm have *separate linear
memories*, so neither can see the other's heap. The param SAB is a **third,
dedicated shared buffer both threads map** — it is the agreed contract
between them, independent of either module's linear memory. We do **not**
pursue a single shared-memory (`atomics`+`bulk-memory`, shared `Memory`)
build of the engine for params: it would force a SIMD/threads feature build
and an audit of `Synth` for shared-memory safety, for no benefit the
dedicated SAB doesn't already give.

**Rejected: param changes as ordinary events on the 0035 ring.** Attractive
because it unifies all main→audio traffic on one transport (notes, gestures,
*and* params) and needs no second shared buffer. Rejected because (i) bulk
load is ~50–80× slower, (ii) it loses the lock-free current-value read the
audio thread needs — a ring is a stream, so the worklet would have to
maintain its own param mirror and the diff-readback needs a second
audio→main ring, and (iii) it diverges from the native `SharedParams` design
the diff pump already implements. Note the two transports are
**complementary, not exclusive**: *sample-accurate* param automation (a param
change that must land at a specific sub-block offset) still rides the 0035
ring as a `param-set` event so the worklet can apply it at the right slice;
the SAB carries the *current value* for lock-free reads and the diff
readback. The ring's `param-set` record and the SAB write the same f32 to the
same CLAP id — they are two views of one value, not two stores.

### 3. Param addressing — the 165-id CLAP layout (pinned)

The codec (0037) and store (0039) address parameters by a **flat `u32` CLAP
id in `[0, 165)`**, laid out contiguously. This is exactly today's
`vxn-app/src/params.rs` layout (verified against the source and its
`clap_id_layout_is_contiguous_and_invertible` test); the web port reuses it
unchanged so native and web share one addressing scheme.

```text
counts:  PATCH_COUNT  = 69   (PatchParam::Osc1Wave .. Spread)
         GLOBAL_COUNT = 27   (GlobalParam::MasterTune .. ReverbMix)
         LAYER_COUNT  = 2    (Upper, Lower)
         TOTAL        = 2*69 + 27 = 165

id ranges:
  [  0 ..  69 )   Upper layer per-patch params  (clap_id = patch_index)
  [ 69 .. 138 )   Lower layer per-patch params  (clap_id = 69 + patch_index)
  [138 .. 165 )   global params                 (clap_id = 138 + global_index)

mapping (matches params.rs):
  patch_clap_id(layer, p) = (layer as usize) * 69 + (p as usize)
  global_clap_id(g)       = 2 * 69 + (g as usize) = 138 + (g as usize)

inverse param_ref(id):
  id <  69          -> Patch(Upper, PatchParam::from_index(id))
  69 <= id < 138    -> Patch(Lower, PatchParam::from_index(id - 69))
  138 <= id < 165   -> Global(GlobalParam::from_index(id - 138))
```

- The SAB is `Int32Array(165)`; word `i` holds the f32 *plain* value bits of
  CLAP id `i`. (Normalisation/taper lives in `ParamDesc`; the store carries
  plain values, as `SharedParams` does.)
- **Non-automatable shared state is NOT in the 165** — `KeyMode` and split
  point (ADR 0003 §3) travel out-of-band, carried as dedicated codec opcodes
  on the 0035 ring (key-mode, split-point) and as narrow controller glue
  calls (§1), mirroring the native "set once per block before event
  ingestion" rule. They are not param ids and never occupy an SAB slot.
- **Stability:** CLAP-id stability is not a hard constraint
  ([[vxn1-id-stability-dropped]]); the layout is derived from the enum
  discriminant order in `params.rs`, which is the single source of truth.
  0037/0039 must read the count constants (`PATCH_COUNT`, `GLOBAL_COUNT`,
  `TOTAL_PARAMS`) from `vxn-app`, not hard-code `69/27/165`, so a future
  param add/remove flows through.

## Consequences

**Positive.**

- One source of truth for model mutation across native and web — the tested
  controller logic is reused, not reforked (ADR 0007 discipline holds online).
- Bulk preset load and lock-free audio reads both stay fast; the diff pump
  ports almost verbatim.
- The param SAB cleanly sidesteps the two-wasm-memories problem without a
  shared-memory engine build.
- VXN-2's eventual web port inherits the controller-in-wasm pattern.

**Negative.**

- A second wasm module (~57 KB gzipped) and a hand-written C-ABI glue layer
  for `UiEvent`/`ViewEvent` marshalling (E018). One-time download, but real
  surface area to maintain.
- Two transports to keep coherent: the SAB (current value) and the ring's
  `param-set` record (sample-accurate apply) write the same value to the same
  id — 0037/0038 must ensure they agree.
- The web `PresetStore` (IndexedDB-backed) is new work (E019); until it
  lands the controller runs with a `NullStore` and preset ops are inert.

**Risks / open questions.**

- The SAB requires cross-origin isolation (COOP/COEP) like the 0035 ring —
  quantified by 0035, not re-litigated here.
- `Atomics.wait` is forbidden on the worklet thread; the param read path is
  pure `Atomics.load` (no wait), so this is satisfied by construction.
- C-ABI string marshalling for `ParamChanged.display` (length-prefixed UTF-8
  out of wasm memory) is straightforward but must be implemented carefully to
  avoid per-tick allocation churn — fold into the single-bridge-call
  discipline in 0038/E018.

## Downstream consequences (per ticket / epic)

- **0037 (binary event codec):** addresses params by the §3 flat `u32` id;
  carries a `param-set` (plain) and `param-set-norm` record plus out-of-band
  key-mode / split-point opcodes. Reads counts from `vxn-app`, not hard-coded.
- **0038 (worklet audio-host):** reads the param SAB lock-free
  (`Atomics.load`) when applying a block; applies ring `param-set` events at
  their sub-block offset for sample accuracy (both write the same f32/id).
- **0039 (param store):** implements the §2 SAB of 165 atomics + the
  audio→main diff readback (port of `push_param_diffs`, `last_seen[165]`
  mirror, SAB scan).
- **E016 (host shell):** instantiates *two* wasm modules (controller on main,
  engine in worklet) and owns the shared param SAB lifecycle.
- **E018 (UI bridge):** marshals `UiEvent`/`ViewEvent` over the §1 C-ABI
  opcode surface; no JS reimplementation of controller logic.

## Out of scope

- The 0035 ring framing / SPSC correctness / COOP/COEP serving — that spike.
- The web `PresetStore` (IndexedDB) and persistence — E019.
- Implementing the store or the controller shell — 0039 / E016.
- Web MIDI / keyboard input — E017.
- The `vxn-app-wasm-probe` crate is throwaway: delete it and its workspace
  member line once 0039 lands; the decision lives on in this ADR.

## Addendum (2026-06-21, E019 / 0062–0063): web preset persistence

E019 ports presets to the browser; two choices landed here since they extend
this ADR's "controller deps only `vxn-app`" / cross-thread-store framing.

- **Factory bank: build-time baked asset, not the engine in the controller**
  (0062). Pulling `vxn-engine` into the lean main-thread controller wasm to read
  the embedded bank would violate §1's intent. Instead xtask's web build runs
  `vxn-engine`'s `bake-factory` bin → a flat `factory.bin` (`vxn-app::factory_asset`
  codec, 29 presets); JS fetches it at boot and feeds it to the controller via
  `vxnc_load_factory`. The shared `PluginState` codec moved to `vxn-app::state`
  and the corpus→JSON projection to `vxn-core-app` so the controller builds the
  byte-identical browser payload native does.

- **User presets: IndexedDB, binary-blob format** (0063). Storage backend is
  **IndexedDB**, not OPFS: the corpus is small key→value blobs, IndexedDB is
  universal, and a flat store fits better than OPFS's file tree. Each preset is
  stored as the canonical `vxn-app::state` blob + its `PresetMeta`
  (`vxn-app::preset_record` codec), keyed by a synthetic `folder/Name.toml` path;
  empty folders persist as their own keys. **The web does NOT reuse the desktop
  TOML preset format** — that codec (`vxn-engine::preset`) is engine-coupled and
  not worth hoisting to wasm, so web user presets are their own world; a
  desktop-saved `.toml` does not parse on web. Cross-platform sharing is the 0066
  export/import path. Name/folder sanitisation is shared with desktop via
  `vxn-app::preset_names` so the two backends can't drift. The synchronous
  `PresetStore` reads/writes an in-memory cache (`vxn-web-controller::user_store`);
  boot-hydration timing and deferred-write flush to IndexedDB are 0064.

## References

- ADR 0003 §3 — key mode / split as non-automatable shared state (not params).
- ADR 0007 — MVC architecture; controller is the sole model mutator.
- Ticket 0036 — this spike; probe under `crates/vxn-app-wasm-probe/`.
- Ticket 0034 — engine WASM feasibility spike (172 KB engine wasm baseline).
- Epic E015 — web event-driven core (0035 ring, 0037–0040 scaffolds).
- [[vxn1-mvc-discipline]] / [[vxn2-mvc-discipline]] — single-arbiter rule.
- [[vxn1-id-stability-dropped]] — CLAP-id stability is not a hard constraint.
- `vxn-app/src/params.rs` — the 165-id layout, single source of truth.
