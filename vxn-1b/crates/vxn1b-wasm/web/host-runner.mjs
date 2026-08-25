// Worklet lifecycle runner (0289) — hardens the audio host for real use:
// instantiate-from-bytes, silence-until-ready, sample rate, reset, teardown, and
// render-thread TRAP SAFETY.
//
// ONE code path, shared by the AudioWorklet (`vxn1b-processor.js`) and the
// headless tests.
//
// Split of concerns: `AudioHost` is the steady-state render loop, this runner is
// the lifecycle and failure policy around it. The runner owns the wasm bytes and
// the SABs so it can re-instantiate after a trap WITHOUT losing transport state
// — the ring's read/write indices and the param store live in the SABs, so a
// fresh host over the same SABs resumes exactly where the dead one left off.
//
// ===========================================================================
// WHAT A TRAP COSTS, AND WHY THIS RUNNER DOES NOT TRY TO FIX IT ALL
// ===========================================================================
//
// After a re-instantiate the params come back on their own: the worklet-side
// mirror is NaN-seeded, so the first fold reapplies every id from the store.
//
// The NON-automatable state does not. VXN1b's is key state (mode, split point,
// LFO 2 link), the whole per-layer matrix topology, the scope tap and the tempo
// — none of it in the param store, and none of it re-sent by a ring that has
// already delivered it. vxn-1's runner shadows its equivalent because its
// equivalent is two bytes; VXN1b's would mean decoding every record this runner
// currently copies as opaque bytes, and keeping a second copy of the topology
// free to drift from the engine's.
//
// The controller already holds the authoritative model, so the replay belongs
// there. This runner raises the signal (`onTrap`) and the coordinator surfaces
// it; the re-broadcast lands with the controller bridge (0290). Until then a
// trap costs routing — which is why the default handler is LOUD rather than
// silent.

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
        console.warn(
          `vxn1b: render trap #${count} — engine rebuilt; routing state needs a ` +
            `re-broadcast from the controller`,
          e,
        ));
    this.onReady = onReady || (() => {});

    this.host = null;
    this.ready = false;
    this.trapCount = 0;
    this._reinitInFlight = false;
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

  setSampleRate(sr) {
    this.sampleRate = sr;
    if (this.host) this.host.setSampleRate(sr);
  }

  /// All-notes-off without dropping ring or store state. Called on resume after
  /// a suspend, to clear voices that were mid-flight when audio stopped.
  reset() {
    if (this.host) this.host.reset();
  }

  /// Render one quantum. Silence until ready. A trap in the wasm render is
  /// caught HERE, at the worklet boundary: output silence, notify, and kick an
  /// async re-instantiate over the same SABs so audio recovers instead of the
  /// context being permanently wedged. Returns true iff real audio was rendered.
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
      // The instance is poisoned after a trap; tear it down and rebuild.
      this.ready = false;
      this.host = null;
      this.trapCount++;
      outL.fill(0);
      if (outR) outR.fill(0);
      this.onTrap(e, this.trapCount);
      if (!this._reinitInFlight) {
        this._reinitInFlight = true;
        Promise.resolve()
          .then(() => this._instantiate())
          .catch(() => {}) // recovery is best-effort; stays silent if it can't
          .finally(() => {
            this._reinitInFlight = false;
          });
      }
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
