---
id: E045
product: vxn-1b
title: "VXN1b web/wasm browser port"
status: open
created: 2026-08-24
---

> Third run of the browser-port blueprint (vxn-1 E015–E019 / ADR 0009,
> vxn-2 [[E030]]). Like E030 this skips the spike-scaffold rhythm: a
> 2026-08-24 compile spike confirmed `vxn1b-engine` (and its whole tree —
> `vxn-dsp`, `vxn-core-utils`, `vxn-core-app`, `vxn-preset`, `include_dir`)
> builds for `wasm32-unknown-unknown --release` with **zero source
> changes**. The work is glue, transport, build pipeline and UI rewire —
> not core changes.
>
> Unlike the first two ports, VXN1b carries **two audio→view telemetry
> streams** (meter + scope, ticket 0240) and **non-param topology state**
> (per-layer matrix) that neither prior port had to move across the
> worklet boundary. Those are the genuinely new engineering here.

## Goal

Ship VXN1b as a browser instrument reachable from a static URL, feature-
matched to the CLAP build: the 3-tab faceplate, both layers' matrices,
meters + scope, factory bank, user presets, MIDI/keyboard input.

When this epic closes:

- `cargo run -p vxn1b-xtask -- web` produces a self-contained `dist/`
  (two wasm modules, JS glue, worklet, faceplate page, baked factory bank,
  `_headers`) in one command; `--serve` serves it cross-origin-isolated.
- The served page boots an AudioContext, runs the engine in an
  AudioWorklet, and plays from Web MIDI / computer keyboard.
- Every faceplate gesture round-trips: params, matrix topology, key
  mode/split, LFO2 link, layer copy, scope tap.
- Meters and the scope animate from real audio-thread data.
- User presets persist in IndexedDB; state autosaves; patches
  export/import/share via URL.
- Deployed alongside vxn-1 and vxn-2 on the site without clobbering their
  `_headers` ([[vxn-web-publish-flow]]).

## Architecture (ADR 0009 blueprint, unchanged)

Two-module wasm, no wasm-bindgen, raw C ABI:

1. **`vxn1b-wasm`** (new cdylib) — `Synth` ×2 + global block in the
   AudioWorklet. Per-quantum render loop + binary event codec. Ports
   `vxn-wasm/src/{host,codec}.rs`, retargeted to `vxn1b_engine`.
2. **`vxn1b-web-controller`** (new cdylib) — `vxn_core_app::Controller<
   SharedParams>` on the main thread, the **same** arbiter `vxn1b-clap`
   drives. Ports `vxn2-web-controller` (closer relative than vxn-1's,
   which wraps the bespoke `vxn-app`).

They share `SharedArrayBuffer`, not linear memory: an SPSC event ring
(16-byte slots) main→worklet, a lock-free param store, and — new here —
a **telemetry channel** worklet→main for meter/scope frames.

## VXN1b deltas vs the vxn-1 / vxn-2 blueprint

These are architecture, not choices. Each is called out in the ticket
that owns it.

1. **Meter + scope must cross the worklet boundary.** Native, `MeterBus`
   / `ScopeBus` are `Arc`-shared between the audio thread and the ~60 Hz
   timer tick, and the frames ride the existing `ViewEvent` batch for
   free. In the browser the engine is a *separate wasm in a separate
   thread with separate linear memory*, so the buses' read side is
   unreachable from the controller. Needs a return SAB (meter: ~13 f32
   read-and-clear; scope: `SCOPE_WINDOW = 384` f32 at half tick rate).
   Neither prior port has an audio-data return path — the CPU-load badge
   rides `port.postMessage`, which is the wrong shape for 60 Hz frames.
   **This is the highest-novelty ticket in the epic.**

2. **Matrix topology is non-param state on the audio path.** Native it
   lives in `SharedParams` behind a `Mutex<[MatrixTable; 2]>`; the shell
   pushes `MatrixSnapshot` echoes on the timer diff. On the web the
   worklet holds its own copy, so `MatrixEdit` (layer/slot/field/value)
   needs a ring event tag, and the load/copy paths need a bulk resend.
   vxn-1 had two such non-param items (`EV_KEY_MODE` 7 / `EV_SPLIT_POINT`
   8); vxn-2 had none and reserved the tags. VXN1b reclaims 7/8 and adds
   more: LFO2 link, matrix edit, layer copy, scope tap.

3. **181 params, two-layer map.** `2 * PATCH_COUNT + GLOBAL_COUNT` — a
   flat CLAP surface over the layer-indexed table ([[vxn1b-two-layer-param-map]]).
   Larger store than vxn-1's 165 and vxn-2's 209-flat; the 16-bit codec
   index field is fine. The JS side needs the layer split back
   (vxn-2 dropped `patchClapId`/`globalClapId`; vxn-1's version is the
   one to port).

4. **MPE.** `vxn1b-clap`'s note dispatch is deliberately bespoke and
   channel-aware (per-note pressure). The forked `midi-input.mjs` is
   channel-agnostic in both prior ports — the MPE path is new wiring.

5. **No host transport.** `sync.rs` resolves LFO/delay subdivisions
   against host BPM; the browser has no host. Ship a UI BPM control
   seeded at `DEFAULT_TEMPO_BPM`, sent as a ring event.

6. **Head start: the standalone page already exists.**
   `vxn1b_ui_web::build_web_faceplate_html()` + the `gen-web-page` bin
   were built for the layout probe ([[vxn-faceplate-layout-probe]]). The
   xtask web target consumes them as-is.

7. **Heavier DSP than either prior port.** 32 voices (2×16), global FX
   chain, dynamics, oversampling. Worst-case wasm perf is an open
   question, not a known-good; [[E020]] (perf/cross-browser ship) is
   deferred and this epic inherits its unknowns.

## Third-fork problem (read before starting)

vxn-1 and vxn-2 each carry ~14 `.mjs` transport/glue modules. Measured
diff between the two ports:

| module | vxn-1 | vxn-2 | changed lines |
|---|---|---|---|
| `midi-input.mjs` | 299 | 299 | **0** |
| `keyboard-input.mjs` | 236 | 236 | **0** |
| `preset-persistence.mjs` | 148 | 148 | 14 |
| `state-autosave.mjs` | 160 | 160 | 22 |
| `patch-io.mjs` | 210 | 210 | 36 |
| `preset-storage.mjs` | 152 | 148 | 48 |
| `host-runner.mjs` | 128 | 106 | 50 |
| `audio-host.mjs` | 138 | 120 | 90 |
| `event-codec.mjs` | 202 | 207 | 147 |
| `param-store.mjs` | 324 | 196 | 264 |
| `event-ring.mjs` | 312 | 240 | 280 |
| `coordinator.mjs` | 569 | 424 | 331 |
| `controller.mjs` | 581 | 549 | 632 |
| `faceplate-bridge.mjs` | 876 | 824 | 1296 |

A third verbatim fork of the top rows is not defensible — VXN1b is the
product that already proved the shared-crate direction (`vxn-core-app`,
`vxn-core-ui-web`, `vxn-core-clap`, shared `vxn-dsp`). **0284 extracts
the six near-identical modules first**, parameterising the deltas
(IndexedDB name, product string, param-space shape). The genuinely
divergent bottom rows stay forked — they encode per-synth model shape,
and forcing them into one file would cost more than it saves.

## Planned tickets

Chain: **0284 → 0286 → 0287 → 0288 → 0289 → 0290 → 0291 → 0292 → 0293 → 0294**
(0285 is the unrelated param-mirror fix 0284 turned up; 0295 and 0296 are two more
product bugs this epic surfaced in the shipped ports).

**Numbering note.** The ids below were reconciled on 2026-08-25 against what was
actually built. The transport JS, telemetry and worklet landed before the
controller cdylib rather than after it, so 0287-0289 carry those and the
controller took 0290 — which is also the order the dependencies actually run in:
the worklet half needs nothing from the controller, and the faceplate rewire
needs everything from it.

- [x] **0284** — `crates/vxn-core-web`: extract the six shared JS modules from
      the vxn-1 / vxn-2 forks; parameterise the deltas; repoint both existing
      ports. The "don't fork a third time" ticket, and the only one that touches
      shipped products. **Closed.**
- [x] **0286** — `vxn1b-wasm` engine cdylib: C-ABI host render loop + event codec
      over `vxn1b_engine::Engine`. **Closed.**
- [x] **0287** — SAB transport JS: `event-ring` / `param-store` / `event-codec` +
      `WIRE-FORMAT.md`, byte-identical to 0286's Rust def, two-layer param space.
      **Closed.**
- [x] **0288** — Telemetry return channel: worklet→main SAB for `MeterFrame` and
      `ScopeFrame` under a seqlock, published on a rate division. The first
      audio→view path in any VXN web port. **Closed.**
- [x] **0289** — AudioWorklet + coordinator bootstrap: processor, host-runner,
      audio-host, AudioContext lifecycle. **Closed.**
- [ ] **0290** — `vxn1b-web-controller` cdylib: `Controller<SharedParams>` +
      `WebPresetStore` over the C-ABI opcode surface (port `vxn2-web-controller`,
      including its `user_store.rs`). Reuses `vxn1b-engine`'s TOML preset codec
      and `EnginePresetStore` shape — web and desktop must not drift on the
      preset record format. Owns the descriptor taper and display strings that
      `param-store.mjs`'s `paramChanged()` currently stubs.
- [ ] **0291** — Faceplate rewire: `controller.mjs` + `faceplate-bridge.mjs`
      replacing wry `window.ipc` / `evaluate_script`; route VXN1b's custom
      opcodes to the right side — topology to the ring, presets to the
      controller. `build_web_faceplate_html()` + `gen-web-page` already exist;
      check the vxn-2 `routeOpcode` quirk (numeric `op` field collision) doesn't
      recur in VXN1b's dispatch. Also closes 0289's deliberate gap: a render trap
      rebuilds the engine, and the controller is what re-broadcasts the
      non-automatable state.
- [ ] **0292** — `vxn1b-xtask web` pipeline: build both wasms (release +
      `simd128`), `bake-factory` bin → `factory.bin`, `gen-web-page` →
      `index.html`, assemble `target/web-dist/`, emit COOP/COEP `_headers`,
      `serve-coep.mjs` dev server. **Rebase on the in-flight uncommitted xtask
      work** (0213).
- [ ] **0293** — Browser persistence: IndexedDB user presets (`vxn1b-presets`),
      state autosave, patch export/import/URL-share — over 0284's shared modules,
      not a fresh fork.
- [ ] **0294** — Input adapters + ship: Web MIDI (**incl. MPE channel/pressure**,
      delta 4) + computer keyboard → ring producers; BPM control (delta 5);
      `deploy-web.sh` that does **not** clobber the vxn-1/vxn-2 `_headers` blocks
      ([[vxn-web-publish-flow]]); hosting doc; DAW-free browser smoke.

## Risks

- **Telemetry channel is unproven.** 0289 has no prior art in this repo.
  A naive `postMessage` per frame at 60 Hz will work but adds GC churn on
  the audio thread's port; a SAB seqlock is the right shape and is new
  code. Budget accordingly.
- **0284 touches two shipped products.** Extracting shared JS out from
  under vxn-1 and vxn-2 can regress live ports. Gate on both node suites
  (vxn-1 web, vxn-2 web 89) staying green, and land it alone.
- **Perf.** 32 voices + oversampled ladder + full FX in wasm, single
  thread, no NEON. Measure before promising; [[E020]] is deferred for a
  reason. Safari's one-quantum AudioWorklet buffer is a known platform
  limit ([[vxn1-web-safari-audioworklet]]) — expect the same glitching,
  and reuse vxn-1's meter-off-on-Safari mitigation.
- **Codec drift.** One Rust def, one JS def, byte-identical. The golden
  table in `WIRE-FORMAT.md` is the contract; both prior ports hit this.
- **Matrix/param coherence.** Depths are CLAP params (store SAB);
  topology is a ring event. A preset load changes both at once — they
  must land in the same block or a slot briefly routes the old source at
  the new depth.
- **Uncommitted xtask work.** `vxn-1b/xtask/src/main.rs` +48/-… and
  `wrapper/` are dirty on `main` right now (0213). 0292 must rebase, not
  race.

## Out of scope

- Changes to `vxn-dsp` / `vxn1b-engine` core (spike proved none needed).
- The native CLAP/VST3 build (`vxn1b-clap` stays as-is, excluded from wasm).
- New DSP features.
- Cross-browser perf hardening beyond a smoke measurement — that's [[E020]]'s
  shape, and wants its own epic once the port runs.

## Acceptance

- `cargo run -p vxn1b-xtask -- web` yields a servable `dist/` in one command.
- Served page reaches "audio live" after a gesture and plays from
  MIDI/keyboard, MPE pressure included.
- Every param, matrix edit, key-mode/split, LFO2 link and layer copy
  round-trips; meters and scope animate from real audio.
- Factory bank loads; user presets persist across reloads.
- `SharedArrayBuffer` available (isolation headers verified); deployed
  next to vxn-1/vxn-2 without breaking their headers.
- No third verbatim fork of `midi-input.mjs` / `keyboard-input.mjs` exists.
