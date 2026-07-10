// Faceplate ⇄ audio bridge (ticket 0157) — the transport swap.
//
// The vxn-2 faceplate assets (bootstrap.js / main.js / panels) were written for
// the native wry editor: UI intents go out as `window.ipc.postMessage(JSON)` and
// view updates come in via `window.__vxn.applyViewEvents(events)`. This module
// keeps those assets UNCHANGED and swaps only the transport underneath:
//
//   - installs a `window.ipc` whose `postMessage(json)` routes each `{op, ...}`
//     opcode to the controller wasm (ticket 0154) over its C-ABI, and unlocks
//     audio on the first user gesture;
//   - runs an rAF loop that ticks the controller, mirrors its param values into
//     the worklet's store SAB (so edits reach the engine), decodes the packed
//     ViewEvent drain into event objects, and feeds them to the faceplate's
//     `applyViewEvents` — the same objects the native path delivered.
//
// The audio engine runs in the worklet (coordinator.mjs / WebHost); the
// controller runs on the main thread (controller.mjs / WebController). They share
// the param + event SABs, never linear memory.

import { WebHost } from "./coordinator.mjs";
import { WebController } from "./controller.mjs";

// Opcodes that don't touch the model / audio and can be ignored on the web path
// until browser persistence (0159) wires them. Kept explicit so an unhandled
// opcode is a loud console warning, not a silent drop.
const DEFERRED_OPS = new Set(["request_text_input"]);

/// Route ONE decoded `{op, ...}` opcode to the controller. Pure (no audio side
/// effects) so it's unit-testable against a mock controller. Returns true if the
/// opcode was handled.
///
/// WIRE QUIRK (matches the native faceplate): `bootstrap.js.dispatch` builds the
/// message as `Object.assign({ op: opcode }, payload)`, so a payload that itself
/// carries an `op` field CLOBBERS the opcode string. The op-indexed custom ops
/// (`set_op_tab` / `set_ks_curve` / `set_eg_curve`) send `{ op: <operatorIndex> }`
/// — so on the wire their `op` is a NUMBER (the operator), not the opcode string,
/// and the opcode name is gone. We recover the intent by field presence, which is
/// unambiguous: only these three ops ever put a number in `op`, and they differ by
/// which of `side` / `curve` they carry. (The native `parse_ui_event` reads
/// `op.as_str()` and DROPS these — a latent native bug the optimistic UI paint
/// hides; the web path handles them correctly.)
export function routeOpcode(ctrl, msg) {
  if (!msg) return false;

  // Numeric `op` == the operator-index collision case (set_op_tab / ks / eg).
  if (typeof msg.op === "number") {
    const opIndex = msg.op;
    if (msg.side != null && msg.curve != null) {
      ctrl.setKsCurve(opIndex, msg.side, msg.curve); // {op, side, curve}
    } else if (msg.curve != null) {
      ctrl.setEgCurve(opIndex, msg.curve); // {op, curve}
    } else {
      ctrl.setOpTab(opIndex); // {op}
    }
    return true;
  }

  if (typeof msg.op !== "string") return false;
  switch (msg.op) {
    case "begin_gesture":
      ctrl.beginGesture(msg.id);
      return true;
    case "end_gesture":
      ctrl.endGesture(msg.id);
      return true;
    case "set_param":
      ctrl.setParam(msg.id, msg.plain);
      return true;
    case "set_param_norm":
      ctrl.setParamNorm(msg.id, msg.norm);
      return true;
    case "ready":
      ctrl.editorReady();
      return true;
    case "request_full_rebroadcast":
      ctrl.requestFullRebroadcast();
      return true;
    case "set_matrix_row":
      ctrl.setMatrixRow(msg.slot, msg.source, msg.dest, msg.curve, msg.active, msg.depth);
      return true;
    case "request_matrix_snapshot":
      ctrl.requestMatrixSnapshot();
      return true;
    case "request_ks_curve_snapshot":
      ctrl.requestKsCurveSnapshot();
      return true;
    case "request_eg_curve_snapshot":
      ctrl.requestEgCurveSnapshot();
      return true;
    default:
      if (DEFERRED_OPS.has(msg.op)) return true; // known-but-deferred (0159)
      console.warn("vxn2 bridge: unhandled opcode", msg.op);
      return false;
  }
}

export class FaceplateBridge {
  // Options:
  //   wasmUrl / controllerWasmUrl : dist-relative URLs of the two wasm modules.
  //   wasmBytes / controllerBytes : pre-fetched bytes (node test; skip fetch).
  //   doc / win                   : DOM seams (default document / globalThis).
  //   hostOptions                 : extra WebHost options (AudioContext seam, …).
  //   rafImpl                     : requestAnimationFrame seam (default global).
  constructor({
    wasmUrl,
    controllerWasmUrl,
    wasmBytes = null,
    controllerBytes = null,
    doc = globalThis.document,
    win = globalThis,
    hostOptions = {},
    rafImpl = null,
  } = {}) {
    this._doc = doc;
    this._win = win;
    this._raf = rafImpl || (win.requestAnimationFrame ? win.requestAnimationFrame.bind(win) : null);

    this.host = new WebHost({ wasmUrl, wasmBytes, ...hostOptions });
    this.controller = new WebController({
      wasmUrl: controllerWasmUrl,
      wasmBytes: controllerBytes,
      store: this.host.store, // controller mirrors its model into the worklet store
    });

    this._ready = false; // controller instantiated
    this._queue = []; // opcodes posted before the controller is live
    this._audioStarted = false;
    this._running = false; // rAF loop active
  }

  // Boot: install the `window.ipc` shim immediately (so no opcode posted during
  // faceplate boot is lost), instantiate the controller, arm the one-shot audio
  // unlock, then start the rAF pump. Resolves once the controller is live.
  async boot() {
    this._installIpc();
    // Replay opcodes the generated page's boot-queue stub captured before this
    // module loaded (notably the faceplate's `ready`). They re-enter through the
    // real `window.ipc.postMessage`, which queues them until the controller is
    // live (below).
    const bootQueue = this._win.__vxnBootQueue;
    if (Array.isArray(bootQueue)) {
      for (const json of bootQueue) this._win.ipc.postMessage(json);
      this._win.__vxnBootQueue = null;
    }
    this._armAudioUnlock();
    await this.controller.instantiate();
    this._ready = true;
    // Flush any opcodes the faceplate posted while we were instantiating.
    for (const msg of this._queue) routeOpcode(this.controller, msg);
    this._queue.length = 0;
    this._startPump();
    return this;
  }

  _installIpc() {
    const bridge = this;
    // The faceplate's `dispatch` calls `window.ipc.postMessage(JSON.stringify(m))`.
    this._win.ipc = {
      postMessage(json) {
        let msg;
        try {
          msg = JSON.parse(json);
        } catch (e) {
          console.error("vxn2 bridge: bad ipc payload", e, json);
          return;
        }
        if (!bridge._ready) {
          bridge._queue.push(msg);
          return;
        }
        routeOpcode(bridge.controller, msg);
      },
    };
  }

  // Audio can only start inside a user-gesture call stack (autoplay policy). Arm a
  // one-shot capture-phase pointerdown/keydown on the document that unlocks it.
  _armAudioUnlock() {
    if (!this._doc || typeof this._doc.addEventListener !== "function") return;
    const unlock = () => {
      this._doc.removeEventListener("pointerdown", unlock, true);
      this._doc.removeEventListener("keydown", unlock, true);
      this.startAudio();
    };
    this._doc.addEventListener("pointerdown", unlock, true);
    this._doc.addEventListener("keydown", unlock, true);
  }

  // Start the audio graph (must be called from a user gesture). Idempotent. After
  // the coordinator seeds the store with engine defaults, force the controller to
  // re-mirror its authoritative model values so an edit made on the unlock gesture
  // isn't clobbered by the default seed.
  async startAudio() {
    if (this._audioStarted) return;
    this._audioStarted = true;
    try {
      await this.host.start();
      this.controller.remirrorStore();
    } catch (e) {
      console.error("vxn2 bridge: audio start failed", e);
      this._audioStarted = false;
    }
  }

  _startPump() {
    if (this._running || !this._raf) return;
    this._running = true;
    const pump = () => {
      if (!this._running) return;
      this._pumpOnce();
      this._raf(pump);
    };
    this._raf(pump);
  }

  // One controller tick → mirror values → decode ViewEvents → hand them to the
  // faceplate. Public so the node test can drive it deterministically without rAF.
  _pumpOnce() {
    const events = this.controller.tick();
    if (events.length) {
      const apply = this._win.__vxn && this._win.__vxn.applyViewEvents;
      if (typeof apply === "function") apply(events);
    }
  }

  async destroy() {
    this._running = false;
    this.controller.destroy();
    await this.host.teardown();
  }
}

// Convenience boot used by the generated index.html: fetch both wasm modules
// (the coordinator/controller default URLs), then boot the bridge.
export async function bootFaceplate(opts = {}) {
  const bridge = new FaceplateBridge(opts);
  await bridge.boot();
  return bridge;
}
