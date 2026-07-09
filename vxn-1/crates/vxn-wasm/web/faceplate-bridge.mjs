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
import { PresetPersistence } from "./preset-persistence.mjs";
import { StateAutosave } from "./state-autosave.mjs";
import {
  exportPatchFile,
  importPatchFile,
  shareLinkFor,
  applyShareLinkOnBoot,
} from "./patch-io.mjs";

// Faceplate event kinds that change the persistable patch state (params + key
// mode + split point) — the trigger for a full-state autosave (E019 / 0065). An
// edit_layer_changed is pure view state and is deliberately excluded.
const PATCH_STATE_KINDS = new Set([
  "param_changed",
  "key_mode_changed",
  "split_point_changed",
  "preset_loaded",
]);

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
    case "PresetLoaded":
      // Binds the preset bar name + browser-panel highlight (E019 / 0062).
      // `source` already carries the {kind, index|path} shape the dispatcher's
      // `preset_loaded` handler reads (byte-identical to `preset_source_json`).
      return {
        kind: "preset_loaded",
        name: ev.name,
        source: ev.source,
        warnings: ev.warnings || [],
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
    onCorpusChanged = () => {},
    onPatchChanged = () => {},
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
    // Called (with the optional follow path) when a tick changed the user-preset
    // corpus — the boot wiring re-pushes `applyPresetCorpus` + flushes the write
    // journal to IndexedDB (E019 / 0064). Not part of the dispatch batch: the
    // corpus rides its own side channel (controller.corpusJson()), not applyViewEvents.
    this.onCorpusChanged = onCorpusChanged;
    // Called when a tick produced a change to the persistable patch state (param
    // / key mode / split / preset load) — the boot wiring debounces a full-state
    // autosave off this (E019 / 0065). Not the corpus channel: this is the live
    // patch ("last session"), not the named-preset corpus.
    this.onPatchChanged = onPatchChanged;
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
      // --- factory presets (E019 / 0062): wired ---
      case "load_factory":
        c.loadFactory(msg.index >>> 0);
        break;
      // --- user preset / folder ops (E019 / 0064): wired ---
      // The controller mutates the in-memory cache + journals the persistence
      // op on the next tick; the corpus-changed event then re-pushes the corpus
      // and flushes the journal to IndexedDB (onCorpusChanged). A null/absent
      // folder means the user-dir root.
      case "load_user":
        c.loadUser(msg.path);
        break;
      case "save_preset":
        c.savePreset(msg.name, msg.folder ?? null);
        break;
      case "rename_preset":
        c.renamePreset(msg.path, msg.new_name);
        break;
      case "delete_preset":
        c.deletePreset(msg.path);
        break;
      case "move_preset":
        c.movePreset(msg.path, msg.dest_folder ?? null);
        break;
      case "rename_folder":
        c.renameFolder(msg.old_name, msg.new_name);
        break;
      case "delete_folder":
        c.deleteFolder(msg.name);
        break;
      case "new_folder":
        c.newFolder(msg.suggested);
        break;
      case "step_preset":
        c.stepPreset(msg.delta | 0);
        break;
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
      // Corpus changes ride their own side channel (re-push applyPresetCorpus +
      // flush the journal); they're not part of the applyViewEvents dispatch.
      if (ev.type === "PresetCorpusChanged") {
        this.onCorpusChanged(ev.follow);
        continue;
      }
      const fe = viewEventToFaceplate(ev);
      if (fe) translated.push(fe);
    }
    const batch = dedupParamChanged(translated);
    if (batch.length) this.dispatch(batch);
    // Full-state autosave trigger (0065): if anything changed the persistable
    // patch state this tick, debounce a session-blob write. Keyed off the
    // emitted ViewEvents (the controller's own model mutations), so corpus-only
    // ops (which ride their own channel) and pure view-state changes don't fire.
    if (batch.some((e) => PATCH_STATE_KINDS.has(e.kind))) this.onPatchChanged();
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
    // Forward non-automatable key mode / split point to the audio worklet.
    // The controller owns them for state + UI echo, but the renderer only
    // learns them through WebHost's out-of-band port hop — they never ride the
    // param-store SAB. Driving off the ViewEvent batch reaches every source
    // (mode button, preset load, state restore, share link), not just direct
    // button clicks. Without this the worklet stays in its Whole default and
    // Split/Dual silently render both layers from the Upper block.
    for (const ev of arr) {
      if (ev.kind === "key_mode_changed") host.setKeyMode(ev.mode);
      else if (ev.kind === "split_point_changed") host.setSplitPoint(ev.note);
    }
    try {
      window.__vxn.applyViewEvents(arr);
    } catch (e) {
      console.warn("vxn: applyViewEvents threw", e);
    }
  };

  // Push the current factory+user corpus to the preset browser (the web analogue
  // of the native `applyPresetCorpus`). Called after the factory loads, after
  // boot hydration, and on every corpus-changing op (save/rename/delete/move/
  // folder). Best-effort: the panel may not be mounted yet.
  const publishCorpus = () => {
    try {
      if (window.__vxn && window.__vxn.applyPresetCorpus) {
        window.__vxn.applyPresetCorpus(controller.corpusJson());
      }
    } catch (e) {
      console.warn("vxn: applyPresetCorpus threw", e);
    }
  };

  // 1. Audio coordinator owns the shared SABs (ring + param store). Construct it
  //    now; `start()` must run from a user gesture (autoplay), so the page's
  //    Start affordance drives it — but the controller + bridge can run before
  //    audio is live (param edits just land in the store, applied on first
  //    sound).
  // A little render-load meter, bottom-left below the faceplate. The worklet
  // posts its per-quantum DSP load up the port; WebHost forwards it via onCpu.
  const cpuMeter = createCpuMeter(document);
  const host = new WebHost({
    onCpu: (load, peak) => cpuMeter.update(load, peak),
  });

  // First-run welcome card: what this is + how to play + a link to the manual.
  // Web-only (this module never loads in the native plugin); dismissed by its
  // Close button. Self-contained, same inline-style pattern as the CPU meter.
  createWelcome(document);

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

  // 2a. Factory bank (E019 / 0062): fetch the build-time baked asset, parse it
  //     into the controller's read-only factory store, and publish the corpus to
  //     the preset browser (the web analogue of the native `applyPresetCorpus`
  //     push). Best-effort: a missing/malformed asset just leaves the factory
  //     list empty — the synth still plays.
  try {
    const resp = await fetch("./factory.bin");
    if (resp.ok) {
      const count = controller.loadFactoryAsset(new Uint8Array(await resp.arrayBuffer()));
      if (count > 0) publishCorpus();
    } else {
      console.warn("vxn: factory.bin fetch failed", resp.status);
    }
  } catch (e) {
    console.warn("vxn: factory bank load failed", e);
  }

  // 2b. User presets (E019 / 0064): bridge async IndexedDB to the sync
  //     controller. Hydrate the in-memory cache from storage BEFORE the
  //     controller goes live so list/load serve synchronously; then re-push the
  //     corpus (now factory + user) and arm the flush-on-hide backstop. Writes
  //     go through `persistence.flush()` off the tick (onCorpusChanged below).
  //     Best-effort: storage unavailable (private mode) just means no persistence.
  const persistence = new PresetPersistence({ controller });
  try {
    await persistence.hydrate();
    publishCorpus();
    persistence.attachFlushOnHide();
  } catch (e) {
    console.warn("vxn: preset persistence init failed", e);
  }

  // 2c. Full patch-state autosave/restore (E019 / 0065): the host-state-blob
  //     analogue. Restore the saved "last session" patch into the model BEFORE
  //     the faceplate's `ready`→EditorReady re-broadcast (step 4), so the
  //     broadcast seeds the UI + param SAB with the restored values (params +
  //     key mode + split point ride the one blob). Then arm the flush-on-hide
  //     backstop; the bridge debounces writes on every patch change (below).
  //     Best-effort: no saved blob / malformed / storage unavailable just boots
  //     to defaults.
  //     A `#patch=…` share-link (E019 / 0066) takes PRECEDENCE over the autosave
  //     restore: an explicit shared patch is the user's intent for this load. Both
  //     run before the `ready`→EditorReady flush (step 4) so the broadcast seeds
  //     the restored/imported values; the share-link decode also strips the
  //     fragment so a reload doesn't re-import it over later edits.
  const autosave = new StateAutosave({ controller });
  try {
    const fromShare = applyShareLinkOnBoot(controller);
    if (!fromShare) await autosave.restore();
    autosave.attachFlushOnHide();
  } catch (e) {
    console.warn("vxn: state autosave / share-link init failed", e);
  }

  // 3. The bridge: opcodes in, ViewEvents out, ~60 Hz coalescing. A corpus
  //    change (user save/rename/delete/move/folder op) re-pushes the corpus and
  //    flushes the write journal to IndexedDB off the tick. A patch-state change
  //    (param / key mode / split / preset load) debounces a full-state autosave.
  const bridge = new FaceplateBridge({
    controller,
    dispatch,
    onTextInput: (req) => openTextInputPopup(req),
    onCorpusChanged: () => {
      publishCorpus();
      persistence.flush();
    },
    onPatchChanged: () => autosave.schedule(),
  });
  bridge.start();

  // 3a. Patch export / import / share controls (E019 / 0066), web-only. Injected
  //     into the preset bar rather than the shared faceplate markup: the native
  //     plugin's HOST owns state save/load, so off-device sharing is a web-only
  //     concern — same reasoning as the CPU meter + welcome card living here in
  //     the web glue. Export downloads a desktop-compatible `.toml`; Import applies
  //     a picked file; Share copies a `#patch=` link to the clipboard.
  createPatchIoControls(document, {
    getName: () => {
      const el = document.getElementById("pbar-name");
      const n = el && el.textContent ? el.textContent.trim() : "";
      return n || "VXN1 Patch";
    },
    onExport: (name) => {
      exportPatchFile(controller, { name });
      flashPatchStatus(document, `Exported “${name}.toml”`);
    },
    onImport: () => {
      importPatchFile(controller, {
        onResult: ({ ok, name, error }) =>
          flashPatchStatus(document, ok ? `Imported ${name}` : `Import failed: ${error}`),
      });
    },
    onShare: async () => {
      const url = shareLinkFor(controller);
      if (!url) {
        flashPatchStatus(document, "Patch too large to share by link — use Export");
        return;
      }
      const copied = await copyToClipboard(url);
      flashPatchStatus(document, copied ? "Share link copied to clipboard" : "Share link ready");
      if (!copied) {
        try {
          window.prompt("Copy this share link:", url);
        } catch {
          /* ignore */
        }
      }
    },
  });

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
  window.__vxnWeb = { host, controller, bridge, persistence, autosave, start: unlock, input };
  return window.__vxnWeb;
}

// ---- patch export / import / share controls (E019 / 0066) ------------------
//
// Web-only preset-bar buttons for off-device patch sharing. Injected at boot into
// the existing `#preset-bar-slot` (the same bar that hosts Save / Save As /
// Browse) rather than baked into the shared faceplate markup — the native plugin
// has no use for file/URL sharing (its host owns state), so this stays in the web
// glue. Idempotent; a no-op if the preset bar isn't present. Reuses the bar's
// `.pbar-btn` styling so the new buttons match the existing ones.
export function createPatchIoControls(doc = globalThis.document, handlers = {}) {
  if (!doc) return { el: null };
  const slot = doc.getElementById("preset-bar-slot");
  if (!slot) return { el: null };
  if (doc.getElementById("vxn-patch-io")) return { el: doc.getElementById("vxn-patch-io") };

  const { getName = () => "VXN1 Patch", onExport = () => {}, onImport = () => {}, onShare = () => {} } =
    handlers;

  const mk = (text, title, onClick) => {
    const b = doc.createElement("button");
    b.type = "button";
    b.className = "pbar-btn";
    b.textContent = text;
    b.title = title;
    b.addEventListener("click", onClick);
    return b;
  };

  // Group wrapper so the three buttons can be found / removed as a unit.
  const group = doc.createElement("span");
  group.id = "vxn-patch-io";
  group.style.cssText = "display:contents"; // don't disturb the bar's flex layout
  group.append(
    mk("Export", "Download this patch as a .toml file", () => onExport(getName())),
    mk("Import", "Load a patch from a .toml file", () => onImport()),
    mk("Share", "Copy a shareable link to this patch", () => onShare()),
  );

  // Place them just before the spacer (so they sit with Save/Save As, left of the
  // status pill); fall back to appending if the bar's shape changes.
  const spacer = slot.querySelector(".pbar-spacer");
  if (spacer) slot.insertBefore(group, spacer);
  else slot.appendChild(group);
  return { el: group };
}

// Flash a transient message in the preset bar's status pill (E019 / 0066). Reuses
// the `#pbar-status` element + `.visible` class the faceplate's `statusPill` does,
// so import/export/share feedback looks like every other status flash.
export function flashPatchStatus(doc = globalThis.document, text = "") {
  if (!doc) return;
  const el = doc.getElementById("pbar-status");
  if (!el) return;
  el.textContent = text;
  el.classList.add("visible");
  if (el._vxnPatchTimer) clearTimeout(el._vxnPatchTimer);
  el._vxnPatchTimer = setTimeout(() => el.classList.remove("visible"), 3000);
}

// Copy `text` to the clipboard, resolving true on success. Uses the async
// Clipboard API where available (needs a user gesture + secure context, which the
// Share button click provides), false otherwise so the caller can fall back to a
// prompt.
export async function copyToClipboard(text) {
  try {
    if (globalThis.navigator && navigator.clipboard && navigator.clipboard.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    /* fall through to false */
  }
  return false;
}

// ---- CPU (render-load) meter -----------------------------------------------
//
// A small fixed badge at the bottom-left, below the faceplate: a bar + percent
// showing the audio thread's per-quantum DSP load (1.0 == the whole deadline).
// Fed by the worklet's `cpu` port messages via WebHost.onCpu. Self-contained
// (inline styles, no external CSS) and created once per boot; idempotent if the
// element already exists (e.g. a re-boot reuses it).
export function createCpuMeter(doc = globalThis.document) {
  if (!doc || !doc.body) return { update() {}, el: null };
  const ID = "vxn-cpu-meter";
  let el = doc.getElementById(ID);
  if (!el) {
    el = doc.createElement("div");
    el.id = ID;
    el.style.cssText =
      "position:fixed;left:10px;bottom:10px;z-index:9999;display:flex;" +
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
    // null load == "no measurement". Show n/a so a missing reading is distinct
    // from a real 0% (and from the initial "—", "wired up, no sample yet").
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
    // One decimal under 10% so a live-but-light load (e.g. 0.0% vs frozen "—")
    // is still legible despite the 1% quantization floor.
    const a = Math.max(0, load) * 100;
    p.textContent = a < 10 ? `${a.toFixed(1)}%` : `${Math.round(a)}%`;
  };
  return { update, el };
}

// ---- welcome card ----------------------------------------------------------
//
// A centred modal shown on load: a one-line "what is this", a "how to play"
// note, a link to the GitHub Pages manual (new tab), and a Close button.
// Self-contained (inline styles, no external CSS); idempotent if it already
// exists. Returns { el, close }. Web-only — bootFaceplate is the sole caller and
// never runs in the native plugin.
const MANUAL_URL = "https://vulpus-labs.github.io/vxn-1/";

// True on Apple WebKit (Safari), where the AudioWorklet is glitch-prone — used
// only to show a low-key heads-up in the welcome card. Mirrors coordinator.mjs's
// isAppleWebKit; kept local so this module has no cross-import for one predicate.
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
  h.textContent = "VXN-1";
  h.style.cssText = "margin:0 0 .6rem;font-size:1.35rem;letter-spacing:.04em;";

  const intro = doc.createElement("p");
  intro.style.cssText = "margin:0 0 .8rem;";
  intro.textContent =
    "A WebAssembly port of the VXN-1 analogue synthesizer by Vulpus Labs.";

  const how = doc.createElement("p");
  how.style.cssText = "margin:0 0 .8rem;";
  how.textContent =
    "Play it with your computer keyboard or a connected MIDI device.";

  const manual = doc.createElement("p");
  manual.style.cssText = "margin:0 0 1.2rem;";
  const link = doc.createElement("a");
  link.href = MANUAL_URL;
  link.target = "_blank";
  link.rel = "noopener noreferrer"; // opens the manual in a new tab, safely
  link.textContent = "Read the manual";
  link.style.cssText = "color:#6db7ff;text-decoration:underline;";
  manual.append(link, doc.createTextNode(" (opens in a new tab)."));

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

  card.append(h, intro, how, manual, ...(note ? [note] : []), close);
  backdrop.appendChild(card);
  doc.body.appendChild(backdrop);
  return { el: backdrop, close: dismiss };
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
