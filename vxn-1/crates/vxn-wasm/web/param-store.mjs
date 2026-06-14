// Cross-thread parameter store + audio->main diff readback (ticket 0039).
//
// The web analogue of the native `SharedParams` (vxn-engine/src/shared.rs):
// one atomic per CLAP id holding the param's PLAIN f32 value, bit-cast into an
// i32 slot. Audio (worklet) reads lock-free in the render loop; the controller
// (main thread) writes on edits / bulk preset load. Latest-value-wins.
//
// ONE code path, imported by BOTH the Node test (param-store.test.mjs) and the
// AudioWorklet wiring, so what we prove headlessly is byte-for-byte what the
// browser runs — same discipline as event-ring.mjs (0035).
//
// Implements ADR 0009 §2 (SAB of 165 atomics) and §3 (the 165-id layout), and
// ports the param-diff pump from vxn-clap/src/lib.rs:193-236.
//
// ===========================================================================
// PARAM ADDRESSING — frozen by ADR 0009 §3 (matches vxn-app/src/params.rs)
// ===========================================================================
//
//   counts:  PATCH_COUNT  = 69   (PatchParam::Osc1Wave .. Spread)
//            GLOBAL_COUNT = 27   (GlobalParam::MasterTune .. ReverbMix)
//            LAYER_COUNT  = 2    (Upper, Lower)
//            TOTAL_PARAMS = 2*69 + 27 = 165
//
//   id ranges:
//     [  0 ..  69 )   Upper layer per-patch params  (clap_id = patch_index)
//     [ 69 .. 138 )   Lower layer per-patch params  (clap_id = 69 + patch_index)
//     [138 .. 165 )   global params                 (clap_id = 138 + global_index)
//
// These JS constants MUST match vxn-app's `PATCH_COUNT` / `GLOBAL_COUNT` /
// `TOTAL_PARAMS`. The Rust side (codec 0037, audio-host 0038) pulls them from
// vxn-app and does NOT hard-code; this JS mirror is the single declared
// constant the layout reconciliation (at integration) checks against. Keep
// LAYOUT exported and named so 0037's codec can assert agreement.
//
// Non-automatable shared state (KeyMode, split point — ADR 0003 §3) is NOT in
// the 165 and never occupies a slot; it travels out-of-band on the ring.

// The id layout is owned by the 0037 codec (event-codec.mjs), itself mirroring
// vxn-app's PATCH_COUNT / GLOBAL_COUNT / TOTAL_PARAMS. Import it here so the
// store and the codec can never drift — one declared constant, reconciled.
import {
  PATCH_COUNT,
  GLOBAL_COUNT,
  LAYER_COUNT,
  TOTAL_PARAMS,
  patchClapId,
  globalClapId,
} from "./event-codec.mjs";
export {
  PATCH_COUNT,
  GLOBAL_COUNT,
  LAYER_COUNT,
  TOTAL_PARAMS,
  patchClapId,
  globalClapId,
};

// A single named layout descriptor with the region bases this store needs.
export const LAYOUT = Object.freeze({
  PATCH_COUNT,
  GLOBAL_COUNT,
  LAYER_COUNT,
  TOTAL_PARAMS,
  UPPER_BASE: 0,
  LOWER_BASE: PATCH_COUNT, // 69
  GLOBAL_BASE: LAYER_COUNT * PATCH_COUNT, // 138
});

// ===========================================================================
// SAB LAYOUT  (two regions, one buffer)
// ===========================================================================
//
// We allocate ONE SharedArrayBuffer carrying two i32 regions, so a host only
// passes one buffer to the worklet (processorOptions):
//
//   region STORE    : Int32Array(165)   main -> audio current-value store
//                                        (controller writes, worklet reads)
//   region READBACK : Int32Array(165)   audio -> main echo of applied values
//                                        (worklet writes, main polls + diffs)
//
//   byte 0                                165*4                       330*4
//   |----------- STORE (165 i32) ---------|------- READBACK (165 i32) -------|
//
// Both regions are i32 atomics; each word holds an f32 PLAIN value bit-cast via
// Atomics.load/store of the bits (mirroring AtomicU32 + f32::to_bits). We read
// the f32 back through an aliasing Float32Array over the SAME buffer (the
// AtomicU32+bitcast trick): Atomics gives the per-slot atomicity + ordering,
// the Float32Array view gives the float interpretation of the published bits.
//
// PER-SLOT ATOMICITY GUARANTEE: every write is a single Atomics.store of one
// 32-bit word; every read is a single Atomics.load of one 32-bit word. A
// concurrent reader therefore always sees a slot as either fully the old value
// or fully the new value — never a torn 32-bit float. This holds for the bulk
// writeBulk() path too: it is 165 independent single-word stores. There is NO
// cross-slot transactionality (a reader mid-bulk-load can see some new + some
// old slots), exactly as the native SharedParams gives (165 independent
// Relaxed AtomicU32 stores). Latest-value-wins per id; that is the contract
// the audio thread is built on.

const STORE_WORDS = TOTAL_PARAMS;
const READBACK_WORDS = TOTAL_PARAMS;
const TOTAL_WORDS = STORE_WORDS + READBACK_WORDS;

const STORE_BASE_WORD = 0;
const READBACK_BASE_WORD = STORE_WORDS;

export const STORE_BYTES = TOTAL_WORDS * 4;

/// Allocate the param SAB (store + readback). In the browser the host
/// allocates and posts it to the worklet via processorOptions; in Node a plain
/// SharedArrayBuffer is constructible without isolation.
export function createParamSAB() {
  const Buf =
    typeof SharedArrayBuffer !== "undefined" ? SharedArrayBuffer : ArrayBuffer;
  return new Buf(STORE_BYTES);
}

// ===========================================================================
// ParamStore — the SharedParams analogue
// ===========================================================================
//
// Both threads construct one of these over the SAME SAB. Lock-free: every
// access is a single Atomics.load/store. No Atomics.wait (forbidden on the
// worklet render thread).
//
// The f32 bit-cast: Atomics only operates on integer typed arrays, so the
// authoritative atomic op is always on `i32`. To turn an f32 into its bits we
// stash it into a 1-element scratch Float32Array that aliases a scratch
// Int32Array, read the int, then Atomics.store that int. The reverse for
// reads. This scratch is per-instance (not shared), so it never races.

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

  /// Bulk write all 165 params (preset load — ADR 0009: ~1us, the case SAB
  /// wins on). 165 independent single-word atomic stores; see the per-slot
  /// atomicity note above for the (intended) lack of cross-slot
  /// transactionality. `values` is a length-165 array/Float32Array of PLAIN
  /// values.
  writeBulk(values) {
    if (values.length !== TOTAL_PARAMS) {
      throw new Error(
        `writeBulk expects ${TOTAL_PARAMS} values, got ${values.length}`,
      );
    }
    for (let id = 0; id < TOTAL_PARAMS; id++) {
      Atomics.store(this.i32, STORE_BASE_WORD + id, this._bitsOf(values[id]));
    }
  }

  /// Snapshot all 165 plain values into a fresh Float32Array (e.g. for a
  /// state save). Lock-free per-slot reads.
  readAll() {
    const out = new Float32Array(TOTAL_PARAMS);
    for (let id = 0; id < TOTAL_PARAMS; id++) out[id] = this.read(id);
    return out;
  }

  // ---- diff readback: audio writes, main reads --------------------------

  /// AUDIO SIDE. Publish the value the worklet actually applied for CLAP id
  /// `id` into the readback region. Called from the render loop after applying
  /// a param (host-automation-style write, modulation echo). Single atomic
  /// word store — never blocks the render thread.
  publishReadback(id, value) {
    Atomics.store(this.i32, READBACK_BASE_WORD + id, this._bitsOf(value));
  }

  /// AUDIO/MAIN SIDE. Read the current readback value for id (lock-free).
  readReadback(id) {
    return this._floatOf(Atomics.load(this.i32, READBACK_BASE_WORD + id));
  }
}

// ===========================================================================
// DIFF-READBACK PUMP — port of vxn-clap push_param_diffs (lib.rs:193-236)
// ===========================================================================
//
// The native pump scans SharedParams against a main-thread `last_seen` mirror
// and emits ViewEvent::ParamChanged for any audio-thread write the controller
// never processed (host automation / modulation echo). NaN-seed semantics
// force a full broadcast on the first tick after open (NaN != NaN, so every
// slot differs against the seed).
//
// Web mapping: the worklet publishes applied values into the READBACK region;
// the main thread polls it on rAF and diffs against `last_seen`. The emitted
// record shape is kept compatible with ViewEvent::ParamChanged
// (vxn-core-app events.rs) so E018's UI bridge can consume it directly:
//
//   ViewEvent::ParamChanged { id, plain, norm, display }
//        |        |     |      |
//        v        v     v      v
//   { id: u32, plain: f32, norm: f32, display: string }
//
// norm/display: the native pump computes norm via the param descriptor taper
// and display via sync_aware_display. Those live in vxn-app and are NOT
// reachable from this headless JS scaffold. The readback SHAPE is what 0039
// must lock down; norm/display are stubbed with a clear contract below. At
// integration (E018) these are filled by the controller wasm, which owns the
// descriptors — see TODO.

/// Create a fresh `last_seen` mirror seeded with NaN, so the FIRST pollDiffs
/// broadcasts all 165 (mirrors the native all-NaN seed vector — lib.rs:206-207
/// "the seeded all-NaN vector forces a full broadcast on the first tick").
export function newLastSeen() {
  const a = new Float32Array(TOTAL_PARAMS);
  a.fill(NaN);
  return a;
}

/// MAIN SIDE. Scan the readback region against `lastSeen`, update it in place,
/// and return the list of changed params as ParamChanged-equivalent records.
///
/// NaN-aware compare, exactly like the native pump: NaN never equals itself, so
/// a freshly-seeded (all-NaN) `lastSeen` forces a full broadcast on the first
/// call. Thereafter only genuine drift surfaces; an unchanged region yields [].
///
/// `lastSeen` MUST be a Float32Array(165) from newLastSeen() (or a prior poll).
/// Returns an array of { id, plain, norm, display }.
export function pollDiffs(store, lastSeen) {
  const out = [];
  for (let id = 0; id < TOTAL_PARAMS; id++) {
    const plain = store.readReadback(id);
    // NaN-aware: `plain === lastSeen[id]` is false when EITHER is NaN, so the
    // all-NaN seed forces every slot to surface on the first poll. A genuine
    // NaN published into the readback would re-emit every poll, but the engine
    // never produces NaN param values (descriptors clamp), matching native.
    if (plain === lastSeen[id]) continue;
    lastSeen[id] = plain;
    out.push(paramChanged(id, plain));
  }
  return out;
}

/// Build a single ParamChanged-equivalent record. Centralised so the shape is
/// declared in one place for E018.
///
/// TODO(E018): `norm` and `display` are derived from the param descriptor
/// (taper) and sync_aware_display, both owned by vxn-app. The controller wasm
/// (ADR 0009 §1) exposes these; until E018 wires it, `norm` is a passthrough of
/// `plain` (NOT correct for Exp-tapered params) and `display` is the plain
/// value stringified. The readback PLUMBING is what 0039 proves; the exact
/// norm/display strings are E018's to fill via the descriptor.
function paramChanged(id, plain) {
  return {
    id, // u32 CLAP id
    plain, // f32 plain value (authoritative — straight from the readback word)
    norm: plain, // TODO(E018): descriptor.to_fader(plain) via controller wasm
    display: String(plain), // TODO(E018): sync_aware_display via controller wasm
  };
}

// ===========================================================================
// WORKLET INTEGRATION SKETCH — the render-loop side (for 0038)
// ===========================================================================
//
// 0038 (worklet audio-host) owns the real render loop; this helper shows the
// param-store half of it and IS the API 0038 calls, so the read/write surface
// is settled here. Not the full audio host (ring drain, block slicing) — just
// the store interaction:
//
//   1. Before/while rendering a quantum, read the current-value store
//      lock-free and push any CHANGED value into the Synth via vxn_set_param.
//   2. Whatever value actually landed in the engine for an id is echoed into
//      the readback region, so the main thread's pollDiffs sees audio-thread
//      drift (host-automation-style writes, modulation echo).
//
// `engine.setParam(id, value)` is the 0035 `vxn_set_param(handle, id, value)`
// shim — REUSED, no new wasm export needed. `workletSeen` is the worklet-local
// mirror of what it last applied (avoids re-applying an unchanged value every
// quantum — the SAB is latest-value-wins, not an event stream).

/// Fresh worklet-side mirror. Seeded NaN so the first render applies all 165
/// (matches the controller seeding the store before the worklet starts).
export function newWorkletSeen() {
  const a = new Float32Array(TOTAL_PARAMS);
  a.fill(NaN);
  return a;
}

/// WORKLET SIDE. Drain the current-value store into the engine: for every id
/// whose store value differs from what we last applied, apply it via
/// vxn_set_param and echo it into the readback region. Returns the count
/// applied (instrumentation). Lock-free throughout; no allocation in steady
/// state.
export function applyStoreToEngine(store, engine, workletSeen) {
  let applied = 0;
  for (let id = 0; id < TOTAL_PARAMS; id++) {
    const v = store.read(id);
    if (v === workletSeen[id]) continue; // unchanged (NaN seed forces first apply)
    workletSeen[id] = v;
    engine.setParam(id, v); // == vxn_set_param(handle, id, v)
    store.publishReadback(id, v); // echo so main-thread pollDiffs observes it
    applied++;
  }
  return applied;
}
