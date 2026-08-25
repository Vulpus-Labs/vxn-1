// Lock-free SPSC event ring — main thread produces, worklet drains (0287).
//
// Ported from vxn-1's ring (spike 0035), whose framing every VXN synth shares.
// The mechanism is unchanged; what differs is the producer surface, which
// carries VXN1b's event set (MIDI channel on notes, matrix topology, scope tap,
// tempo, per-note pressure). See WIRE-FORMAT.md.
//
// The ring is a fixed-stride slot array carved out of a SharedArrayBuffer.
// Fixed slots (not byte-packed variable records) are deliberate:
//   * no record straddles the wrap boundary, so the reader never stitches a
//     header across two ranges — the classic ring bug;
//   * the write index advances by exactly one slot, so the lock-free protocol
//     is a single Atomics.store of a monotonic counter;
//   * 16 bytes holds every event with room to spare.
// The cost is internal fragmentation (a 6-byte note-on still burns 16). At a
// few hundred events per quantum worst case, that is free.
//
// SharedArrayBuffer layout:
//   [ CTRL: Int32Array, 2 slots ]    <- writeIdx (i32[0]), readIdx (i32[1])
//   [ DATA: SLOT_BYTES * CAPACITY ]  <- the slot array, byte-addressed
//
// writeIdx / readIdx are MONOTONIC slot counters (never wrapped); the slot is
// `idx & (CAPACITY-1)` and CAPACITY is a power of two. Monotonic counters make
// empty (w == r) and full (w - r == CAPACITY) unambiguous without burning a
// slot.
//
// OVERFLOW POLICY: BLOCK-WRITER (never drop). The producer is the main thread,
// which may stall a microsecond; the consumer is the realtime worklet, which may
// not. On a full ring the WRITER fails the push (returns false) and the caller
// retries or coalesces, rather than the reader dropping musical events —
// drop-oldest would corrupt the slice loop with an unpaired note-off or a lost
// gesture-end. The ring is sized so this should never happen; if it does, the
// audio thread has died, and dropping events would only mask that.
//
// There is deliberately NO JS block-slicing loop here. vxn-1's ring carries one
// (its spike drove the slice loop from JS); VXN1b's slicing lives in Rust
// (`host.rs`), reached via `drainRawInto` — one implementation, not two that can
// disagree about what "apply at offset k" means.

import {
  SLOT_BYTES,
  EV_NOTE_ON,
  EV_NOTE_OFF,
  EV_PARAM,
  EV_PITCH_BEND,
  EV_MOD_WHEEL,
  EV_KEY_MODE,
  EV_SPLIT_POINT,
  EV_GESTURE_BEGIN,
  EV_GESTURE_END,
  EV_LFO2_LINK,
  EV_MATRIX_EDIT,
  EV_SCOPE_TAP,
  EV_TEMPO,
  EV_POLY_PRESSURE,
  EV_CHANNEL_PRESSURE,
  PARAM_FLAG_NORM,
  packMatrixAddr,
} from "./event-codec.mjs";

export { SLOT_BYTES };
export const CTRL_I32 = 2; // writeIdx, readIdx
export const CTRL_BYTES = CTRL_I32 * 4;
export const DEFAULT_CAPACITY = 1024; // slots; must be a power of two

const I_WRITE = 0;
const I_READ = 1;

function isPow2(n) {
  return n > 0 && (n & (n - 1)) === 0;
}

/// Allocate a fresh SAB sized for `capacity` slots. Both threads then construct
/// an EventRing view over it; in the browser the main thread allocates and posts
/// it to the worklet via processorOptions.
export function createRingSAB(capacity = DEFAULT_CAPACITY) {
  if (!isPow2(capacity)) throw new Error("capacity must be a power of two");
  const bytes = CTRL_BYTES + SLOT_BYTES * capacity;
  // Node without cross-origin isolation still constructs a SharedArrayBuffer;
  // the worklet case needs crossOriginIsolated, proven separately by the headers.
  const Buf = typeof SharedArrayBuffer !== "undefined" ? SharedArrayBuffer : ArrayBuffer;
  return new Buf(bytes);
}

/// SPSC ring view over a SAB. Producer (main) and consumer (worklet) each
/// construct one over the SAME SAB. Lock-free: the only cross-thread state is
/// the two monotonic i32 counters, via Atomics. No Atomics.wait anywhere — the
/// consumer free-polls in process(), because blocking the render thread is not
/// an option.
export class EventRing {
  constructor(sab, capacity = DEFAULT_CAPACITY) {
    if (!isPow2(capacity)) throw new Error("capacity must be a power of two");
    this.capacity = capacity;
    this.mask = capacity - 1;
    this.ctrl = new Int32Array(sab, 0, CTRL_I32);
    this.data = new DataView(sab, CTRL_BYTES);
    // Byte view over the same region, cached so drainRawInto allocates nothing
    // per quantum.
    this.bytes = new Uint8Array(sab, CTRL_BYTES);
    this._seq = 0; // producer-local monotonic counter (drop detection)
  }

  // ---- producer side (main thread) --------------------------------------

  /// The sequence number the next push will stamp. Tests use it to predict the
  /// expected seq stream.
  peekSeq() {
    return this._seq & 0xffff;
  }

  /// Low-level slot writer. BLOCK-WRITER: returns false if the ring is full so
  /// the caller decides. Acquire-load the reader index; release-store the writer
  /// index AFTER the slot bytes land, so the consumer never observes a
  /// half-written slot.
  _push(type, offset, paramIdx, value, note, flag) {
    const w = Atomics.load(this.ctrl, I_WRITE);
    const r = Atomics.load(this.ctrl, I_READ);
    if (w - r >= this.capacity) return false; // full -> block-writer
    const base = (w & this.mask) * SLOT_BYTES;
    const d = this.data;
    d.setUint8(base + 0, type);
    d.setUint8(base + 1, offset & 0xff);
    d.setUint16(base + 2, paramIdx & 0xffff, true);
    d.setFloat32(base + 4, value, true);
    d.setUint8(base + 8, note & 0xff);
    d.setUint8(base + 9, flag & 0xff);
    d.setUint16(base + 10, this._seq & 0xffff, true);
    d.setFloat32(base + 12, 0, true);
    this._seq = (this._seq + 1) & 0x7fffffff;
    // Release: publish the write index only after the slot is fully written.
    Atomics.store(this.ctrl, I_WRITE, w + 1);
    return true;
  }

  // Notes carry the MIDI channel in `flag` — VXN1b is MPE-aware, and a
  // channel-agnostic producer simply passes 0.
  pushNoteOn(offset, note, velocity, channel = 0) {
    return this._push(EV_NOTE_ON, offset, 0, velocity, note, channel);
  }
  pushNoteOff(offset, note, channel = 0) {
    return this._push(EV_NOTE_OFF, offset, 0, 0, note, channel);
  }
  pushPolyPressure(offset, note, value, channel = 0) {
    return this._push(EV_POLY_PRESSURE, offset, 0, value, note, channel);
  }
  pushChannelPressure(offset, value, channel = 0) {
    return this._push(EV_CHANNEL_PRESSURE, offset, 0, value, 0, channel);
  }

  pushParam(offset, paramIdx, plain) {
    return this._push(EV_PARAM, offset, paramIdx, plain, 0, 0);
  }
  pushParamNorm(offset, paramIdx, norm) {
    return this._push(EV_PARAM, offset, paramIdx, norm, 0, PARAM_FLAG_NORM);
  }
  pushGestureBegin(offset, paramIdx) {
    return this._push(EV_GESTURE_BEGIN, offset, paramIdx, 0, 0, 0);
  }
  pushGestureEnd(offset, paramIdx) {
    return this._push(EV_GESTURE_END, offset, paramIdx, 0, 0, 0);
  }

  pushPitchBend(offset, value) {
    return this._push(EV_PITCH_BEND, offset, 0, value, 0, 0);
  }
  pushModWheel(offset, value) {
    return this._push(EV_MOD_WHEEL, offset, 0, value, 0, 0);
  }

  // Non-automatable domain state (key mode, split point, LFO 2 link) — not
  // params, so they never occupy a store slot; they travel here.
  pushKeyMode(offset, mode) {
    return this._push(EV_KEY_MODE, offset, 0, 0, 0, mode);
  }
  pushSplitPoint(offset, note) {
    return this._push(EV_SPLIT_POINT, offset, 0, 0, 0, note);
  }
  pushLfo2Link(offset, on) {
    return this._push(EV_LFO2_LINK, offset, 0, 0, 0, on ? 1 : 0);
  }

  /// One matrix slot's topology field. Slot DEPTH is a CLAP param and goes
  /// through pushParam / the store instead — that split is the point of 0219.
  pushMatrixEdit(offset, layer, slot, field, value) {
    return this._push(EV_MATRIX_EDIT, offset, packMatrixAddr(layer, slot, field), 0, 0, value);
  }
  pushScopeTap(offset, tap) {
    return this._push(EV_SCOPE_TAP, offset, 0, 0, 0, tap);
  }
  pushTempo(offset, bpm) {
    return this._push(EV_TEMPO, offset, 0, bpm, 0, 0);
  }

  // ---- consumer side (worklet render thread) ----------------------------

  /// Records currently waiting.
  pending() {
    return Atomics.load(this.ctrl, I_WRITE) - Atomics.load(this.ctrl, I_READ);
  }

  /// Drain every available record into `out` (reused across calls to avoid
  /// render-thread allocation), as decoded field objects. Acquire-load the
  /// writer index first so only published slots are read; release-store the
  /// reader index after, so the producer can reclaim them.
  ///
  /// This is the DEBUG/test path. Production drains raw bytes straight into wasm
  /// linear memory — see drainRawInto.
  drainInto(out) {
    out.length = 0;
    const w = Atomics.load(this.ctrl, I_WRITE); // acquire
    let r = Atomics.load(this.ctrl, I_READ);
    const d = this.data;
    while (r !== w) {
      const base = (r & this.mask) * SLOT_BYTES;
      out.push({
        type: d.getUint8(base + 0),
        offset: d.getUint8(base + 1),
        paramIdx: d.getUint16(base + 2, true),
        value: d.getFloat32(base + 4, true),
        note: d.getUint8(base + 8),
        flag: d.getUint8(base + 9),
        seq: d.getUint16(base + 10, true),
      });
      r++;
    }
    Atomics.store(this.ctrl, I_READ, w); // release: slots reclaimed
    return out;
  }

  /// Drain raw wire bytes (the 16-byte slots verbatim, arrival order, wrap
  /// handled) into `dstU8` — a byte view over wasm linear memory. Returns the
  /// record COUNT copied. The production path: the ring's bytes ARE the codec's
  /// input, so they go straight into the wasm decode scratch with no per-record
  /// JS object churn.
  ///
  /// Caps at `dstU8`'s record capacity and reclaims ONLY what it copied, so a
  /// too-small destination degrades gracefully instead of dropping events.
  drainRawInto(dstU8) {
    const w = Atomics.load(this.ctrl, I_WRITE); // acquire
    let r = Atomics.load(this.ctrl, I_READ);
    const maxRecs = (dstU8.length / SLOT_BYTES) | 0;
    const src = this.bytes;
    let count = 0;
    while (r !== w && count < maxRecs) {
      const sbase = (r & this.mask) * SLOT_BYTES;
      const dbase = count * SLOT_BYTES;
      // Byte loop, not src.subarray(...) -> dst.set(...): subarray allocates a
      // fresh view per event on the audio thread, churning the GC (Safari's JSC
      // stalls the render thread on collection -> audible blips).
      for (let k = 0; k < SLOT_BYTES; k++) dstU8[dbase + k] = src[sbase + k];
      r++;
      count++;
    }
    Atomics.store(this.ctrl, I_READ, r); // release: reclaim only what we copied
    return count;
  }
}
