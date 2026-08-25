// Audio -> view telemetry: meter and scope frames (0288).
//
// The first thing in any VXN web port that travels UP from the worklet. Every
// other channel is main -> worklet; this one is the reverse, and it exists
// because VXN1b's faceplate shows live audio (six meter bars and a scope strip)
// that natively costs nothing: `MeterBus` / `ScopeBus` are Arc-shared with the
// ~60 Hz timer and the frames ride the existing ViewEvent batch. Here the engine
// is a separate wasm with its own linear memory, so the frames have to be
// carried across explicitly.
//
// ===========================================================================
// WHY A SAB AND NOT postMessage
// ===========================================================================
//
// `port.postMessage` allocates per message ON THE AUDIO THREAD. At 60 Hz with a
// 384-sample scope window that is real GC churn inside the render callback, and
// Safari's JSC stalls the render thread on collection — the documented cause of
// VXN1's audio blips. So: a second SharedArrayBuffer, written by the worklet,
// read by the main thread on rAF. Steady state allocates nothing on either side.
//
// ===========================================================================
// LAYOUT (one buffer)
// ===========================================================================
//
//   i32[0] meterSeq   seqlock counter: even = stable, odd = mid-write
//   i32[1] scopeSeq   ditto
//   i32[2] scopeLen   samples valid in the scope region (0 = no frame yet)
//   i32[3] reserved
//   f32[4 ..)         meter frame, MeterTap order, linear peak magnitudes
//   f32[..)           scope window, oldest -> newest
//
// Region sizes come from the wasm (`vxn1b_meter_len()`, `vxn1b_scope_window()`),
// never from literals here — adding a meter tap must not silently truncate the
// frame. Ticket 0285 is the standing reminder of what hand-copied sizes cost.
//
// ===========================================================================
// WHY A SEQLOCK
// ===========================================================================
//
// The param store gets away with plain per-slot atomics because each slot is
// independently meaningful: a reader seeing some new and some old params is
// fine. A FRAME is not like that. A scope window stitched from two different
// captures shows a discontinuity, which reads as a glitch in the trace rather
// than as slightly stale data.
//
// So each region carries a seqlock. The writer bumps the counter to odd, writes,
// bumps to even. The reader takes the counter, reads, re-takes it, and retries
// if it changed or was odd. The writer NEVER blocks — two atomic stores, no CAS,
// no waiting, which is mandatory on the render thread — and the reader is the
// main thread, which can afford to retry. It retries a bounded number of times
// and then keeps the previous frame; a dropped visual frame is not worth
// spinning rAF over.
//
// Ordering: `Atomics.store` / `Atomics.load` are sequentially consistent, so a
// reader that observes the even counter also observes the plain float writes
// that preceded it. That is what makes the plain (non-atomic) writes to the
// float regions safe between the two atomic stores.

const I_METER_SEQ = 0;
const I_SCOPE_SEQ = 1;
const I_SCOPE_LEN = 2;
const CTRL_I32 = 4; // meterSeq, scopeSeq, scopeLen, reserved
const CTRL_BYTES = CTRL_I32 * 4;

/// How many times a reader re-attempts a torn read before giving up and keeping
/// the frame it already had. A handful is plenty: the writer's critical section
/// is a memcpy of at most a few hundred floats, so losing three races in a row
/// means the audio thread is in trouble and a stale meter is the least of it.
const MAX_READ_RETRIES = 3;

/// Byte size of a telemetry SAB for the given region lengths.
export function telemetryBytes(meterLen, scopeWindow) {
  return CTRL_BYTES + (meterLen + scopeWindow) * 4;
}

/// Allocate the telemetry SAB. `meterLen` and `scopeWindow` must come from the
/// wasm exports, so the buffer always matches the engine that fills it.
export function createTelemetrySAB(meterLen, scopeWindow) {
  const Buf = typeof SharedArrayBuffer !== "undefined" ? SharedArrayBuffer : ArrayBuffer;
  return new Buf(telemetryBytes(meterLen, scopeWindow));
}

/// Shared view construction. Both sides build the same regions over the same
/// buffer; only which methods they call differs.
class TelemetryView {
  constructor(sab, meterLen, scopeWindow) {
    this.meterLen = meterLen;
    this.scopeWindow = scopeWindow;
    this.ctrl = new Int32Array(sab, 0, CTRL_I32);
    this.meter = new Float32Array(sab, CTRL_BYTES, meterLen);
    this.scope = new Float32Array(sab, CTRL_BYTES + meterLen * 4, scopeWindow);
  }
}

// ===========================================================================
// WORKLET SIDE — the writer
// ===========================================================================
//
// Rate division, and why it is not "every quantum": the meter drain is
// READ-AND-CLEAR — each frame reports the extreme since the previous drain.
// Natively that drain runs on the ~60 Hz timer, so a frame covers the whole
// interval the UI is about to display. Draining every quantum would mean ~375
// drains a second against a 60 Hz reader, so the SAB would hold only the newest
// quantum's peak and the other ~5 would be discarded unseen — a transient
// landing in a discarded quantum would simply never appear on the meter.
//
// So the writer divides down to ~60 Hz, and publishes the scope every second
// meter publish (~30 Hz), matching the native SCOPE_TICK_DIVISOR.

export class TelemetryWriter extends TelemetryView {
  /// `engine` is the wasm-export shim: drainMeters(), meterFrame(),
  /// readScope() -> count, scopeSamples(count). `sampleRate` and `quantum` set
  /// the rate division.
  constructor(sab, { meterLen, scopeWindow, engine, sampleRate, quantum = 128 }) {
    super(sab, meterLen, scopeWindow);
    this.engine = engine;
    // Quanta per publish, targeting ~60 Hz. At 48 kHz / 128 frames that is 6
    // (~62.5 Hz); at 44.1 kHz it is 6 (~57.4 Hz). Never below 1.
    this.everyN = Math.max(1, Math.round(sampleRate / quantum / 60));
    this._tick = 0;
    // The scope publishes every second meter publish (native SCOPE_TICK_DIVISOR).
    this._scopeTurn = 0;
  }

  /// Call once per rendered quantum, AFTER the render. Publishes on the divided
  /// tick and does nothing on the others, so the common quantum costs one
  /// increment and a compare.
  tick() {
    if (++this._tick < this.everyN) return false;
    this._tick = 0;
    this.publishMeters();
    if ((this._scopeTurn ^= 1) === 0) this.publishScope();
    return true;
  }

  /// Drain the meter bus and publish the frame under the seqlock.
  publishMeters() {
    this.engine.drainMeters();
    const src = this.engine.meterFrame(); // Float32Array over wasm memory
    const seq = Atomics.load(this.ctrl, I_METER_SEQ);
    Atomics.store(this.ctrl, I_METER_SEQ, seq + 1); // odd: writing
    this.meter.set(src.subarray(0, this.meterLen));
    Atomics.store(this.ctrl, I_METER_SEQ, seq + 2); // even: stable
  }

  /// Read the latest scope window and publish it under the seqlock. A read that
  /// finds no full window (freshly cleared ring, or the tap is Off) publishes
  /// nothing and leaves the previous frame in place.
  publishScope() {
    const count = this.engine.readScope();
    if (count === 0) return false;
    const src = this.engine.scopeSamples(count);
    const seq = Atomics.load(this.ctrl, I_SCOPE_SEQ);
    Atomics.store(this.ctrl, I_SCOPE_SEQ, seq + 1); // odd: writing
    this.scope.set(src.subarray(0, Math.min(count, this.scopeWindow)));
    Atomics.store(this.ctrl, I_SCOPE_LEN, Math.min(count, this.scopeWindow));
    Atomics.store(this.ctrl, I_SCOPE_SEQ, seq + 2); // even: stable
    return true;
  }
}

// ===========================================================================
// MAIN SIDE — the reader
// ===========================================================================
//
// Silence suppression lives here, not on the audio thread. Natively the tick
// stops pushing once a silent frame has been sent, so an idle plugin does not
// stream 60 identical frames a second across the bridge. On the web the SAB
// write is nearly free and the reader polls regardless, so the writer stays
// dumb and unconditional and the POLICY lives on the main thread — same
// observable behaviour, none of it on the render thread.

export class TelemetryReader extends TelemetryView {
  constructor(sab, { meterLen, scopeWindow }) {
    super(sab, meterLen, scopeWindow);
    // Scratch the caller reads from; reused so a 60 Hz poll allocates nothing.
    this._meterOut = new Float32Array(meterLen);
    this._scopeOut = new Float32Array(scopeWindow);
    // Seeded to 0, the counter's value BEFORE any publish — not -1. A -1 seed
    // makes the very first read see "0 !== -1", conclude something is new, and
    // hand back the still-zeroed region as though the engine had published
    // silence. That fabricated frame then consumes the one silent frame the
    // rule below allows, so the real first frame is the one that gets
    // suppressed. The writer's first publish takes the counter 0 -> 2, so a
    // genuine frame is always distinguishable from "nothing yet".
    this._meterSeen = 0;
    this._scopeSeen = 0;
    // Silence rule: deliver the FIRST silent frame (the view needs the zero that
    // starts its decay falling, or the flat line that settles the trace), then
    // suppress until something is audible again.
    this._sentSilentMeter = false;
    this._sentSilentScope = false;
  }

  /// Read the meter frame, or `null` if there is nothing new to deliver —
  /// either the frame is unchanged, a torn read could not be resolved, or the
  /// silence rule suppressed it.
  readMeters() {
    const frame = this._readRegion(I_METER_SEQ, this.meter, this._meterOut, this.meterLen);
    if (frame === null) return null;
    const silent = isSilent(frame);
    if (silent && this._sentSilentMeter) return null;
    this._sentSilentMeter = silent;
    return frame;
  }

  /// Read the scope window, or `null` on nothing-new / torn / suppressed.
  readScope() {
    // Read outside the seqlock deliberately: the length only ever goes 0 ->
    // SCOPE_WINDOW, once, when the ring first fills. It never shrinks and never
    // varies after, so there is no length/data pairing for a torn read to get
    // wrong — and the data itself is still covered by the lock below.
    const len = Atomics.load(this.ctrl, I_SCOPE_LEN);
    if (len === 0) return null; // no frame captured yet (tap off, or ring cold)
    const frame = this._readRegion(I_SCOPE_SEQ, this.scope, this._scopeOut, len);
    if (frame === null) return null;
    const silent = isSilent(frame);
    if (silent && this._sentSilentScope) return null;
    this._sentSilentScope = silent;
    return frame;
  }

  /// Seqlock read of one region into `out`. Returns a subarray view of `out`, or
  /// `null` if the sequence never settled or has not advanced since last time.
  _readRegion(seqIndex, src, out, len) {
    for (let attempt = 0; attempt <= MAX_READ_RETRIES; attempt++) {
      const before = Atomics.load(this.ctrl, seqIndex);
      if (before & 1) continue; // writer mid-update; take another look
      // Nothing published since the last successful read.
      const seen = seqIndex === I_METER_SEQ ? this._meterSeen : this._scopeSeen;
      if (before === seen) return null;
      for (let i = 0; i < len; i++) out[i] = src[i];
      // If the counter is unchanged, nothing overwrote what we just copied.
      if (Atomics.load(this.ctrl, seqIndex) !== before) continue; // torn: retry
      if (seqIndex === I_METER_SEQ) this._meterSeen = before;
      else this._scopeSeen = before;
      return out.subarray(0, len);
    }
    return null; // gave up; the caller keeps the frame it already had
  }
}

/// Every sample/tap at rest. The view still needs the FIRST such frame — it is
/// what starts a meter's decay falling and settles a trace on the centre line.
function isSilent(frame) {
  for (let i = 0; i < frame.length; i++) {
    if (frame[i] !== 0) return false;
  }
  return true;
}

export { CTRL_BYTES, I_METER_SEQ, I_SCOPE_SEQ, I_SCOPE_LEN, MAX_READ_RETRIES };
