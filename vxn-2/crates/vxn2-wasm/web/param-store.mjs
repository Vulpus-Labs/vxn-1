// Cross-thread parameter store + audio→main diff readback (ticket 0155).
//
// The web analogue of the native `SharedParams` (vxn2-engine/src/shared.rs):
// one atomic per CLAP id holding the param's PLAIN f32 value, bit-cast into an
// i32 slot. Audio (worklet) reads lock-free block-start; the controller (main
// thread) writes on edits / bulk preset load. Latest-value-wins.
//
// Ported from vxn-1's `vxn-wasm/web/param-store`; the only vxn-2 change is the
// param-space SIZE. vxn-2's id space is FLAT (CLAP id == param index,
// `0 .. TOTAL_PARAMS`) — no Upper/Lower layer split, so the vxn-1 PATCH_COUNT /
// GLOBAL_COUNT / patchClapId / globalClapId layout is gone. TOTAL_PARAMS is
// imported from the codec so the store and codec can never drift; the Rust side
// owns the authoritative count (`vxn2_engine::TOTAL_PARAMS`).

import { TOTAL_PARAMS } from "./event-codec.mjs";
export { TOTAL_PARAMS };

/// Single named layout descriptor (flat id space).
export const LAYOUT = Object.freeze({ TOTAL_PARAMS });

// ===========================================================================
// SAB LAYOUT  (two regions, one buffer)
// ===========================================================================
//
//   region STORE    : Int32Array(TOTAL_PARAMS)  main → audio current-value store
//   region READBACK : Int32Array(TOTAL_PARAMS)  audio → main echo of applied vals
//
// Both regions are i32 atomics; each word holds an f32 PLAIN value bit-cast via
// Atomics.load/store of the bits (mirroring AtomicU32 + f32::to_bits). The f32 is
// read back through the per-instance bit-cast scratch.
//
// PER-SLOT ATOMICITY: every write is a single Atomics.store of one 32-bit word;
// every read a single Atomics.load. A concurrent reader always sees a slot fully
// old or fully new — never torn. No cross-slot transactionality (a reader mid-
// bulk-load can see some new + some old slots), exactly as the native
// `SharedParams` gives (independent Relaxed AtomicU32 stores). Latest-value-wins
// per id.

const STORE_WORDS = TOTAL_PARAMS;
const READBACK_WORDS = TOTAL_PARAMS;
const TOTAL_WORDS = STORE_WORDS + READBACK_WORDS;

const STORE_BASE_WORD = 0;
const READBACK_BASE_WORD = STORE_WORDS;

export const STORE_BYTES = TOTAL_WORDS * 4;

/// Allocate the param SAB (store + readback). In the browser the host allocates
/// and posts it to the worklet via processorOptions; in Node a plain
/// SharedArrayBuffer is constructible without isolation.
export function createParamSAB() {
  const Buf = typeof SharedArrayBuffer !== "undefined" ? SharedArrayBuffer : ArrayBuffer;
  return new Buf(STORE_BYTES);
}

// ===========================================================================
// ParamStore — the SharedParams analogue
// ===========================================================================

export class ParamStore {
  constructor(sab) {
    this.i32 = new Int32Array(sab, 0, TOTAL_WORDS);
    // 1-word scratch for the f32<->i32 bit-cast (instance-local, unshared).
    const scratch = new ArrayBuffer(4);
    this._sf = new Float32Array(scratch);
    this._si = new Int32Array(scratch);
  }

  _bitsOf(value) {
    this._sf[0] = value;
    return this._si[0];
  }
  _floatOf(bits) {
    this._si[0] = bits;
    return this._sf[0];
  }

  // ---- current-value store: main writes, audio reads --------------------

  /// Write the PLAIN f32 value for CLAP id `id`. Single atomic word store.
  write(id, value) {
    Atomics.store(this.i32, STORE_BASE_WORD + id, this._bitsOf(value));
  }

  /// Read the PLAIN f32 value for CLAP id `id`. Single atomic word load.
  read(id) {
    return this._floatOf(Atomics.load(this.i32, STORE_BASE_WORD + id));
  }

  /// Bulk write all TOTAL_PARAMS params (preset load). Independent single-word
  /// atomic stores; see the per-slot atomicity note above for the (intended)
  /// lack of cross-slot transactionality. `values` is a length-TOTAL_PARAMS
  /// array/Float32Array of PLAIN values.
  writeBulk(values) {
    if (values.length !== TOTAL_PARAMS) {
      throw new Error(`writeBulk expects ${TOTAL_PARAMS} values, got ${values.length}`);
    }
    for (let id = 0; id < TOTAL_PARAMS; id++) {
      Atomics.store(this.i32, STORE_BASE_WORD + id, this._bitsOf(values[id]));
    }
  }

  /// Snapshot all plain values into a fresh Float32Array (e.g. for a state
  /// save). Lock-free per-slot reads.
  readAll() {
    const out = new Float32Array(TOTAL_PARAMS);
    for (let id = 0; id < TOTAL_PARAMS; id++) out[id] = this.read(id);
    return out;
  }

  // ---- diff readback: audio writes, main reads --------------------------

  /// AUDIO SIDE. Publish the value the worklet actually applied for CLAP id `id`
  /// into the readback region. Single atomic word store — never blocks render.
  publishReadback(id, value) {
    Atomics.store(this.i32, READBACK_BASE_WORD + id, this._bitsOf(value));
  }

  /// AUDIO/MAIN SIDE. Read the current readback value for id (lock-free).
  readReadback(id) {
    return this._floatOf(Atomics.load(this.i32, READBACK_BASE_WORD + id));
  }
}

// ===========================================================================
// DIFF-READBACK PUMP — port of vxn2-clap's param-diff drain
// ===========================================================================
//
// The worklet publishes applied values into the READBACK region; the main
// thread polls it on rAF and diffs against `lastSeen`. The emitted record shape
// is kept compatible with vxn-2's `ViewEvent::ParamChanged` so the controller
// bridge (ticket 0157) can consume it. NaN-seed semantics force a full broadcast
// on the first tick (NaN != NaN, so every slot differs against the seed).

/// Fresh `lastSeen` mirror seeded NaN, so the FIRST pollDiffs broadcasts all
/// params (mirrors the native all-NaN seed).
export function newLastSeen() {
  const a = new Float32Array(TOTAL_PARAMS);
  a.fill(NaN);
  return a;
}

/// MAIN SIDE. Scan the readback region against `lastSeen`, update it in place,
/// and return the changed params as ParamChanged-equivalent records. NaN-aware
/// compare, so a freshly-seeded (all-NaN) `lastSeen` forces a full broadcast on
/// the first call; thereafter only genuine drift surfaces.
///
/// `norm` / `display` are owned by the controller wasm's param descriptors
/// (ticket 0157) — this headless pump passes `plain` through for `norm` and
/// stringifies `plain` for `display`; the bridge fills the descriptor-correct
/// values. The readback PLUMBING is what this ticket locks down.
export function pollDiffs(store, lastSeen) {
  const out = [];
  for (let id = 0; id < TOTAL_PARAMS; id++) {
    const plain = store.readReadback(id);
    if (plain === lastSeen[id]) continue; // NaN-aware: seed forces first surface
    lastSeen[id] = plain;
    out.push({ id, plain, norm: plain, display: String(plain) });
  }
  return out;
}

// ===========================================================================
// WORKLET INTEGRATION — the block-start param fold (for the coordinator, 0156)
// ===========================================================================
//
// vxn-2 has no per-id engine setter; the store folds into the engine via
// `vxn_host_set_param(id, plain)` block-start, and the wasm host then folds the
// whole engine param set with `Engine::snapshot_params` inside `vxn_host_render`.
// `workletSeen` is the worklet-local mirror of what it last pushed (avoids
// re-pushing an unchanged value every quantum — the SAB is latest-value-wins,
// not an event stream).

/// Fresh worklet-side mirror. Seeded NaN so the first render pushes all params.
export function newWorkletSeen() {
  const a = new Float32Array(TOTAL_PARAMS);
  a.fill(NaN);
  return a;
}

/// WORKLET SIDE. Push changed store values into the wasm host: for every id
/// whose store value differs from what we last pushed, call
/// `host.setParam(id, v)` (== `vxn_host_set_param`) and echo it into the readback
/// region. Returns the count pushed. Lock-free; no allocation in steady state.
export function applyStoreToHost(store, host, workletSeen) {
  let applied = 0;
  for (let id = 0; id < TOTAL_PARAMS; id++) {
    const v = store.read(id);
    if (v === workletSeen[id]) continue; // unchanged (NaN seed forces first push)
    workletSeen[id] = v;
    host.setParam(id, v); // == vxn_host_set_param(handle, id, v)
    store.publishReadback(id, v); // echo so main-thread pollDiffs observes it
    applied++;
  }
  return applied;
}
