// Worklet lifecycle runner (0289) — hardens the audio host for real use:
// instantiate-from-bytes, silence-until-ready, sample rate, reset, teardown, and
// render-thread TRAP SAFETY.
//
// ONE code path, shared by the AudioWorklet (`vxn1b-processor.js`) and the
// headless tests.
//
// Split of concerns: `AudioHost` is the steady-state render loop, this runner is
// the lifecycle and failure policy around it. The runner holds the wasm bytes
// and the SAB references for construction and teardown — NOT so it can rebuild
// after a trap. It deliberately does not; see below.
//
// ===========================================================================
// A TRAP STOPS THE AUDIO. IT DOES NOT PRETEND TO RECOVER. (ticket 0297)
// ===========================================================================
//
// vxn-1's runner re-instantiates over the same SABs after a trap, so audio comes
// back. VXN1b deliberately does not, because here that would come back WRONG.
//
// A rebuilt engine reloads its params for free — the worklet-side mirror is
// NaN-seeded, so the first fold reapplies every id from the store. But none of
// the non-automatable state returns: key mode, split point, LFO 2 link, the
// whole per-layer matrix topology, the scope tap and the tempo are not in the
// store, and a ring that has already delivered them will not resend. The synth
// would resume playing a different patch, with nothing on screen saying so.
//
// For a demo, "the sound stopped, reload the page" beats that. So: catch the
// trap (it must not escape `process()` and wedge the context), go silent, report
// loudly — and stop.

import { AudioHost } from "./audio-host.mjs";

export class WorkletHostRunner {
  constructor({
    wasmBytes,
    ringSab = null,
    storeSab = null,
    telemetrySab = null,
    sampleRate,
    capacity,
    onTrap,
    onReady,
  } = {}) {
    this.wasmBytes = wasmBytes;
    this.ringSab = ringSab;
    this.storeSab = storeSab;
    this.telemetrySab = telemetrySab;
    this.sampleRate = sampleRate;
    this.capacity = capacity;
    // Deliberately not a no-op default: an unhandled trap means the engine was
    // rebuilt and the non-automatable state went with it. Better a console line
    // than silence.
    this.onTrap =
      onTrap ||
      ((e, count) =>
        console.warn(`vxn1b: render trap #${count} — audio stopped; reload the page`, e));
    this.onReady = onReady || (() => {});

    this.host = null;
    this.ready = false;
    this.trapCount = 0;
  }

  /// Instantiate the wasm and build the host. Until it resolves, `process()`
  /// renders silence and the ring buffers whatever the producer writes — its
  /// read index is untouched while not ready, so nothing is lost.
  async init() {
    await this._instantiate();
  }

  async _instantiate() {
    const { instance } = await WebAssembly.instantiate(this.wasmBytes, {});
    this.host = new AudioHost(instance.exports, {
      ringSab: this.ringSab,
      storeSab: this.storeSab,
      telemetrySab: this.telemetrySab,
      sampleRate: this.sampleRate,
      capacity: this.capacity,
    });
    this.ready = true;
    this.onReady();
  }

  /// All-notes-off without dropping ring or store state. Called on resume after
  /// a suspend, to clear voices that were mid-flight when audio stopped.
  reset() {
    if (this.host) this.host.reset();
  }

  /// Render one quantum. Silence until ready. A trap in the wasm render is
  /// caught HERE, at the worklet boundary: output silence and notify. The engine
  /// stays down — see the note at the top of this file. Returns true iff real
  /// audio was rendered.
  process(outL, outR) {
    if (!this.ready || !this.host) {
      outL.fill(0);
      if (outR) outR.fill(0);
      return false;
    }
    try {
      this.host.process(outL, outR);
      return true;
    } catch (e) {
      // The instance is poisoned after a trap. Drop it and stay down.
      this.ready = false;
      this.host = null;
      this.trapCount++;
      outL.fill(0);
      if (outR) outR.fill(0);
      this.onTrap(e, this.trapCount);
      return false;
    }
  }

  /// Free the engine and release SAB references so nothing leaks across re-init.
  destroy() {
    this.ready = false;
    if (this.host) {
      this.host.destroy();
      this.host = null;
    }
    this.ringSab = null;
    this.storeSab = null;
    this.telemetrySab = null;
    this.wasmBytes = null;
  }
}
