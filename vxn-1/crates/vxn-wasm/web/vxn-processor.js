// AudioWorkletProcessor driving the VXN1 wasm engine (ticket 0034 spike).
//
// The worklet scope has no fetch/import, so the main thread hands us the
// compiled wasm bytes via processorOptions. We instantiate inside the
// constructor (async) and render silence until ready, then one quantum of
// Synth::process per process() call straight out of linear memory.
class VxnProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.ready = false;
    this.synth = 0;
    this.pendingNotes = [];

    // Note on/off arrive from the main thread over the port.
    this.port.onmessage = (e) => {
      const m = e.data;
      if (!this.ready) {
        this.pendingNotes.push(m);
        return;
      }
      this._apply(m);
    };

    const bytes = options.processorOptions.wasmBytes;
    WebAssembly.instantiate(bytes, {}).then(({ instance }) => {
      this.x = instance.exports;
      this.Q = this.x.vxn_quantum();
      this.synth = this.x.vxn_new(sampleRate); // sampleRate is a worklet global
      this.ready = true;
      for (const m of this.pendingNotes) this._apply(m);
      this.pendingNotes.length = 0;
    });
  }

  _apply(m) {
    if (m.type === "noteOn") this.x.vxn_note_on(this.synth, m.note, m.velocity);
    else if (m.type === "noteOff") this.x.vxn_note_off(this.synth, m.note);
  }

  process(_inputs, outputs) {
    const out = outputs[0];
    if (!this.ready) return true; // silence until wasm is live

    this.x.vxn_process(this.synth);
    const buf = this.x.memory.buffer;
    const l = new Float32Array(buf, this.x.vxn_out_l(this.synth), this.Q);
    const r = new Float32Array(buf, this.x.vxn_out_r(this.synth), this.Q);
    out[0].set(l);
    if (out[1]) out[1].set(r);
    return true;
  }
}

registerProcessor("vxn-processor", VxnProcessor);
