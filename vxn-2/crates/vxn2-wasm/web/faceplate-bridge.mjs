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
import { attachKeyboard } from "./keyboard-input.mjs";
import { attachMidi } from "./midi-input.mjs";

// Opcodes that don't touch the model / audio and can be ignored on the web path
// until browser persistence (0159) wires them. Kept explicit so an unhandled
// opcode is a loud console warning, not a silent drop.
const DEFERRED_OPS = new Set([
  "request_text_input",
  // User-preset management — deferred (minimal 0159 is factory-load only).
  "load_user",
  "save_preset",
  "delete_preset",
  "rename_preset",
  "move_preset",
  "new_folder",
  "rename_folder",
  "delete_folder",
]);

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
    case "set_matrix_row": {
      // Panel + native decoder both carry the fields nested under `row`
      // (vxn2-ui-web parse_custom_ui reads v.get("row")). Reading msg.source
      // et al. top-level yielded undefined → `x >>> 0` → 0, so any topology
      // edit wiped source/dest/active and killed the route.
      var row = msg.row || {};
      ctrl.setMatrixRow(msg.slot, row.source, row.dest, row.curve, row.active, row.depth);
      return true;
    }
    case "request_matrix_snapshot":
      ctrl.requestMatrixSnapshot();
      return true;
    case "request_ks_curve_snapshot":
      ctrl.requestKsCurveSnapshot();
      return true;
    case "request_eg_curve_snapshot":
      ctrl.requestEgCurveSnapshot();
      return true;
    case "load_factory":
      ctrl.loadFactory(msg.index);
      return true;
    case "step_preset":
      ctrl.stepPreset(msg.delta ?? (msg.dir === "next" ? 1 : -1));
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
    enableKeyboard = true,
    enableMidi = true,
    factoryUrl = "./factory.bin",
    fetchImpl = globalThis.fetch,
    showCpuMeter = true,
    showWelcome = true,
    showPianoKeyboard = true,
  } = {}) {
    this._doc = doc;
    this._win = win;
    this._factoryUrl = factoryUrl;
    this._fetch = fetchImpl ? fetchImpl.bind(globalThis) : null;
    this._raf = rafImpl || (win.requestAnimationFrame ? win.requestAnimationFrame.bind(win) : null);
    this._enableKeyboard = enableKeyboard;
    this._enableMidi = enableMidi;
    this._showWelcome = showWelcome;
    this._showPianoKeyboard = showPianoKeyboard;
    this._keyboard = null;
    this._midi = null;
    this._piano = null;

    // Render-load meter (bottom-left badge): the worklet posts its per-quantum
    // DSP load up the port, WebHost forwards it via onCpu → the meter. A no-op in
    // headless tests (no document). An explicit hostOptions.onCpu still wins.
    this._cpuMeter = showCpuMeter ? createCpuMeter(doc) : { update() {}, el: null };
    this.host = new WebHost({
      wasmUrl,
      wasmBytes,
      onCpu: (load, peak) => this._cpuMeter.update(load, peak),
      ...hostOptions,
    });
    this.controller = new WebController({
      wasmUrl: controllerWasmUrl,
      wasmBytes: controllerBytes,
      store: this.host.store, // controller mirrors its model into the worklet store
      ring: this.host.ring, // matrix topology rides the ring (no CLAP id — 0193)
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
    // Mount the web-only chrome (welcome card + on-screen piano) up front —
    // synchronously, before the controller-instantiate await — so it shows
    // immediately and doesn't hinge on a successful async boot. `this.host`
    // already exists from the constructor, so the piano can push notes into the
    // ring (they buffer until audio is live).
    this._mountWebChrome();
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
    // Load the factory bank (minimal 0159) before flushing queued opcodes, so a
    // page that requests a preset during boot finds the bank populated. Non-fatal
    // — the instrument plays without presets.
    await this._loadFactory();
    // Flush any opcodes the faceplate posted while we were instantiating.
    for (const msg of this._queue) routeOpcode(this.controller, msg);
    this._queue.length = 0;
    this._startPump();
    this._attachInputs();
    return this;
  }

  // Fetch `factory.bin`, parse it into the controller's factory bank, and hand
  // the resulting corpus to the faceplate's preset browser.
  async _loadFactory() {
    if (!this._fetch) return;
    try {
      const resp = await this._fetch(this._factoryUrl);
      if (!resp.ok) return;
      const bytes = new Uint8Array(await resp.arrayBuffer());
      this.controller.loadFactoryAsset(bytes);
      const corpus = this.controller.corpusJson();
      const apply = this._win.__vxn && this._win.__vxn.applyPresetCorpus;
      if (typeof apply === "function") apply(corpus);
    } catch (e) {
      console.warn("vxn2 bridge: factory bank load failed (presets unavailable)", e);
    }
  }

  // Wire the browser input adapters (ticket 0160) onto the coordinator's producer
  // surface: computer keyboard now, Web MIDI async (resolves granted=false and
  // stays silent where Web MIDI is unavailable — the keyboard still plays). Notes
  // pushed before audio is live buffer in the ring and sound on the first quantum.
  // Web-only affordances (self-contained DOM, inline styles): the first-run
  // welcome card and the on-screen piano. Both no-op without a document body, so
  // headless tests are unaffected. The piano is a note producer onto the same
  // host surface the computer keyboard uses. Called synchronously from boot().
  _mountWebChrome() {
    if (this._showWelcome && this._doc && this._doc.body) {
      createWelcome(this._doc);
    }
    if (this._showPianoKeyboard && this._doc && this._doc.body) {
      this._piano = createPianoKeyboard(this._doc, this.host);
    }
  }

  _attachInputs() {
    if (this._enableKeyboard && this._doc) {
      this._keyboard = attachKeyboard(this.host, { target: this._doc });
    }
    if (this._enableMidi) {
      attachMidi(this.host)
        .then((m) => {
          this._midi = m;
        })
        .catch(() => {}); // graceful: no MIDI → keyboard-only, never throws
    }
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
    if (this._keyboard) this._keyboard.detach();
    if (this._midi) this._midi.detach();
    if (this._piano) this._piano.detach();
    this.controller.destroy();
    await this.host.teardown();
  }
}

// ---- CPU (render-load) meter -----------------------------------------------
//
// A small fixed badge at the bottom-left: a bar + percent showing the audio
// thread's per-quantum DSP load (1.0 == the whole deadline). Fed by the worklet's
// `cpu` port messages via WebHost.onCpu. Self-contained (inline styles, no
// external CSS) and created once per boot; idempotent if the element already
// exists (a re-boot reuses it). Ported verbatim from vxn-1.
export function createCpuMeter(doc = globalThis.document) {
  if (!doc || !doc.body) return { update() {}, el: null };
  const ID = "vxn-cpu-meter";
  let el = doc.getElementById(ID);
  if (!el) {
    el = doc.createElement("div");
    el.id = ID;
    el.style.cssText =
      // bottom offset clears the 92px on-screen piano bar (which is fixed to the
      // page bottom); harmless extra lift when the piano is disabled.
      "position:fixed;left:10px;bottom:102px;z-index:9999;display:flex;" +
      "align-items:center;gap:6px;font:11px/1 system-ui,sans-serif;" +
      "color:#cfd3d8;background:rgba(20,22,26,.78);padding:4px 7px;" +
      "border-radius:5px;user-select:none;pointer-events:none;";
    const label = doc.createElement("span");
    label.textContent = "CPU";
    label.style.cssText = "opacity:.7;letter-spacing:.04em;";
    const track = doc.createElement("span");
    track.style.cssText =
      "position:relative;width:54px;height:6px;border-radius:3px;" +
      "background:rgba(255,255,255,.14);overflow:hidden;";
    const fill = doc.createElement("span");
    fill.style.cssText =
      "position:absolute;left:0;top:0;bottom:0;width:0%;background:#46c46e;" +
      "transition:width .1s linear,background .2s linear;";
    track.appendChild(fill);
    const pct = doc.createElement("span");
    pct.textContent = "—";
    pct.style.cssText = "min-width:28px;text-align:right;font-variant-numeric:tabular-nums;";
    el.append(label, track, pct);
    doc.body.appendChild(el);
    el._fill = fill;
    el._pct = pct;
  }
  const update = (load, peak) => {
    const f = el._fill, p = el._pct;
    // null load == "no measurement" (meter disabled — e.g. Safari). Show n/a so a
    // missing reading is distinct from a real 0% (and from the initial "—").
    if (load == null) {
      f.style.width = "0%";
      p.textContent = "n/a";
      return;
    }
    // Bar tracks peak (the worklet posts mean + per-window peak; peak shows
    // transient spikes the mean smooths away).
    const pk = Math.max(0, Math.min(1.5, Math.max(load, peak || 0)));
    f.style.width = `${Math.min(100, pk * 100).toFixed(0)}%`;
    // green < 0.7, amber < 0.9, red beyond — the usual xrun-headroom bands.
    f.style.background = pk < 0.7 ? "#46c46e" : pk < 0.9 ? "#e0b341" : "#e0564b";
    // One decimal under 10% so a live-but-light load stays legible despite the
    // 1% quantization floor.
    const a = Math.max(0, load) * 100;
    p.textContent = a < 10 ? `${a.toFixed(1)}%` : `${Math.round(a)}%`;
  };
  return { update, el };
}

// ---- welcome card ----------------------------------------------------------
//
// A centred modal shown on load: a one-line "what is this", a "how to play"
// note, a link to the VXN-2 product page (new tab), and a Close button. There is
// no manual yet, so the link goes to the product page. Self-contained (inline
// styles, no external CSS); idempotent if it already exists. Returns
// { el, close }. Web-only — the bridge is the sole caller and never runs in the
// native plugin. Ported from vxn-1's createWelcome.
const PRODUCT_URL = "https://vulpuslabs.com/products/vxn-2/";

// True on Apple WebKit (Safari), where the AudioWorklet is glitch-prone — used
// only to show a low-key heads-up in the welcome card. Kept local so this module
// has no cross-import for one predicate.
function isAppleWebKitUA(doc = globalThis.document) {
  const nav = (doc && doc.defaultView && doc.defaultView.navigator) || globalThis.navigator;
  if (!nav) return false;
  const ua = nav.userAgent || "";
  const vendor = nav.vendor || "";
  return /Apple/.test(vendor) && !/CriOS|FxiOS|EdgiOS|Chrome|Chromium|Edg|Android/.test(ua);
}

export function createWelcome(doc = globalThis.document) {
  if (!doc || !doc.body) return { el: null, close() {} };
  const ID = "vxn-welcome";
  if (doc.getElementById(ID)) return { el: doc.getElementById(ID), close() {} };

  const backdrop = doc.createElement("div");
  backdrop.id = ID;
  backdrop.style.cssText =
    "position:fixed;inset:0;z-index:10000;display:flex;align-items:center;" +
    "justify-content:center;background:rgba(8,9,11,.62);" +
    "font:14px/1.5 system-ui,sans-serif;color:#e6e9ee;";

  const card = doc.createElement("div");
  card.style.cssText =
    "max-width:30rem;margin:1rem;padding:22px 24px;border-radius:10px;" +
    "background:#1b1e24;border:1px solid rgba(255,255,255,.10);" +
    "box-shadow:0 14px 48px rgba(0,0,0,.55);";

  const h = doc.createElement("h2");
  h.textContent = "VXN-2";
  h.style.cssText = "margin:0 0 .6rem;font-size:1.35rem;letter-spacing:.04em;";

  const intro = doc.createElement("p");
  intro.style.cssText = "margin:0 0 .8rem;";
  intro.textContent =
    "A WebAssembly port of the VXN-2 FM synthesizer by Vulpus Labs.";

  const how = doc.createElement("p");
  how.style.cssText = "margin:0 0 .8rem;";
  how.textContent =
    "Play it with the on-screen keyboard, your computer keyboard, or a connected MIDI device.";

  const product = doc.createElement("p");
  product.style.cssText = "margin:0 0 1.2rem;";
  const link = doc.createElement("a");
  link.href = PRODUCT_URL;
  link.target = "_blank";
  link.rel = "noopener noreferrer"; // opens the product page in a new tab, safely
  link.textContent = "Learn more about VXN-2";
  link.style.cssText = "color:#6db7ff;text-decoration:underline;";
  product.append(link, doc.createTextNode(" (opens in a new tab)."));

  // Safari-only heads-up: its AudioWorklet runs with a single render quantum of
  // output buffer and ignores latencyHint, so its realtime audio thread is prone
  // to occasional dropouts no matter how cheap our render is. Chrome/Edge don't
  // have this. Keep it low-key and only show it where it applies.
  let note = null;
  if (isAppleWebKitUA(doc)) {
    note = doc.createElement("p");
    note.style.cssText =
      "margin:0 0 1.2rem;padding:8px 10px;border-radius:6px;font-size:12px;" +
      "background:rgba(224,179,65,.12);color:#e0b341;";
    note.textContent =
      "For the smoothest audio, use Chrome or Edge — Safari may produce occasional clicks.";
  }

  const close = doc.createElement("button");
  close.type = "button";
  close.textContent = "Close";
  close.style.cssText =
    "display:block;margin-left:auto;padding:7px 18px;border:0;border-radius:6px;" +
    "background:#46c46e;color:#0c0e10;font:600 14px system-ui,sans-serif;cursor:pointer;";

  const dismiss = () => backdrop.remove();
  close.addEventListener("click", dismiss);
  // Click outside the card or press Escape also dismisses.
  backdrop.addEventListener("click", (e) => { if (e.target === backdrop) dismiss(); });
  const onKey = (e) => {
    if (e.key === "Escape") { dismiss(); doc.removeEventListener("keydown", onKey); }
  };
  doc.addEventListener("keydown", onKey);

  card.append(h, intro, how, product, ...(note ? [note] : []), close);
  backdrop.appendChild(card);
  doc.body.appendChild(backdrop);
  return { el: backdrop, close: dismiss };
}

// ---- on-screen piano keyboard ----------------------------------------------
//
// A clickable piano along the bottom of the page: white keys in a flex row with
// black keys absolutely positioned overlapping them. Purely a producer for the
// E015 ring — mouse/touch on a key calls host.noteOn / host.noteOff, the same
// surface computer-keyboard and MIDI input use, so the ring stays
// source-agnostic. Self-contained inline styles; idempotent per boot.
//
// Interaction: press a key → note-on + highlight; release (or pointer leaving
// the keyboard) → note-off. Holding the button and dragging across keys plays a
// glissando (each newly-entered key releases the previous note and sounds its
// own). Returns { el, detach, allNotesOff }.

// MIDI note -> true if it's a black (accidental) key. Pattern within an octave:
// C# D# _ F# G# A# are the five black keys (pitch classes 1,3,6,8,10).
export function isBlackKey(note) {
  const pc = ((note % 12) + 12) % 12;
  return pc === 1 || pc === 3 || pc === 6 || pc === 8 || pc === 10;
}

// Build the key layout for the inclusive MIDI range [startNote, endNote]:
// an ordered list of { note, black }. White keys lay out left-to-right; each
// black key floats between its neighbouring whites.
export function pianoLayout(startNote, endNote) {
  const keys = [];
  for (let n = startNote; n <= endNote; n++) keys.push({ note: n, black: isBlackKey(n) });
  return keys;
}

const PIANO_DEFAULT_START = 48; // C3
const PIANO_DEFAULT_END = 84; // C6 (inclusive) — three octaves
const PIANO_VELOCITY = 0.8; // no pressure sensing on click; match keyboard-input

export function createPianoKeyboard(doc = globalThis.document, host = null, opts = {}) {
  if (!doc || !doc.body) return { el: null, detach() {}, allNotesOff() {} };
  const ID = "vxn-piano";
  if (doc.getElementById(ID)) return { el: doc.getElementById(ID), detach() {}, allNotesOff() {} };

  const startNote = opts.startNote != null ? opts.startNote : PIANO_DEFAULT_START;
  const endNote = opts.endNote != null ? opts.endNote : PIANO_DEFAULT_END;
  const velocity = opts.velocity != null ? opts.velocity : PIANO_VELOCITY;
  const layout = pianoLayout(startNote, endNote);
  const whites = layout.filter((k) => !k.black);

  const bar = doc.createElement("div");
  bar.id = ID;
  bar.style.cssText =
    "position:fixed;left:0;right:0;bottom:0;z-index:9998;height:92px;" +
    "display:flex;background:#14161a;border-top:1px solid rgba(255,255,255,.10);" +
    "box-shadow:0 -6px 20px rgba(0,0,0,.4);user-select:none;touch-action:none;" +
    "-webkit-user-select:none;";

  // A relative container the whites flex inside and the blacks absolutely sit on.
  const bed = doc.createElement("div");
  bed.style.cssText = "position:relative;display:flex;flex:1;height:100%;";
  bar.appendChild(bed);

  // note -> key element, so glissando and note-off can toggle the highlight.
  const keyEls = new Map();
  const whiteW = 100 / whites.length; // percent width per white key

  function styleWhite(el, active) {
    el.style.background = active ? "#8fd0ff" : "#f4f4f2";
  }
  function styleBlack(el, active) {
    el.style.background = active ? "#4c8fbf" : "#1b1d21";
  }

  // Lay out white keys first (flex children), then overlay black keys.
  let whiteIndex = 0;
  for (const k of layout) {
    if (k.black) continue;
    const el = doc.createElement("div");
    el.className = "vxn-piano-white";
    el.dataset.note = String(k.note);
    el.style.cssText =
      "flex:1;height:100%;border-right:1px solid rgba(0,0,0,.28);" +
      "border-radius:0 0 3px 3px;box-sizing:border-box;";
    styleWhite(el, false);
    bed.appendChild(el);
    keyEls.set(k.note, el);
    whiteIndex++;
  }

  // Overlay black keys. A black key at MIDI note n sits over the boundary between
  // the white below it (n-1) and the white above (n+1); centre it on that seam.
  for (const k of layout) {
    if (!k.black) continue;
    const whitesBelow = whites.filter((w) => w.note < k.note).length; // seam index
    const el = doc.createElement("div");
    el.className = "vxn-piano-black";
    el.dataset.note = String(k.note);
    const centre = whitesBelow * whiteW; // seam position, in percent
    el.style.cssText =
      "position:absolute;top:0;height:62%;width:" + (whiteW * 0.62).toFixed(4) + "%;" +
      "left:" + centre.toFixed(4) + "%;transform:translateX(-50%);" +
      "border-radius:0 0 3px 3px;box-sizing:border-box;z-index:2;" +
      "box-shadow:0 2px 3px rgba(0,0,0,.5);";
    styleBlack(el, false);
    bed.appendChild(el);
    keyEls.set(k.note, el);
  }

  // ---- pointer -> note plumbing ----
  let pointerDown = false;
  let current = null; // the single note sounding from the mouse/touch drag

  function paint(note, active) {
    const el = keyEls.get(note);
    if (!el) return;
    if (isBlackKey(note)) styleBlack(el, active);
    else styleWhite(el, active);
  }

  function press(note) {
    if (note == null || note === current) return;
    if (current != null) release(); // monophonic drag: release the old note first
    current = note;
    paint(note, true);
    if (host && typeof host.noteOn === "function") host.noteOn(note, velocity, 0);
  }

  function release() {
    if (current == null) return;
    const note = current;
    current = null;
    paint(note, false);
    if (host && typeof host.noteOff === "function") host.noteOff(note, 0);
  }

  // Resolve the DOM target under a pointer to its MIDI note (data-note). Because
  // black keys sit above whites in z-order, elementFromPoint / event.target gives
  // the topmost key, which is what we want.
  function noteFromTarget(t) {
    if (!t || !t.dataset) return null;
    const n = t.dataset.note;
    return n == null ? null : parseInt(n, 10);
  }

  function onDown(e) {
    pointerDown = true;
    const note = noteFromTarget(e.target);
    if (note != null) {
      press(note);
      if (typeof e.preventDefault === "function") e.preventDefault();
    }
  }
  function onOver(e) {
    if (!pointerDown) return;
    const note = noteFromTarget(e.target);
    if (note != null) press(note);
  }
  function onUp() {
    pointerDown = false;
    release();
  }

  // Pointer events cover mouse + touch + pen uniformly. mouseover/enter on the
  // per-key elements bubbles to `bed`, so one delegated listener handles drag.
  bed.addEventListener("pointerdown", onDown);
  bed.addEventListener("pointerover", onOver);
  // Release anywhere (pointer may lift off the keyboard) — listen on the document.
  doc.addEventListener("pointerup", onUp);
  doc.addEventListener("pointercancel", onUp);

  function allNotesOff() {
    pointerDown = false;
    release();
  }

  doc.body.appendChild(bar);

  return {
    el: bar,
    allNotesOff,
    detach() {
      allNotesOff();
      bed.removeEventListener("pointerdown", onDown);
      bed.removeEventListener("pointerover", onOver);
      doc.removeEventListener("pointerup", onUp);
      doc.removeEventListener("pointercancel", onUp);
      if (bar.remove) bar.remove();
    },
    // Exposed for tests / drivers that synthesise pointer events.
    _press: press,
    _release: release,
  };
}

// Convenience boot used by the generated index.html: fetch both wasm modules
// (the coordinator/controller default URLs), then boot the bridge.
export async function bootFaceplate(opts = {}) {
  const bridge = new FaceplateBridge(opts);
  await bridge.boot();
  return bridge;
}
