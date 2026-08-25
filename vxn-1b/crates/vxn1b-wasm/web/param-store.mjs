// Cross-thread parameter store (0287, trimmed by 0297).
//
// The web analogue of the native `SharedParams`: one atomic per CLAP id holding
// the param's PLAIN f32 value, bit-cast into an i32 slot. Audio (worklet) reads
// lock-free in the render loop; the controller (main thread) writes on edits and
// bulk preset loads. Latest-value-wins.
//
// ONE direction only. vxn-1's store carries a second `readback` region so the
// main thread can see params the AUDIO thread changed — which for a plugin means
// CLAP host automation writing `SharedParams` from `process()`. There is no host
// in a browser: every value here originates in the controller's model, and the
// worklet would only ever echo back what it had just read. The region and its
// diff pump were carried over from vxn-1 and removed once that was traced.
//
// The id layout is owned by `event-codec.mjs`, itself a declared mirror of
// vxn1b-engine's params.rs. Imported here so the store and the codec can never
// disagree about how many slots exist — see WIRE-FORMAT.md, and ticket 0285 for
// what happens when that mirror is allowed to rot.

import {
  PATCH_COUNT,
  GLOBAL_COUNT,
  LAYER_COUNT,
  TOTAL_PARAMS,
  patchClapId,
  globalClapId,
} from "./event-codec.mjs";

export { PATCH_COUNT, GLOBAL_COUNT, LAYER_COUNT, TOTAL_PARAMS, patchClapId, globalClapId };

/// A single named layout descriptor with the region bases this store needs.
/// VXN1b is two-layer like vxn-1 (vxn-2 flattened its space and has no
/// equivalent), so the patch block appears twice before the globals.
export const LAYOUT = Object.freeze({
  PATCH_COUNT,
  GLOBAL_COUNT,
  LAYER_COUNT,
  TOTAL_PARAMS,
  L1_BASE: 0,
  L2_BASE: PATCH_COUNT, // 75
  GLOBAL_BASE: LAYER_COUNT * PATCH_COUNT, // 150
});

// ===========================================================================
// SAB LAYOUT
// ===========================================================================
//
//   Int32Array(TOTAL_PARAMS)   main -> audio current values
//
// Each word holds an f32 PLAIN value bit-cast via Atomics.load/store of the bits
// (mirroring AtomicU32 + f32::to_bits).
//
// PER-SLOT ATOMICITY: every write is a single Atomics.store of one 32-bit word,
// every read a single Atomics.load. A concurrent reader always sees a slot as
// either fully the old or fully the new value — never a torn float. This holds
// for writeBulk too: it is TOTAL_PARAMS independent single-word stores. There is
// deliberately NO cross-slot transactionality — a reader mid-bulk-load can see
// some new and some old slots, exactly as the native SharedParams gives.
// Latest-value-wins per id is the contract the audio thread is built on.

const TOTAL_WORDS = TOTAL_PARAMS;
const STORE_BASE_WORD = 0;

export const STORE_BYTES = TOTAL_WORDS * 4;

/// Allocate the param SAB. In the browser the host allocates and posts it to the
/// worklet via processorOptions; in Node a plain SharedArrayBuffer is
/// constructible without isolation.
export function createParamSAB() {
  const Buf = typeof SharedArrayBuffer !== "undefined" ? SharedArrayBuffer : ArrayBuffer;
  return new Buf(STORE_BYTES);
}

// ===========================================================================
// ParamStore — the SharedParams analogue
// ===========================================================================
//
// Both threads construct one over the SAME SAB. Lock-free: every access is a
// single Atomics.load/store. No Atomics.wait (forbidden on the render thread).
//
// The f32 bit-cast: Atomics only operates on integer typed arrays, so the
// authoritative atomic op is always on i32. An f32 is stashed into a 1-element
// scratch Float32Array aliasing a scratch Int32Array, the int read out, and that
// int stored atomically; the reverse for reads. The scratch is per-instance and
// unshared, so it never races.

export class ParamStore {
  constructor(sab) {
    this.i32 = new Int32Array(sab, 0, TOTAL_WORDS);
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

  /// Bulk write every param (preset load). TOTAL_PARAMS independent single-word
  /// atomic stores; see the per-slot atomicity note above for the intended lack
  /// of cross-slot transactionality. `values` is a length-TOTAL_PARAMS
  /// array/Float32Array of PLAIN values.
  writeBulk(values) {
    if (values.length !== TOTAL_PARAMS) {
      throw new Error(`writeBulk expects ${TOTAL_PARAMS} values, got ${values.length}`);
    }
    for (let id = 0; id < TOTAL_PARAMS; id++) {
      Atomics.store(this.i32, STORE_BASE_WORD + id, this._bitsOf(values[id]));
    }
  }

  /// Snapshot every plain value into a fresh Float32Array (e.g. for a state
  /// save). Lock-free per-slot reads.
  readAll() {
    const out = new Float32Array(TOTAL_PARAMS);
    for (let id = 0; id < TOTAL_PARAMS; id++) out[id] = this.read(id);
    return out;
  }
}

// ===========================================================================
// WORKLET SIDE — the store half of the render loop
// ===========================================================================

/// Fresh worklet-local mirror, NaN-seeded so the first render applies every id
/// (matching the controller seeding the store before the worklet starts).
export function newWorkletSeen() {
  const a = new Float32Array(TOTAL_PARAMS);
  a.fill(NaN);
  return a;
}

/// WORKLET SIDE. Fold the current-value store into the engine: for every id
/// whose store value differs from what was last applied, apply it. Returns the
/// count applied (instrumentation).
///
/// `engine.setParam(id, value)` is the `vxn1b_host_set_param` shim. The mirror
/// avoids re-applying an unchanged value every quantum — the SAB is
/// latest-value-wins, not an event stream. Lock-free throughout; no allocation
/// in steady state.
export function applyStoreToEngine(store, engine, workletSeen) {
  let applied = 0;
  for (let id = 0; id < TOTAL_PARAMS; id++) {
    const v = store.read(id);
    if (v === workletSeen[id]) continue; // unchanged (NaN seed forces first apply)
    workletSeen[id] = v;
    engine.setParam(id, v);
    applied++;
  }
  return applied;
}
