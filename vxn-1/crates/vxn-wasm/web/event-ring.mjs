// Shared SPSC event-ring + worklet drain/slice logic (ticket 0035 spike).
//
// ONE code path, imported by BOTH the Node harness (harness-0035.mjs) and the
// production AudioWorklet (vxn-processor.js), so the thing we measure headlessly
// is byte-for-byte the thing the browser runs. This is the de-risk core of E015:
// a lock-free main->worklet transport plus the CLAP block-slicing loop ported
// to JS.
//
// ===========================================================================
// FRAMING  (decided here, frozen for 0037's binary codec)
// ===========================================================================
//
// The ring is a fixed-stride slot array carved out of a SharedArrayBuffer.
// Fixed slots (not byte-packed variable records) were chosen deliberately:
//   * no record ever straddles the wrap boundary, so the reader never has to
//     stitch a header across two memcpy ranges — the classic ring bug;
//   * write index advances by exactly one slot, so the lock-free protocol is
//     a single Atomics.store of a monotonic counter;
//   * a 16-byte slot holds every event type we need with room to spare.
// The cost is internal fragmentation (a 6-byte note-on still burns 16 bytes).
// At our volumes (a few hundred events/quantum worst case) that is free.
//
// SharedArrayBuffer layout:
//   [ CTRL: Int32Array, 2 slots ]   <- writeIdx (i32[0]), readIdx (i32[1])
//   [ DATA: SLOT_BYTES * CAPACITY ]  <- the slot array, byte-addressed
//
//   writeIdx / readIdx are MONOTONIC slot counters (never wrapped). The actual
//   slot is (idx & (CAPACITY-1)); CAPACITY is a power of two. Monotonic
//   counters make empty (w==r) vs full (w-r==CAPACITY) unambiguous without a
//   wasted slot. They are i32 and will wrap at 2^31 slots (~6.8 years at
//   375 events/quantum @ 48k) — a non-issue for a spike, flagged for 0038.
//
// Per-slot record (16 bytes, little-endian):
//   off 0  u8   type      (EV_* below)
//   off 1  u8   offset    sample offset within the upcoming quantum, 0..Q-1
//   off 2  u16  paramIdx  CLAP param id (EV_PARAM only; else 0)
//   off 4  f32  value     note velocity / param value / bend / wheel
//   off 8  u8   note      MIDI note number (EV_NOTE_ON/OFF)
//   off 9  u8   flag      generic small int (key mode, sustain on/off, ...)
//   off 10 u16  seq       low 16 bits of a monotonic producer sequence, so the
//                          consumer can assert "no event dropped / reordered"
//   off 12 f32  _reserved
//
// MAX RECORD SIZE: 16 bytes (== SLOT_BYTES). RING CAPACITY: 1024 slots = 16 KiB
// of data (+ 8 bytes ctrl). That is 8 quanta of headroom even at a pathological
// 128 events/quantum, ~340 ms of buffering at 48 kHz / 128-frame quanta.
//
// ===========================================================================
// OVERFLOW POLICY:  BLOCK-WRITER (never drop)  — see findings doc for rationale
// ===========================================================================
// The producer is the main thread, which is allowed to stall a microsecond; the
// consumer is the realtime worklet, which is not. So on a full ring the WRITER
// fails the push (returns false) and the caller retries / coalesces, rather
// than the reader silently dropping musical events. drop-oldest would corrupt
// the slice loop (an unpaired note-off, a lost gesture-end). The ring is sized
// so block should never actually happen in practice; if it does it is a sign
// the audio thread has died, and dropping events would only mask that.

export const SLOT_BYTES = 16;
export const CTRL_I32 = 2; // writeIdx, readIdx
export const CTRL_BYTES = CTRL_I32 * 4;
export const DEFAULT_CAPACITY = 1024; // slots; must be power of two

// Event type tags. Mirror vxn-core-clap dispatch_event semantics; the subset
// the spike exercises is NOTE_ON / NOTE_OFF / PARAM. The rest are reserved so
// the framing is the one 0037 inherits.
export const EV_NOTE_ON = 1;
export const EV_NOTE_OFF = 2;
export const EV_PARAM = 3; // set_param by CLAP id (plain value)
export const EV_PITCH_BEND = 4; // value in [-1, 1]
export const EV_MOD_WHEEL = 5; // value in [0, 1]
export const EV_SUSTAIN = 6; // flag 0/1
export const EV_KEY_MODE = 7; // flag = mode
export const EV_SPLIT_POINT = 8; // flag = note

const I_WRITE = 0;
const I_READ = 1;

function isPow2(n) {
  return n > 0 && (n & (n - 1)) === 0;
}

/// Allocate a fresh SAB sized for `capacity` slots. Returns the SAB; both
/// threads then construct an EventRing view over it. In the browser the main
/// thread allocates and posts the SAB to the worklet via processorOptions.
export function createRingSAB(capacity = DEFAULT_CAPACITY) {
  if (!isPow2(capacity)) throw new Error("capacity must be a power of two");
  const bytes = CTRL_BYTES + SLOT_BYTES * capacity;
  // In Node without isolation a SharedArrayBuffer is still constructible; the
  // worklet case needs crossOriginIsolated, proven separately by the headers.
  const Buf = typeof SharedArrayBuffer !== "undefined" ? SharedArrayBuffer : ArrayBuffer;
  return new Buf(bytes);
}

/// SPSC ring view over a SAB. Both producer (main) and consumer (worklet)
/// construct one of these over the SAME SAB. Lock-free: the only cross-thread
/// state is the two monotonic i32 counters, accessed via Atomics. No
/// Atomics.wait anywhere — the consumer free-polls in process().
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

  // Returns the next producer sequence number that push* will stamp. Tests
  // use it to predict the expected seq stream.
  peekSeq() {
    return this._seq & 0xffff;
  }

  // Low-level slot writer. BLOCK-WRITER overflow policy: returns false if the
  // ring is full so the caller decides (retry/coalesce). Acquire-load the
  // reader index, release-store the writer index AFTER the slot bytes land, so
  // the consumer never observes a half-written slot.
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
    // Release: publish the new write index only after the slot is fully written.
    Atomics.store(this.ctrl, I_WRITE, w + 1);
    return true;
  }

  pushNoteOn(offset, note, velocity) {
    return this._push(EV_NOTE_ON, offset, 0, velocity, note, 0);
  }
  pushNoteOff(offset, note) {
    return this._push(EV_NOTE_OFF, offset, 0, 0, note, 0);
  }
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

  // ---- consumer side (worklet render thread) ----------------------------

  // Number of records currently waiting.
  pending() {
    return Atomics.load(this.ctrl, I_WRITE) - Atomics.load(this.ctrl, I_READ);
  }

  // Drain ALL currently-available records into `out` (an array reused across
  // calls to avoid render-thread allocation). Each entry is the decoded record.
  // Acquire-load the writer index first so we only read slots it has published;
  // release-store the reader index after, so the producer can reclaim them.
  // No Atomics.wait — pure free-poll, safe on the render thread.
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

  // Drain raw wire bytes (the 16-byte slots verbatim, arrival order, wrap
  // handled) into `dstU8` — a byte view, e.g. over wasm linear memory. Returns
  // the record COUNT copied. This is the 0038 audio-host path: the ring's bytes
  // ARE the codec's input, so we copy them straight into the wasm decode scratch
  // with no per-record JS object churn. Caps at dstU8's record capacity; only
  // the records actually copied are reclaimed (the rest stay for next drain), so
  // a too-small destination degrades gracefully rather than dropping events.
  // Acquire-load writer first; release-store reader after — same SPSC discipline
  // as drainInto, no Atomics.wait.
  drainRawInto(dstU8) {
    const w = Atomics.load(this.ctrl, I_WRITE); // acquire
    let r = Atomics.load(this.ctrl, I_READ);
    const maxRecs = (dstU8.length / SLOT_BYTES) | 0;
    const src = this.bytes;
    let count = 0;
    while (r !== w && count < maxRecs) {
      const sbase = (r & this.mask) * SLOT_BYTES;
      const dbase = count * SLOT_BYTES;
      // Direct 16-byte copy, not src.subarray(...) → dst.set(...): subarray
      // allocates a fresh view per event on the audio thread, churning the GC
      // (Safari's JSC stalls the render thread on collection → audible blips).
      for (let k = 0; k < SLOT_BYTES; k++) dstU8[dbase + k] = src[sbase + k];
      r++;
      count++;
    }
    Atomics.store(this.ctrl, I_READ, r); // release: reclaim only what we copied
    return count;
  }
}

// ===========================================================================
// BLOCK-SLICING — the CLAP batch loop, ported (vxn-clap/src/lib.rs:335-369)
// ===========================================================================
//
// `engine` is a thin facade so the same loop drives the wasm in both the
// harness and the worklet:
//   engine.setParam(idx, value)
//   engine.noteOn(note, vel) / noteOff(note)
//   engine.pitchBend(v) / modWheel(v) / sustain(on) ...
//   engine.processSlice(start, end)   // render [start,end) of the quantum
//
// `records` MUST be sorted by offset ascending (the producer writes them in
// time order, and a single producer guarantees that). We apply every event at
// a given offset, then render up to the NEXT distinct offset — exactly the
// plugin: apply the batch, then process the batch's sample bounds.
//
// `quantum` is the frame count for this process() call (128 in Web Audio).

export function applyRecord(engine, rec) {
  switch (rec.type) {
    case EV_NOTE_ON:
      engine.noteOn(rec.note, rec.value);
      break;
    case EV_NOTE_OFF:
      engine.noteOff(rec.note);
      break;
    case EV_PARAM:
      engine.setParam(rec.paramIdx, rec.value);
      break;
    case EV_PITCH_BEND:
      engine.pitchBend?.(rec.value);
      break;
    case EV_MOD_WHEEL:
      engine.modWheel?.(rec.value);
      break;
    case EV_SUSTAIN:
      engine.sustain?.(rec.flag !== 0);
      break;
    case EV_KEY_MODE:
      engine.keyMode?.(rec.flag);
      break;
    case EV_SPLIT_POINT:
      engine.splitPoint?.(rec.flag);
      break;
    default:
      break; // unknown type: ignore (forward-compat)
  }
}

// Render one quantum with sample-accurate event placement. Returns the number
// of slices rendered (>=1), purely for instrumentation.
export function renderQuantumSliced(engine, records, quantum) {
  let prev = 0;
  let slices = 0;
  let i = 0;
  const n = records.length;
  while (i < n) {
    const k = Math.min(records[i].offset, quantum);
    // Render everything strictly before this event's offset.
    if (k > prev) {
      engine.processSlice(prev, k);
      prev = k;
      slices++;
    }
    // Apply ALL events at this same offset (a batch boundary in CLAP terms).
    while (i < n && Math.min(records[i].offset, quantum) === k) {
      applyRecord(engine, records[i]);
      i++;
    }
  }
  // Render the tail.
  if (prev < quantum) {
    engine.processSlice(prev, quantum);
    slices++;
  }
  return slices;
}

// Apply-at-block-start path (the simpler alternative we measure against): apply
// every event with NO offset, then render the whole quantum in one go. This is
// what postMessage-style delivery degrades to — every event lands at frame 0.
export function renderQuantumBlockStart(engine, records, quantum) {
  for (let i = 0; i < records.length; i++) applyRecord(engine, records[i]);
  engine.processSlice(0, quantum);
  return 1;
}
