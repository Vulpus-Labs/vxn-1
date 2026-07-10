// Shared SPSC event-ring + worklet drain logic (ticket 0155, epic E030).
//
// The main→worklet transport: a lock-free single-producer / single-consumer
// ring over a SharedArrayBuffer. Ported from vxn-1's `vxn-wasm/web/event-ring`
// (spike 0035) unchanged in mechanism — the framing is byte-identical (see
// event-codec.mjs), only the vxn-1 key-mode / split-point push helpers are
// dropped (the vxn-2 FM engine has no such shared state).
//
// ===========================================================================
// FRAMING
// ===========================================================================
//
// The ring is a fixed-stride 16-byte slot array carved out of a SAB. Fixed
// slots (not byte-packed variable records) mean no record straddles the wrap
// boundary, the write index advances by exactly one slot (a single
// Atomics.store of a monotonic counter), and a 16-byte slot holds every event
// with room to spare.
//
// SharedArrayBuffer layout:
//   [ CTRL: Int32Array, 2 slots ]    <- writeIdx (i32[0]), readIdx (i32[1])
//   [ DATA: SLOT_BYTES * CAPACITY ]  <- the slot array, byte-addressed
//
//   writeIdx / readIdx are MONOTONIC slot counters (never wrapped); the actual
//   slot is (idx & (CAPACITY-1)), CAPACITY a power of two. Monotonic counters
//   make empty (w==r) vs full (w-r==CAPACITY) unambiguous without a wasted slot.
//
// Per-slot record (16 bytes, little-endian) — matches event-codec.mjs:
//   off 0 type | off 1 offset | off 2 u16 paramIdx | off 4 f32 value
//   off 8 note | off 9 flag | off 10 u16 seq (RING-owned) | off 12 reserved
//
// ===========================================================================
// OVERFLOW POLICY:  BLOCK-WRITER (never drop)
// ===========================================================================
// The producer is the main thread (may stall a microsecond); the consumer is
// the realtime worklet (may not). On a full ring the WRITER fails the push
// (returns false) and the caller retries/coalesces, rather than the reader
// dropping musical events (an unpaired note-off / lost gesture-end would corrupt
// the loop). The ring is sized so block never happens in practice.

export const SLOT_BYTES = 16;
export const CTRL_I32 = 2; // writeIdx, readIdx
export const CTRL_BYTES = CTRL_I32 * 4;
export const DEFAULT_CAPACITY = 1024; // slots; must be power of two.
// Matches the Rust host's MAX_EVENTS so a full ring drains in one quantum.

// Event type tags — the subset the ring's push helpers produce. Full tag set +
// codec live in event-codec.mjs; kept here in sync for the byte-level pushers.
export const EV_NOTE_ON = 1;
export const EV_NOTE_OFF = 2;
export const EV_PARAM = 3;
export const EV_PITCH_BEND = 4;
export const EV_MOD_WHEEL = 5;
export const EV_SUSTAIN = 6;

const I_WRITE = 0;
const I_READ = 1;

function isPow2(n) {
  return n > 0 && (n & (n - 1)) === 0;
}

/// Allocate a fresh SAB sized for `capacity` slots. Both threads then construct
/// an EventRing view over it. In the browser the main thread allocates and posts
/// the SAB to the worklet via processorOptions.
export function createRingSAB(capacity = DEFAULT_CAPACITY) {
  if (!isPow2(capacity)) throw new Error("capacity must be a power of two");
  const bytes = CTRL_BYTES + SLOT_BYTES * capacity;
  const Buf = typeof SharedArrayBuffer !== "undefined" ? SharedArrayBuffer : ArrayBuffer;
  return new Buf(bytes);
}

/// SPSC ring view over a SAB. Both producer (main) and consumer (worklet)
/// construct one over the SAME SAB. Lock-free: the only cross-thread state is
/// the two monotonic i32 counters, accessed via Atomics. No Atomics.wait — the
/// consumer free-polls in process().
export class EventRing {
  constructor(sab, capacity = DEFAULT_CAPACITY) {
    if (!isPow2(capacity)) throw new Error("capacity must be a power of two");
    this.capacity = capacity;
    this.mask = capacity - 1;
    this.ctrl = new Int32Array(sab, 0, CTRL_I32);
    this.data = new DataView(sab, CTRL_BYTES);
    // Byte view over the same data region, cached for drainRawInto so the render
    // thread allocates nothing per quantum.
    this.bytes = new Uint8Array(sab, CTRL_BYTES);
    this._seq = 0; // producer-local monotonic counter (for drop detection)
  }

  // ---- producer side (main thread) --------------------------------------

  /// Next producer sequence number push* will stamp (low 16 bits). Tests use it
  /// to predict the expected seq stream.
  peekSeq() {
    return this._seq & 0xffff;
  }

  // Low-level slot writer. BLOCK-WRITER: returns false if the ring is full so
  // the caller decides (retry/coalesce). Acquire-load the reader index, then
  // release-store the writer index AFTER the slot bytes land, so the consumer
  // never observes a half-written slot.
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
    Atomics.store(this.ctrl, I_WRITE, w + 1); // release
    return true;
  }

  pushNoteOn(offset, note, velocity) {
    return this._push(EV_NOTE_ON, offset, 0, velocity, note, 0);
  }
  pushNoteOff(offset, note) {
    return this._push(EV_NOTE_OFF, offset, 0, 0, note, 0);
  }
  /// Plain-value param write (flag 0). Normalised param writes go through the
  /// codec's `encodeInto` (flag = PARAM_FLAG_NORM); this byte-level helper is the
  /// common plain path the input adapters use.
  pushParam(offset, paramIdx, value) {
    return this._push(EV_PARAM, offset, paramIdx, value, 0, 0);
  }
  pushPitchBend(offset, value) {
    return this._push(EV_PITCH_BEND, offset, 0, value, 0, 0);
  }
  pushModWheel(offset, value) {
    return this._push(EV_MOD_WHEEL, offset, 0, value, 0, 0);
  }
  pushSustain(offset, on) {
    return this._push(EV_SUSTAIN, offset, 0, 0, 0, on ? 1 : 0);
  }

  /// Push a pre-built codec event object (e.g. a normalised param, a gesture)
  /// by encoding it straight into the next slot. Returns false if full. Lets the
  /// producer use the full event-codec vocabulary, not just the byte pushers.
  pushEvent(event, encodeInto) {
    const w = Atomics.load(this.ctrl, I_WRITE);
    const r = Atomics.load(this.ctrl, I_READ);
    if (w - r >= this.capacity) return false;
    const base = (w & this.mask) * SLOT_BYTES;
    encodeInto(this.data, base, event);
    // Codec zeroes seq; the ring owns it, so stamp it after the codec write.
    this.data.setUint16(base + 10, this._seq & 0xffff, true);
    this._seq = (this._seq + 1) & 0x7fffffff;
    Atomics.store(this.ctrl, I_WRITE, w + 1);
    return true;
  }

  // ---- consumer side (worklet render thread) ----------------------------

  /// Number of records currently waiting.
  pending() {
    return Atomics.load(this.ctrl, I_WRITE) - Atomics.load(this.ctrl, I_READ);
  }

  /// Drain ALL currently-available records into `out` (reused across calls to
  /// avoid render-thread allocation). Acquire-load the writer index first so we
  /// only read published slots; release-store the reader index after.
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
  /// record COUNT copied. This is the production audio-host path: the ring's
  /// bytes ARE the Rust codec's input, so they copy straight into the wasm decode
  /// scratch (`vxn_host_events_ptr`) with no per-record JS object churn. Caps at
  /// dstU8's record capacity; only records actually copied are reclaimed, so a
  /// too-small destination degrades gracefully rather than dropping events.
  drainRawInto(dstU8) {
    const w = Atomics.load(this.ctrl, I_WRITE); // acquire
    let r = Atomics.load(this.ctrl, I_READ);
    const maxRecs = (dstU8.length / SLOT_BYTES) | 0;
    const src = this.bytes;
    let count = 0;
    while (r !== w && count < maxRecs) {
      const sbase = (r & this.mask) * SLOT_BYTES;
      const dbase = count * SLOT_BYTES;
      // Direct 16-byte copy, not src.subarray → dst.set: subarray allocates a
      // fresh view per event on the audio thread, churning the GC (Safari's JSC
      // stalls the render thread on collection → audible blips).
      for (let k = 0; k < SLOT_BYTES; k++) dstU8[dbase + k] = src[sbase + k];
      r++;
      count++;
    }
    Atomics.store(this.ctrl, I_READ, r); // release: reclaim only what we copied
    return count;
  }
}
