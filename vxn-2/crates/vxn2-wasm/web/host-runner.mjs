// Worklet lifecycle runner (ticket 0156) — hardens the audio-host for real-world
// use: instantiate-from-bytes, silence-until-ready, sample-rate, suspend/resume,
// teardown, and render-thread TRAP SAFETY. Ported from vxn-1's `host-runner`;
// the vxn-2 change is that there's no key-mode/split shared state to buffer.
//
// The runner owns the wasm bytes and the SABs so it can re-instantiate after a
// trap WITHOUT losing transport state — the ring's read/write indices and the
// param store live in the SABs, so a fresh AudioHost over the same SABs resumes
// exactly where the dead one left off.

import { AudioHost } from "./audio-host.mjs";

export class WorkletHostRunner {
  constructor({ wasmBytes, ringSab = null, storeSab = null, sampleRate, capacity, onTrap, onReady } = {}) {
    this.wasmBytes = wasmBytes;
    this.ringSab = ringSab;
    this.storeSab = storeSab;
    this.sampleRate = sampleRate;
    this.capacity = capacity;
    this.onTrap = onTrap || (() => {});
    this.onReady = onReady || (() => {});

    this.host = null;
    this.ready = false;
    this.trapCount = 0;
    this._reinitInFlight = false;
  }

  // Instantiate the wasm and build the host. Async; until it resolves, process()
  // outputs silence and the ring buffers any events the producer writes.
  async init() {
    await this._instantiate();
  }

  async _instantiate() {
    const { instance } = await WebAssembly.instantiate(this.wasmBytes, {});
    this.host = new AudioHost(instance.exports, {
      ringSab: this.ringSab,
      storeSab: this.storeSab,
      sampleRate: this.sampleRate,
      capacity: this.capacity,
    });
    this.ready = true;
    this.onReady();
  }

  // Context sample-rate change. AudioWorklet sampleRate is fixed per context, so
  // a real change means a new context; this wires the engine call for
  // completeness / offline render.
  setSampleRate(sr) {
    this.sampleRate = sr;
    if (this.host) this.host.setSampleRate(sr);
  }

  // All-notes-off without dropping ring/store state — call on resume after a long
  // suspend to clear any voices that were mid-flight when audio stopped.
  reset() {
    if (this.host) this.host.reset();
  }

  // Render one quantum. Silence until ready. A trap/panic in the wasm render is
  // caught HERE (the worklet boundary): output silence, notify main, and kick an
  // async re-instantiate over the same SABs so audio recovers instead of the
  // context being permanently wedged. Returns true iff real audio was rendered.
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
          .catch(() => {}) // recovery best-effort; stays silent if it can't
          .finally(() => {
            this._reinitInFlight = false;
          });
      }
      return false;
    }
  }

  // Free the engine and release SAB references so nothing leaks across re-init.
  destroy() {
    this.ready = false;
    if (this.host) {
      this.host.destroy();
      this.host = null;
    }
    this.ringSab = null;
    this.storeSab = null;
    this.wasmBytes = null;
  }
}
