// Lock-free SPSC event ring — main thread produces, worklet drains (0287).
//
// The framing is the one every VXN synth shares (spike 0035); the producer
// surface below is VXN1b's own event set (MIDI channel on notes, matrix
// topology, scope tap, tempo, per-note pressure). See WIRE-FORMAT.md.
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
// Do NOT add a JS block-slicing loop here. Slicing lives in Rust (`host.rs`),
// reached via `drainRawInto` — one implementation, not two that can disagree
// about what "apply at offset k" means.

import { SLOT_BYTES, encodeInto, ev } from "./event-codec.mjs";

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

  /// Publish one built event (`ev.*` from the codec) into the next slot.
  /// BLOCK-WRITER: returns false if the ring is full so the caller decides.
  /// Acquire-load the reader index; release-store the writer index AFTER the
  /// slot bytes land, so the consumer never observes a half-written slot.
  ///
  /// The slot bytes come from `encodeInto` — the wire's one encoder, and the one
  /// the golden table checks (0312). The ring owns exactly two things the codec
  /// does not: which slot, and the `seq` stamp written over the zero the codec
  /// leaves at off 10. Encoding happens before the write index advances, so an
  /// unknown tag throws without publishing anything.
  ///
  /// The event object is allocated per push. This runs on the MAIN thread at
  /// gesture rate — never in the worklet — so the churn is nominal, and one
  /// named encoder is worth more than avoiding it.
  _push(event) {
    const w = Atomics.load(this.ctrl, I_WRITE);
    const r = Atomics.load(this.ctrl, I_READ);
    if (w - r >= this.capacity) return false; // full -> block-writer
    const base = (w & this.mask) * SLOT_BYTES;
    encodeInto(this.data, base, event);
    this.data.setUint16(base + 10, this._seq & 0xffff, true);
    this._seq = (this._seq + 1) & 0x7fffffff;
    // Release: publish the write index only after the slot is fully written.
    Atomics.store(this.ctrl, I_WRITE, w + 1);
    return true;
  }

  // The producer surface is `ev.*` plus the ring bookkeeping, one for one, so
  // the argument lists match the builders' and `WebHost`'s: the event's own
  // fields, then `offset`, then `channel`. Notes carry the MIDI channel in
  // `flag` — VXN1b is MPE-aware, and a channel-agnostic producer omits it.
  pushNoteOn(note, velocity, offset = 0, channel = 0) {
    return this._push(ev.noteOn(note, velocity, offset, channel));
  }
  pushNoteOff(note, offset = 0, channel = 0) {
    return this._push(ev.noteOff(note, offset, channel));
  }
  pushPolyPressure(note, value, offset = 0, channel = 0) {
    return this._push(ev.polyPressure(note, value, offset, channel));
  }
  pushChannelPressure(value, offset = 0, channel = 0) {
    return this._push(ev.channelPressure(value, offset, channel));
  }

  pushParam(paramIdx, plain, offset = 0) {
    return this._push(ev.setParam(paramIdx, plain, offset));
  }
  pushParamNorm(paramIdx, norm, offset = 0) {
    return this._push(ev.setParamNorm(paramIdx, norm, offset));
  }
  pushGestureBegin(paramIdx, offset = 0) {
    return this._push(ev.gestureBegin(paramIdx, offset));
  }
  pushGestureEnd(paramIdx, offset = 0) {
    return this._push(ev.gestureEnd(paramIdx, offset));
  }

  pushPitchBend(value, offset = 0) {
    return this._push(ev.pitchBend(value, offset));
  }
  pushModWheel(value, offset = 0) {
    return this._push(ev.modWheel(value, offset));
  }

  // Non-automatable domain state (key mode, split point, LFO 2 link) — not
  // params, so they never occupy a store slot; they travel here.
  pushKeyMode(mode, offset = 0) {
    return this._push(ev.keyMode(mode, offset));
  }
  pushSplitPoint(note, offset = 0) {
    return this._push(ev.splitPoint(note, offset));
  }
  pushLfo2Link(on, offset = 0) {
    return this._push(ev.lfo2Link(on, offset));
  }

  /// One matrix slot's topology field. Slot DEPTH is a CLAP param and goes
  /// through pushParam / the store instead — that split is the point of 0219.
  pushMatrixEdit(layer, slot, field, value, offset = 0) {
    return this._push(ev.matrixEdit(layer, slot, field, value, offset));
  }
  pushScopeTap(tap, offset = 0) {
    return this._push(ev.scopeTap(tap, offset));
  }
  pushTempo(bpm, offset = 0) {
    return this._push(ev.tempo(bpm, offset));
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
      out.push(readSlot(d, (r & this.mask) * SLOT_BYTES));
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

/// The raw FIELD view of a 16-byte slot: the six wire fields plus the ring's
/// `seq`, verbatim, with no interpretation of the tag.
///
/// Deliberately not a decoder — the wire has exactly one of those and it is in
/// Rust (`src/codec.rs`). Nothing here switches on `type`, so there is no
/// per-event semantics to drift; it is the inverse of the ring's framing, not of
/// the codec's. `drainInto` and the ring's own tests are its only callers.
///
/// Accepts a DataView or any typed-array/buffer view over the slot bytes.
export function readSlot(view, base = 0) {
  const d =
    view instanceof DataView
      ? view
      : new DataView(view.buffer, view.byteOffset, view.byteLength);
  return {
    type: d.getUint8(base + 0),
    offset: d.getUint8(base + 1),
    paramIdx: d.getUint16(base + 2, true),
    value: d.getFloat32(base + 4, true),
    note: d.getUint8(base + 8),
    flag: d.getUint8(base + 9),
    seq: d.getUint16(base + 10, true),
  };
}
