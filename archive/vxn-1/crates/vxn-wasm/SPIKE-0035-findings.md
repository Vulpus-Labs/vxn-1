# SPIKE 0035 — SAB event-ring transport + worklet block-slicing

**Ticket:** [0035](../../../tickets/open/0035-web-sab-event-ring-spike.md) ·
**Epic:** [E015](../../../epics/open/E015-web-event-driven-core.md) ·
**Builds on:** spike [0034](../../README.md) (engine renders in a worklet) ·
**Verdict: GO.** The lock-free SAB ring delivers events to the worklet and the
CLAP block-slicing loop ports to JS with **exact** (0-frame) sample accuracy.

This is a throwaway de-risk spike. It is not shippable code; it exists to land
the decisions 0037 (codec) and 0038 (audio-host) inherit, and to prove the
mechanism headlessly before any UI exists.

## What was built (all in `crates/vxn-wasm/`, engine untouched)

| File | Role |
|------|------|
| `src/lib.rs` | **+2 exports** (existing ones kept): `vxn_process_slice(ptr,start,end)` renders a sub-range of the quantum; `vxn_set_param(ptr,idx,value)` applies a CLAP param mid-block. Both are thin wrappers over the unchanged `Synth` — the host owns slicing, exactly as the CLAP shell does. |
| `web/event-ring.mjs` | **The shared core.** SPSC ring over a `SharedArrayBuffer` (lock-free `Atomics`, no `Atomics.wait`), the binary record framing, and the CLAP-parity slice loop (`renderQuantumSliced`) plus the `renderQuantumBlockStart` comparison path. **Imported verbatim by both the worklet and the Node harness** — one code path. |
| `web/vxn-processor-0035.js` | `AudioWorkletProcessor` that imports the shared module: each `process()` drains the ring lock-free, then slices the 128-frame block at event offsets. |
| `web/index-0035.html` | Browser bootstrap: allocates the SAB, posts it to the worklet, reports `crossOriginIsolated` / SAB constructibility, drives note-ons into the ring. |
| `serve-coep.mjs` | Tiny static server setting the COOP/COEP (+CORP) headers that enable `SharedArrayBuffer`. |
| `harness-0035.mjs` | Node harness — the headless proof. Drives the wasm through the **same** ring + slice code the worklet uses; asserts onset accuracy, quantifies jitter vs block-start, stress-tests the overflow policy. |

Run: `cargo build -p vxn-wasm --target wasm32-unknown-unknown --release`,
copy the `.wasm` to `web/`, then `node crates/vxn-wasm/harness-0035.mjs`
(green) and `node crates/vxn-wasm/serve-coep.mjs` for the browser path.

---

## Decisions (frozen for 0037 / 0038)

### Record framing — fixed 16-byte slots, monotonic-counter SPSC ring

- **SAB layout:** `[ CTRL: Int32Array(2) = {writeIdx, readIdx} ][ DATA: 16 × CAPACITY bytes ]`.
- **Indices are monotonic slot counters** (never wrapped); the slot is
  `idx & (CAPACITY-1)`, so `CAPACITY` is a power of two. Monotonic counters make
  *empty* (`w==r`) vs *full* (`w-r==CAPACITY`) unambiguous with no wasted slot.
  They are `i32`; wrap at 2³¹ slots is ~years away — flagged for 0038, not a
  spike concern.
- **Per-slot record (16 B, little-endian):** `u8 type · u8 offset(0..127) ·
  u16 paramIdx · f32 value · u8 note · u8 flag · u16 seq · f32 reserved`.
- **Max record size = 16 B = slot stride.** Chose **fixed slots over byte-packed
  variable-length records** deliberately: no record ever straddles the ring wrap,
  so the reader never stitches a header across two ranges (the classic ring bug),
  and the write index advances by exactly one slot (a single `Atomics.store`).
  Cost is internal fragmentation (a 6-byte note-on burns 16 B) — free at our
  volumes.
- **Ring capacity = 1024 slots = 16 KiB data** (+8 B ctrl). ≈ 8 quanta of
  headroom even at a pathological 128 events/quantum; ≈ 340 ms of buffering.
- **`seq` field**: low 16 bits of a producer-monotonic counter, so the consumer
  can assert no event was dropped or reordered (used by the harness).
- **Event types** cover the E015 set (note on/off, param, pitch-bend, mod-wheel,
  sustain, key-mode, split-point); the spike exercises note + param. The codec
  proper is 0037 — this framing is what it inherits.

### Slicing fidelity — **full per-event slicing** (not apply-at-block-start)

Justified by measured jitter (numbers below). The sliced path is the direct
port of the CLAP batch loop (`vxn-clap/src/lib.rs:335-369`): apply all events at
offset *k*, render `[prev..k)`, repeat, render the tail. CPU cost is one extra
`process()` call per *distinct* event offset per quantum — negligible at the
density measured (54× realtime with 8 sliced events/quantum, one held voice).
Sample-accuracy is structural, not approximate; timing parity with the plugin
is preserved. **No reason to settle for block-start.**

### Ring-overflow policy — **block-writer (never drop)**

On a full ring the **producer** push fails (returns `false`) and the caller
retries/coalesces. The producer is the main thread (may stall a µs); the
consumer is the realtime worklet (must not). **drop-oldest is rejected**: it
would corrupt the slice loop — an unpaired note-off, a lost gesture-end, a
half-applied preset. The ring is sized so blocking should never occur; if it
does, the audio thread has likely died and dropping events would only mask that.

### Isolation — **GO.** COOP/COEP enables SAB; headers reproduced.

`serve-coep.mjs` sets, on **every** response (document, `.wasm`, `.mjs`):

```
Cross-Origin-Opener-Policy:   same-origin
Cross-Origin-Embedder-Policy: require-corp
Cross-Origin-Resource-Policy: same-origin     # so same-origin subresources qualify
```

Verified by `curl` against the running server (200s with all three headers and
correct MIME — `application/wasm`, `text/javascript`). With both COOP+COEP on
the top-level document the browser sets `self.crossOriginIsolated === true` and
`new SharedArrayBuffer()` becomes constructible; `index-0035.html` displays both
flags live. Everything here is same-origin so `require-corp` "just works"; a
real deploy (E016) must add CORP/CORS to any third-party subresource.

**Fallback budget if isolation is ever unavailable** (e.g. a host that can't set
headers): fall back to `postMessage`, which degrades to the *block-start* path —
the harness measured its cost at **up to 127 frames ≈ 2.65 ms of onset jitter**
at 48 kHz, plus `postMessage` queue latency on top. That is musically audible
for tight timing but tolerable for a coarse fallback. The headers are trivial to
set on any first-party host, so this is a true last resort, not the plan.

---

## Measured numbers (Node, 48 kHz, 128-frame quantum)

Onset = first strictly-non-zero output frame (the engine's silent-skip drives
un-noted output to exact 0.0, so this is threshold-free). The amp envelope
starts at 0.0 on attack, giving a constant **1-frame** engine latency; a
sample-accurate path reproduces `onset = appliedOffset + 1` for every offset.

| Path | Onset error vs intended offset |
|------|--------------------------------|
| **Sliced (per-event)** | **0 frames at every offset 0…127** — exact sample accuracy |
| Block-start (postMessage-equiv) | 0 at offset 0, rising to **127 frames (2645.8 µs) at offset 127**; avg 45.4 frames (946.8 µs) |

- Sliced max error: **0 frames (0.0 µs)**.
- Block-start max error: **127 frames (2645.8 µs)**; worst-case quantum jitter ≈
  one full quantum (`Q-1` frames).
- Stress: dense stream, 24 events/quantum × 2000 quanta = **48 000 events drained
  with 0 dropped, 0 reordered** (seq + offset-order continuity verified).
- Forced overflow: writer accepts exactly `CAPACITY` then refuses the next
  `CAPACITY` (block-writer, no silent drop); after the consumer drains, the
  writer's retry succeeds — nothing lost.
- Throughput: 5 s of audio with 8 sliced events/quantum + a held voice rendered
  in ~0.09 s wall ⇒ **~54× realtime** (single voice, Node; SIMD/full-poly perf
  on real browsers is E020).

---

## Acceptance-criteria → evidence

- [x] **SAB SPSC ring delivers binary records, drained every quantum, no
  `Atomics.wait` on the render thread.** `event-ring.mjs` `EventRing` —
  `_push`/`drainInto` use only `Atomics.load`/`Atomics.store`; the worklet
  `process()` free-polls via `drainInto`. Harness §0/§3 prove delivery + drain.
- [x] **Worklet slices at event offsets; note-on at sub-block offset N takes
  effect at N (parity with CLAP).** `renderQuantumSliced` (shared by worklet +
  harness). Harness §1: 0-frame onset error at offsets 0,1,7,16,31,63,64,100,127.
- [x] **`crossOriginIsolated === true`; documented COOP/COEP headers reproduce
  it.** `serve-coep.mjs` headers verified by curl (above); `index-0035.html`
  reports the flag in-browser. Headers documented in this file.
- [x] **Jitter measured vs the 0034 postMessage path; overflow policy chosen.**
  Sliced 0 µs vs block-start up to 2645.8 µs (block-start === the postMessage
  degradation). Policy: block-writer, proven in harness §3b.
- [x] **Short writeup feeding 0037/0038.** This document.

## Notes / caveats

- Headless Node is the proof surface (no audio device here), exactly as 0034's
  `harness.mjs` proved the render path. Browser audibility on a real device is a
  manual check via `index-0035.html` + `serve-coep.mjs` (out of headless scope).
- The harness exercises the ring end-to-end (write → `Atomics` → drain → slice),
  but single-threaded (one event loop). True concurrent producer/consumer memory
  ordering across a real worklet thread is the same `Atomics` protocol; the
  release-store-after-write / acquire-load-before-read discipline is in place and
  is the standard SPSC correctness argument. Worth a cross-thread soak in 0038.
- `i32` index wrap and render-thread trap safety are explicitly deferred to 0038
  (audio-host) / 0040 (lifecycle harden).
