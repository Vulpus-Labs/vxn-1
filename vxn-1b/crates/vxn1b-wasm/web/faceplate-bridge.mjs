// Faceplate bridge (ticket 0291) — the browser's stand-in for wry.
//
// The page (`vxn1b_ui_web::build_web_faceplate_html`) speaks one narrow
// protocol to whatever hosts it, and does not know or care that it is in a tab:
//
//   out:  window.ipc.postMessage(JSON.stringify({op, …}))
//   in:   window.__vxn.applyViewEvents(arr)      once per tick
//         window.__vxn.applyPresetCorpus(snap)   on a corpus change
//
// Natively wry provides the first and `evaluate_script` the second. This module
// provides both: an `ipc` shim that routes each opcode to the controller wasm
// and a pump that turns controller ticks + telemetry frames into one
// `applyViewEvents` call per animation frame.
//
// # Where each opcode goes, and why
//
// The model and the engine are separate wasms with separate linear memories —
// natively one `SharedParams` is visible to both threads and this split does not
// exist at all. One rule decides where an opcode goes: **does the opcode have a presence in the model?**
//
//   - IT DOES → post it to the controller and nothing else. It reaches the
//     engine on the next pump, as a diff: param VALUES through the store SAB
//     mirror, non-param state (key mode, split point, LFO 2 link, matrix
//     topology) through the echo resend below. That covers params, gestures,
//     presets, folders and copy_layer.
//   - IT DOES NOT → push it straight onto the ring. Only the scope tap and the
//     tempo qualify: pure audio-thread view state with no CLAP id and no model
//     presence, so there is no echo to carry them and routing them through the
//     model would put view state into the patch.
//
// An earlier cut of this file ALSO pushed key/matrix ops onto the ring at route
// time, "because the engine needs them too". That double-pushed every UI
// topology edit — once from the router, once from the resend — and bought
// nothing: the extra push is one tick earlier, which is the same tick a param
// edit waits for anyway (params reach the SAB on the pump, not on the click).
// Gestures stop at the controller for a different reason: the engine treats
// them as a no-op ("controller / host-echo concern, they never reach
// rendering" — codec.rs), and there is no host here to bracket edits for.
//
// # The load / restore / copy problem
//
// A preset load, a state restore and `copy_layer` all rewrite matrix topology
// and the keyboard record inside the CONTROLLER, so the ring never hears about
// them and the engine would keep playing the old routing. Rather than
// enumerating those three causes, the pump uses the controller's own echoes as
// the trigger: 0290 emits a matrix / key record exactly when either changes,
// from any cause, so the bridge diffs each against what it last pushed and
// resends the fields that moved. Load, restore, copy and any future cause are
// covered by one mechanism.
//
// # Ordering (see also audio-host.mjs step 1 vs step 3)
//
// Slot DEPTH is a CLAP param (store SAB); slot TOPOLOGY is a ring event. A load
// moves both, and nothing makes the two writes atomic against a block boundary,
// so a block can see one and not the other.
//
// The worklet reads the store FIRST and the ring SECOND within one quantum
// (`audio-host.mjs` process(): applyStoreToEngine at (1), drainRawInto at (2)).
// Given that, pushing the ring BEFORE mirroring the store makes the harmful tear
// impossible: for a block to see new depths with old topology it would need the
// mirror (second) to land before the store read AND the ring push (first) to
// land after the ring read — i.e. the first write after the second. The only
// reachable tear is new-topology-with-old-depth, which is the right route at a
// stale amount rather than a stale route at a new amount.
//
// **If audio-host.mjs ever reads the ring before the store, this inverts and
// nothing here will fail — it will just occasionally click on preset load.**
// The two orderings are load-bearing as a pair.
//
// This is a small effect and not a browser-only one: the native plugin has the
// same window (`SharedParams::restore_from_bytes` writes params, then topology,
// then the reload flag, while the audio thread folds params every block), and
// the click-prone destinations are smoothed anyway (`mod_smoothing.rs`, 0208).
// Getting the order right is free, so it is done; nothing more elaborate is
// warranted.

import {
  LAYER_L1,
  LAYER_L2,
  MATRIX_FIELD_SOURCE,
  MATRIX_FIELD_DEST,
  MATRIX_FIELD_CURVE,
  MATRIX_FIELD_SCALE_SRC,
  MATRIX_SLOTS,
} from "./event-codec.mjs";

/// Scope-tap wire codes (match `vxn1b_engine::ScopeTap::code()`), keyed by the
/// strings the page sends.
const SCOPE_TAP = { off: 0, upper: 1, lower: 2 };

/// Layer names the page uses → wire layer index.
const LAYER = { upper: LAYER_L1, lower: LAYER_L2 };

/// Matrix field names the page uses → wire field index.
const MATRIX_FIELD = {
  source: MATRIX_FIELD_SOURCE,
  dest: MATRIX_FIELD_DEST,
  curve: MATRIX_FIELD_CURVE,
  scale: MATRIX_FIELD_SCALE_SRC,
};

/// Opcodes the page still posts that VXN1b has no handler for, native or web.
/// Dropped deliberately rather than silently: `reset_layer` is a live dead
/// button in the shipped plugin and `set_edit_layer` is handled in-page, both
/// fork artifacts from vxn-1 (see ticket 0307). Routing them to something
/// invented here would make the web build behave differently from the plugin.
const KNOWN_UNHANDLED = new Set(["reset_layer", "set_edit_layer"]);

/// Meter frame layout — `MeterTap` order, from vxn-core-utils::meter. The page
/// wants the named shape `vxn1b_ui_web::serialise_custom_payload` produces, so
/// the flat telemetry region is mapped here rather than in the page.
function meterEvent(frame) {
  return {
    kind: "meters",
    l1: [frame[0], frame[1]],
    l2: [frame[2], frame[3]],
    dynIn: [frame[4], frame[5]],
    dynOut: [frame[6], frame[7]],
    // One value, not a pair — the compressor's detector is stereo-linked.
    dynGr: frame[8],
    master: [frame[9], frame[10]],
  };
}

/// Scope frame → the page's shape. Rounded to 3 dp exactly as the native
/// serialiser does: the canvas is ~120 px tall, so finer is invisible, and a
/// 384-sample frame at 30 Hz is worth not tripling in length.
function scopeEvent(frame) {
  const s = new Array(frame.length);
  for (let i = 0; i < frame.length; i++) {
    s[i] = Math.round(Math.min(2, Math.max(-2, frame[i])) * 1000) / 1000;
  }
  return { kind: "scope", s };
}

/// Route one page opcode. Pure with respect to its arguments — no DOM, no
/// timers — so the destination table is testable with fakes. `coord` is used
/// for exactly one opcode (the scope tap); everything else with a model
/// presence reaches the engine through the pump's diffs.
///
/// Returns `true` if the opcode was handled. An unknown or non-string `op` is
/// dropped and returns `false` rather than being guessed at: VXN1b's page never
/// posts a numeric `op` (vxn-2's does, for its operator tab, which is why its
/// router has to sniff the type first).
export function routeOpcode(ctrl, coord, msg, hooks = {}) {
  if (!msg || typeof msg.op !== "string") return false;
  switch (msg.op) {
    // ---- controller only: params + gestures ------------------------------
    case "set_param":
      ctrl.setParam(msg.id, msg.plain);
      return true;
    case "set_param_norm":
      ctrl.setParamNorm(msg.id, msg.norm);
      return true;
    case "begin_gesture":
      ctrl.beginGesture(msg.id);
      return true;
    case "end_gesture":
      ctrl.endGesture(msg.id);
      return true;
    case "ready":
      ctrl.editorReady();
      return true;

    // ---- controller only: non-param state --------------------------------
    //
    // These have no CLAP id, so the store mirror cannot carry them — but they
    // DO live in the model, so the pump's echo resend puts them on the ring on
    // the next frame. Pushing here as well would double every edit.
    case "set_key_mode":
      ctrl.setKeyMode(msg.mode | 0);
      return true;
    case "set_split_point":
      ctrl.setSplitPoint(msg.note | 0);
      return true;
    case "set_lfo2_link":
      ctrl.setLfo2Link(!!msg.on);
      return true;
    case "set_matrix": {
      const layer = LAYER[msg.layer];
      const field = MATRIX_FIELD[msg.field];
      if (layer === undefined || field === undefined) return false;
      ctrl.setMatrix(layer, msg.slot | 0, field, msg.value | 0);
      return true;
    }

    // ---- controller only: bulk patch ops ---------------------------------
    case "copy_layer": {
      const from = LAYER[msg.from];
      const to = LAYER[msg.to];
      if (from === undefined || to === undefined) return false;
      // Params reach the engine through the mirror; topology through the echo
      // resend in the pump. Nothing to push here.
      ctrl.copyLayer(from, to);
      return true;
    }

    // ---- controller only: presets + folders ------------------------------
    case "load_factory":
      ctrl.loadFactory(msg.index | 0);
      return true;
    case "load_user":
      ctrl.loadUser(msg.path);
      return true;
    case "step_preset":
      ctrl.stepPreset(msg.delta | 0);
      return true;
    case "save_preset":
      ctrl.savePreset(msg.name, msg.folder ?? null);
      return true;
    case "rename_preset":
      ctrl.renamePreset(msg.path, msg.new_name);
      return true;
    case "delete_preset":
      ctrl.deletePreset(msg.path);
      return true;
    case "move_preset":
      ctrl.movePreset(msg.path, msg.dest_folder ?? null);
      return true;
    case "new_folder":
      ctrl.newFolder(msg.suggested);
      return true;
    case "rename_folder":
      ctrl.renameFolder(msg.old_name, msg.new_name);
      return true;
    case "delete_folder":
      ctrl.deleteFolder(msg.name);
      return true;

    // ---- ring only -------------------------------------------------------
    case "set_scope_source": {
      const tap = SCOPE_TAP[msg.source];
      if (tap === undefined) return false;
      if (coord) coord.setScopeTap(tap);
      return true;
    }

    // ---- neither: answered in-page ---------------------------------------
    case "request_text_input":
      // The native opcode exists because the plugin editor needs an NSWindow
      // outside the host's event monitor. A page can prompt itself, so this
      // never reaches the controller — whose OpenTextInput / TextInputResult
      // variants 0290 deliberately does not pack.
      if (hooks.promptText) hooks.promptText(msg.id, msg.title ?? "", msg.initial ?? "");
      return true;

    default:
      return false;
  }
}

/// Drives the controller wasm and the page: one pump per animation frame.
export class FaceplateBridge {
  constructor({
    controller,
    coordinator = null,
    win = globalThis,
    onJournal = null,
    raf = null,
  } = {}) {
    if (!controller) throw new Error("FaceplateBridge needs a controller");
    this.controller = controller;
    this.coordinator = coordinator;
    this.win = win;
    // Journal drain hook — 0293 wires it to IndexedDB. Until then the ops are
    // drained and dropped, so the wasm journal cannot grow unbounded.
    this.onJournal = onJournal;
    this._raf =
      raf || (win && win.requestAnimationFrame ? win.requestAnimationFrame.bind(win) : null);
    this._running = false;
    this._frame = 0;

    // What the RING was last told, so the echo-driven resend pushes only drift.
    // Null = "tell it everything next time", which is also the boot state.
    this._sentMatrix = null;
    this._sentKey = null;

    // In-page text input.
    this._prompt = null;
  }

  /// Install the `window.ipc` shim the page posts through. Replaces whatever is
  /// there — under wry the page gets a real one; in a tab, this is it.
  install() {
    const self = this;
    this.win.ipc = {
      postMessage(json) {
        let msg = null;
        try {
          msg = JSON.parse(json);
        } catch (e) {
          console.warn("vxn: unparseable opcode", e);
          return;
        }
        self.handle(msg);
      },
    };
    return this;
  }

  /// Route one already-parsed opcode. Split from the shim so tests drive it
  /// without a `window`.
  handle(msg) {
    const handled = routeOpcode(this.controller, this.coordinator, msg, {
      promptText: (id, title, initial) => this._promptText(id, title, initial),
    });
    if (!handled && msg && typeof msg.op === "string" && !KNOWN_UNHANDLED.has(msg.op)) {
      console.warn(`vxn: unrouted opcode "${msg.op}"`);
    }
    return handled;
  }

  /// The in-page answer to `request_text_input`. Synthesises the
  /// `text_input_result` the page's dispatcher already expects, so the
  /// promptText callback fires exactly as it does natively.
  _promptText(id, title, initial) {
    const value = this._prompt
      ? this._prompt(title, initial)
      : this.win.prompt
        ? this.win.prompt(title, initial)
        : null;
    this._deliver([{ kind: "text_input_result", id, value: value ?? null }]);
  }

  /// Override the prompt (tests, or a nicer in-page modal later).
  setPrompt(fn) {
    this._prompt = fn;
    return this;
  }

  /// Hand a batch to the page. Buffers safely before `init()` runs — the page's
  /// own `__vxn.applyViewEvents` stub queues until its dispatcher is bound.
  _deliver(events) {
    if (!events.length) return;
    const sink = this.win.__vxn;
    if (sink && typeof sink.applyViewEvents === "function") sink.applyViewEvents(events);
  }

  _deliverCorpus(snap) {
    const sink = this.win.__vxn;
    if (sink && typeof sink.applyPresetCorpus === "function") sink.applyPresetCorpus(snap);
  }

  /// Publish the corpus snapshot the controller holds. Available immediately —
  /// the factory bank is embedded (0290), not fetched.
  publishCorpus() {
    this._deliverCorpus(this.controller.corpusJson());
  }

  /// Push whatever moved in `slots` (a `matrix` view record) onto the ring, and
  /// remember it. Returns the number of ring events pushed.
  _resendMatrix(slots) {
    if (!this.coordinator) return 0;
    let pushed = 0;
    for (let layer = 0; layer < slots.length; layer++) {
      for (let slot = 0; slot < MATRIX_SLOTS; slot++) {
        const now = slots[layer][slot];
        const was = this._sentMatrix ? this._sentMatrix[layer][slot] : null;
        if (!was || was.source !== now.source) {
          this.coordinator.setMatrix(layer, slot, MATRIX_FIELD_SOURCE, now.source);
          pushed++;
        }
        if (!was || was.dest !== now.dest) {
          this.coordinator.setMatrix(layer, slot, MATRIX_FIELD_DEST, now.dest);
          pushed++;
        }
        if (!was || was.curve !== now.curve) {
          this.coordinator.setMatrix(layer, slot, MATRIX_FIELD_CURVE, now.curve);
          pushed++;
        }
        if (!was || was.scale !== now.scale) {
          this.coordinator.setMatrix(layer, slot, MATRIX_FIELD_SCALE_SRC, now.scale);
          pushed++;
        }
      }
    }
    // Structured clone: the decoder hands out fresh objects each tick, but this
    // memo outlives them and must not alias.
    this._sentMatrix = slots.map((layer) => layer.map((s) => ({ ...s })));
    return pushed;
  }

  /// The same for the keyboard record.
  _resendKey(rec) {
    if (!this.coordinator) return 0;
    const was = this._sentKey;
    let pushed = 0;
    if (!was || was.mode !== rec.mode) {
      this.coordinator.setKeyMode(rec.mode);
      pushed++;
    }
    if (!was || was.split !== rec.split) {
      this.coordinator.setSplitPoint(rec.split);
      pushed++;
    }
    if (!was || was.link !== rec.link) {
      this.coordinator.setLfo2Link(rec.link);
      pushed++;
    }
    this._sentKey = { mode: rec.mode, split: rec.split, link: rec.link };
    return pushed;
  }

  /// One pump: tick the controller, resync the engine, hand the page its batch.
  /// Returns the batch (for tests; the page gets it via `applyViewEvents`).
  pump() {
    const events = this.controller.tick();

    // (1) Engine resync from the echoes, BEFORE the mirror — see the ordering
    // note at the top of this file. Topology first, depths second.
    let corpusDirty = false;
    for (const ev of events) {
      if (ev.kind === "matrix") this._resendMatrix(ev.slots);
      else if (ev.kind === "keys") this._resendKey(ev);
      else if (ev.kind === "preset_corpus_changed") corpusDirty = true;
    }

    // (2) Param values into the store SAB the worklet folds in at block start.
    this.controller.mirrorToStore();

    // (3) Telemetry rides the same batch the controller's events do — one
    // `applyViewEvents` per frame, matching what the native shell pushes into a
    // single `evaluate_script`. Meters and scope never pass through the
    // controller: they are audio-thread data on their own SAB (0288).
    if (this.coordinator) {
      const meters = this.coordinator.pollMeters();
      if (meters) events.push(meterEvent(meters));
      const scope = this.coordinator.pollScope();
      if (scope) events.push(scopeEvent(scope));
    }

    this._deliver(events);
    if (corpusDirty) this.publishCorpus();

    // (4) Persistence ops off the tick. Always drained, even with no sink, so
    // the wasm journal cannot grow without bound.
    const ops = this.controller.takeJournal();
    if (ops.length && this.onJournal) this.onJournal(ops);

    this._frame++;
    return events;
  }

  start() {
    if (this._running || !this._raf) return this;
    this._running = true;
    const loop = () => {
      if (!this._running) return;
      try {
        this.pump();
      } catch (e) {
        // A throw here would kill the rAF chain and freeze the whole faceplate
        // silently. Report and keep pumping — the next frame usually works.
        console.error("vxn: pump failed", e);
      }
      this._raf(loop);
    };
    this._raf(loop);
    return this;
  }

  stop() {
    this._running = false;
    return this;
  }
}
