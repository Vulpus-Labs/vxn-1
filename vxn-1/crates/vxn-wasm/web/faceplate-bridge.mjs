// Faceplate transport bridge (E018 / 0057-0061) — the web replacement for the
// wry IPC + evaluate_script bridge the plugin uses.
//
// The vxn-1 faceplate is already an HTML/JS app behind a JSON OPCODE protocol:
//   - JS -> Rust: the page posts `window.ipc.postMessage(JSON.stringify({op,..}))`.
//   - Rust -> JS: the host batches `ViewEvent`s once per ~60 Hz timer tick and
//     calls `window.__vxn.applyViewEvents(arr)` (deduping ParamChanged by id).
//
// On the web there is no wry. This module keeps the OPCODE VOCABULARY BYTE-
// COMPATIBLE and only swaps the transport:
//   - JS -> controller (0058): `handleUiOpcode(json)` parses the same `{op,..}`
//     shapes and drives the controller wasm (controller.mjs WebController) over
//     its C-ABI opcode surface; the controller mutates vxn-app's model and the
//     changed values mirror into the SAME store SAB the worklet folds.
//   - controller -> JS (0059): each ~60 Hz tick drains the controller's
//     ViewEvents, translates them to the faceplate's `{kind,..}` shape, dedupes
//     ParamChanged by id, and calls the page dispatcher directly — no
//     evaluate_script.
//
// ViewEvents are driven by the controller's OWN model mutations (a `set_param`
// edit emits ParamChanged; a preset load fans out). The E015 readback diff pump
// is DORMANT in standalone web (no audio-thread param writer exists — 0044): the
// readback SAB region stays allocated-but-unpolled and NO rAF diff-readback pump
// is wired here.
//
// ONE code path, like the rest of E015/E016: `FaceplateBridge` is exercised by
// the Node test (faceplate-bridge.test.mjs) and the browser, so the headless
// proof is the byte-for-byte transport the page runs.

import { WebController, LAYER_UPPER, LAYER_LOWER } from "./controller.mjs";

// ---- opcode <-> controller routing helpers ---------------------------------

// `set_edit_layer` / `reset_layer` carry the layer as the faceplate's
// 'upper'/'lower' string (panels.js KEY_LAYERS); the controller wants 0/1.
function layerCode(layer) {
  return layer === "lower" ? LAYER_LOWER : LAYER_UPPER;
}

// Translate one decoded controller ViewEvent (controller.mjs `{type,..}`,
// PascalCase) into the faceplate dispatcher's `{kind,..}` shape (snake_case) —
// byte-identical to the plugin's `view_event_to_json` output.
export function viewEventToFaceplate(ev) {
  switch (ev.type) {
    case "ParamChanged":
      return {
        kind: "param_changed",
        id: ev.id,
        plain: ev.plain,
        norm: ev.norm,
        display: ev.display,
      };
    case "KeyModeChanged":
      // The faceplate `keysPanel.setMode` compares against the int mode index.
      return { kind: "key_mode_changed", mode: ev.mode };
    case "SplitPointChanged":
      return { kind: "split_point_changed", note: ev.note };
    case "EditLayerChanged":
      // The faceplate dispatcher reads `ev.layer === 'lower' ? ... : 'upper'`.
      return {
        kind: "edit_layer_changed",
        layer: ev.layer === LAYER_LOWER ? "lower" : "upper",
      };
    default:
      return null;
  }
}

// Dedupe ParamChanged by id (latest value wins, last-occurrence position kept
// relative to non-ParamChanged events) — the same rule as the native
// `vxn_core_ui_web::dedup_param_changes`, so DOM updates don't thrash under a
// preset-load fan-out or automation burst.
export function dedupParamChanged(events) {
  const latestForId = new Map();
  for (let i = 0; i < events.length; i++) {
    const e = events[i];
    if (e && e.kind === "param_changed") latestForId.set(e.id, i);
  }
  const out = [];
  for (let i = 0; i < events.length; i++) {
    const e = events[i];
    if (!e) continue;
    if (e.kind === "param_changed" && latestForId.get(e.id) !== i) continue;
    out.push(e);
  }
  return out;
}

// ---- FaceplateBridge --------------------------------------------------------

export class FaceplateBridge {
  // Options:
  //   controller : a WebController (controller.mjs) — already instantiated, its
  //                `store` the SHARED WebHost param SAB.
  //   dispatch   : function(arr) called with the translated+deduped ViewEvent
  //                batch each tick (the browser passes `window.__vxn.applyViewEvents`).
  //   onTextInput: function({id,title,initial}) for `request_text_input` — the
  //                DOM popup (0061). Optional; default no-op.
  //   scheduleFrame / cancelFrame : rAF seam (default the browser globals; the
  //                Node test injects a manual pump).
  constructor({
    controller,
    dispatch = () => {},
    onTextInput = () => {},
    scheduleFrame = (cb) =>
      typeof requestAnimationFrame === "function"
        ? requestAnimationFrame(cb)
        : setTimeout(() => cb(performance.now ? performance.now() : Date.now()), 16),
    cancelFrame = (h) =>
      typeof cancelAnimationFrame === "function" ? cancelAnimationFrame(h) : clearTimeout(h),
  } = {}) {
    if (!controller) throw new Error("FaceplateBridge needs a controller");
    this.controller = controller;
    this.dispatch = dispatch;
    this.onTextInput = onTextInput;
    this._scheduleFrame = scheduleFrame;
    this._cancelFrame = cancelFrame;

    this._dirty = false; // a UiEvent arrived since the last tick
    this._frame = null; // pending rAF handle
    this._running = false;
  }

  // ---- JS -> controller (0058) ---------------------------------------------
  //
  // Route one opcode JSON string (the exact `{op,..}` bridge.js `_post` builds)
  // to the controller. Marks the bridge dirty so the next tick mutates the model
  // + mirrors the store SAB; ordering within a tick is preserved because the
  // controller drains its bounded queue FIFO (begin/set/end stays bracketed).
  handleUiOpcode(json) {
    let msg;
    try {
      msg = typeof json === "string" ? JSON.parse(json) : json;
    } catch {
      return; // malformed — drop, same as the native parser's None
    }
    if (!msg || typeof msg.op !== "string") return;
    const c = this.controller;
    switch (msg.op) {
      // --- hot path (1:1 with UiEvent) ---
      case "set_param":
        c.setParam(msg.id >>> 0, msg.plain);
        break;
      case "set_param_norm":
        c.setParamNorm(msg.id >>> 0, msg.norm);
        break;
      case "begin_gesture":
        c.beginGesture(msg.id >>> 0);
        break;
      case "end_gesture":
        c.endGesture(msg.id >>> 0);
        break;
      case "ready":
        // Re-broadcast every param + view state so the freshly-mounted page is
        // seeded (the native EditorReady path).
        c.editorReady();
        break;
      // --- per-synth custom (non-param shared state) ---
      case "set_key_mode":
        c.setKeyMode(msg.mode >>> 0);
        break;
      case "set_split_point":
        c.setSplitPoint(msg.note >>> 0);
        break;
      case "set_edit_layer":
        c.setEditLayer(layerCode(msg.layer));
        break;
      case "reset_layer":
        c.resetLayer(layerCode(msg.layer));
        break;
      // --- text input (0061): handled in JS, NOT forwarded ---
      case "request_text_input":
        this.onTextInput({ id: msg.id, title: msg.title || "", initial: msg.initial || "" });
        return; // no controller tick needed
      // --- preset / folder ops (E019): accepted, inert under the NullStore ---
      // The controller has no opcode surface for these yet (preset storage is
      // E019); swallow them so the page's buttons don't throw. When E019 wires
      // an IndexedDB store + controller opcodes, route them here.
      case "load_factory":
      case "load_user":
      case "save_preset":
      case "rename_preset":
      case "delete_preset":
      case "move_preset":
      case "rename_folder":
      case "delete_folder":
      case "new_folder":
      case "step_preset":
        return; // inert for now
      default:
        return; // unknown opcode — drop (matches native parse returning None)
    }
    this._dirty = true;
    this._scheduleTick();
  }

  // ---- controller -> JS (0059) ---------------------------------------------
  //
  // Drain the controller's ViewEvents (its OWN model mutations), translate +
  // dedupe, and hand the batch to the page dispatcher. Returns the dispatched
  // array (the Node test asserts on it).
  tick() {
    this._dirty = false;
    const raw = this.controller.tick(); // mutates model, mirrors store SAB
    if (!raw || raw.length === 0) return [];
    const translated = [];
    for (const ev of raw) {
      const fe = viewEventToFaceplate(ev);
      if (fe) translated.push(fe);
    }
    const batch = dedupParamChanged(translated);
    if (batch.length) this.dispatch(batch);
    return batch;
  }

  // ---- ~60 Hz coalescing loop ----------------------------------------------
  //
  // Coalesce: many opcodes in one frame collapse to a SINGLE controller tick +
  // a single dispatch batch, so a fast drag / automation sweep can't thrash the
  // DOM. A free-running rAF loop also keeps ticking so a `ready` re-broadcast or
  // any late ViewEvent still flushes even if no fresh opcode arrived.
  start() {
    if (this._running) return this;
    this._running = true;
    const pump = () => {
      if (!this._running) return;
      this.tick();
      this._frame = this._scheduleFrame(pump);
    };
    this._frame = this._scheduleFrame(pump);
    return this;
  }

  stop() {
    this._running = false;
    if (this._frame != null) {
      this._cancelFrame(this._frame);
      this._frame = null;
    }
  }

  // Schedule a one-shot tick for the next frame when not free-running (the Node
  // test drives `tick()` directly; the browser uses `start()`'s loop). No-op
  // while the loop is running (it already ticks every frame).
  _scheduleTick() {
    if (this._running) return;
    if (this._frame != null) return;
    this._frame = this._scheduleFrame(() => {
      this._frame = null;
      this.tick();
    });
  }
}

// ---- browser boot -----------------------------------------------------------
//
// Wire the live page: boot the audio coordinator (WebHost) + the controller
// (WebController) over the SAME param store SAB, install the `window.ipc`
// router (draining the queue the inline boot-head stub buffered), and feed the
// page dispatcher. Skipped under Node (no document) so the test imports the pure
// FaceplateBridge without side effects.
//
// E017 input (Web MIDI + computer keyboard) is wired below, right after `host`
// boots: both attach to the WebHost producer surface and write into the E015
// ring. Dynamic-imported so the headless import guard above keeps this file
// pure under Node.
export async function bootFaceplate({ WebHostClass } = {}) {
  if (typeof document === "undefined") return null; // headless import guard

  const { WebHost } = WebHostClass
    ? { WebHost: WebHostClass }
    : await import("./coordinator.mjs");

  // The page dispatcher: `init()` (faceplate dispatch.js) swaps
  // `window.__vxn.applyViewEvents` for the real fan-out; before that bridge.js
  // buffers. We always call through `window.__vxn.applyViewEvents` so whichever
  // is current receives the batch.
  const dispatch = (arr) => {
    try {
      window.__vxn.applyViewEvents(arr);
    } catch (e) {
      console.warn("vxn: applyViewEvents threw", e);
    }
  };

  // 1. Audio coordinator owns the shared SABs (ring + param store). Construct it
  //    now; `start()` must run from a user gesture (autoplay), so the page's
  //    Start affordance drives it — but the controller + bridge can run before
  //    audio is live (param edits just land in the store, applied on first
  //    sound).
  const host = new WebHost({});

  // 1b. E017 input adapters → the WebHost producer surface (ring). Web MIDI +
  //     computer keyboard both write notes/CC into the same E015 ring the
  //     worklet drains; events written before audio is live buffer in the ring
  //     and apply on first sound. Dynamic-imported so the headless test (which
  //     returns at the document guard above) never pulls the browser-only
  //     adapters. MIDI is best-effort: no device / denied permission is fine,
  //     the keyboard is the fallback.
  let input = { midi: null, keyboard: null };
  try {
    const [{ attachKeyboard }, { attachMidi }] = await Promise.all([
      import("./keyboard-input.mjs"),
      import("./midi-input.mjs"),
    ]);
    input.keyboard = attachKeyboard(host, {});
    input.midi = await attachMidi(host, {
      onError: (e) => console.info("vxn: Web MIDI unavailable", e && e.message),
    });
  } catch (e) {
    console.warn("vxn: input adapters failed to attach", e);
  }

  // 2. Controller over the SAME param store SAB the worklet folds. Its
  //    ViewEvents feed the page; its model mirror writes the store.
  const controller = new WebController({
    store: host.store,
    onViewEvents: () => {}, // the bridge drains via tick(); no extra sink
  });
  await controller.instantiate();

  // 3. The bridge: opcodes in, ViewEvents out, ~60 Hz coalescing.
  const bridge = new FaceplateBridge({
    controller,
    dispatch,
    onTextInput: (req) => openTextInputPopup(req),
  });
  bridge.start();

  // 4. Replace the queuing `window.ipc` stub with the live router and flush
  //    whatever the faceplate buffered during parse (its `init()` `ready`).
  const queued = (window.__VXN_UI_QUEUE__ ||= []);
  window.ipc = { postMessage: (json) => bridge.handleUiOpcode(json) };
  for (const json of queued.splice(0)) bridge.handleUiOpcode(json);

  // 5. Autoplay unlock. The browser starts the AudioContext suspended and only
  //    lets it resume inside a user-gesture call stack. The faceplate has no
  //    dedicated "Start" button (it IS the synth UI), so unlock on the first
  //    interaction anywhere on the page. Param edits before this still land in
  //    the store (applied on first sound); this just brings audio live. Guarded
  //    to run host.start() exactly once.
  let audioStarted = false;
  const unlock = async () => {
    if (audioStarted) return;
    audioStarted = true;
    window.removeEventListener("pointerdown", unlock, true);
    window.removeEventListener("keydown", unlock, true);
    try {
      await host.start();
      // `host.start()` seeds the store with engine defaults (writeBulk), which
      // can clobber values the controller already mirrored (an edit on this very
      // gesture). Re-mirror the controller's authoritative table on the next
      // tick so the store reflects the controller, not the defaults.
      controller.remirrorStore();
    } catch (e) {
      console.warn("vxn: audio start failed", e);
      audioStarted = false; // allow a retry on the next gesture
    }
  };
  window.addEventListener("pointerdown", unlock, true);
  window.addEventListener("keydown", unlock, true);

  // Expose for the page's Start button + E017 input adapters.
  window.__vxnWeb = { host, controller, bridge, start: unlock, input };
  return window.__vxnWeb;
}

// ---- DOM text-input popup (0061) -------------------------------------------
//
// The desktop floating NSWindow/HWND input is gone on the web; replace it with a
// plain DOM modal. On commit/cancel deliver `text_input_result` straight to the
// page (`window.vxn.onViewEvent`) so bridge.js's one-shot promptText callback
// fires — `value` is the string on Enter, null on Esc / click-outside, matching
// the plugin contract.
export function openTextInputPopup({ id, title, initial }, doc = globalThis.document) {
  if (!doc) return;
  const deliver = (value) => {
    try {
      window.vxn.onViewEvent({ kind: "text_input_result", id, value });
    } catch (e) {
      console.warn("vxn: text_input_result delivery threw", e);
    }
  };

  const backdrop = doc.createElement("div");
  backdrop.className = "vxn-ti-backdrop";
  const box = doc.createElement("div");
  box.className = "vxn-ti-box";
  const label = doc.createElement("div");
  label.className = "vxn-ti-title";
  label.textContent = title || "";
  const input = doc.createElement("input");
  input.type = "text";
  input.className = "vxn-ti-input";
  input.value = initial || "";
  box.appendChild(label);
  box.appendChild(input);
  backdrop.appendChild(box);
  doc.body.appendChild(backdrop);

  let done = false;
  const close = (value) => {
    if (done) return;
    done = true;
    try {
      backdrop.remove();
    } catch {}
    deliver(value);
  };
  input.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter") {
      ev.preventDefault();
      close(input.value);
    } else if (ev.key === "Escape") {
      ev.preventDefault();
      close(null);
    }
  });
  backdrop.addEventListener("pointerdown", (ev) => {
    if (ev.target === backdrop) close(null); // click outside cancels
  });
  // Focus after mount so the input takes keystrokes immediately.
  try {
    input.focus();
    input.select();
  } catch {}
}

// Auto-boot in the browser (deferred module script — runs after the faceplate's
// synchronous init()). Headless imports skip this (no document).
if (typeof document !== "undefined" && document.getElementById("faceplate")) {
  bootFaceplate().catch((e) => console.error("vxn: faceplate boot failed", e));
}
