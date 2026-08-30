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
// provides both: an `ipc` shim routing each opcode to the controller wasm, and a
// pump turning controller ticks + telemetry frames into one `applyViewEvents`
// call per animation frame.
//
// Routing rule: an opcode with a presence in the MODEL goes to the controller
// and nowhere else; one with none goes straight onto the ring (`routeOpcode`
// argues each case). `pump`'s step (1)/(2) ordering is load-bearing — read the
// comment there before touching it.

import {
  LAYER_L1,
  LAYER_L2,
  MATRIX_FIELD_SOURCE,
  MATRIX_FIELD_DEST,
  MATRIX_FIELD_POLARITY,
  MATRIX_FIELD_SCALE_SRC,
  MATRIX_FIELD_SHAPE,
  MATRIX_FIELD_SCALE_SHAPE,
  MATRIX_FIELD_ENABLED,
  MATRIX_SLOTS,
} from "./event-codec.mjs";
import { WebController } from "./controller.mjs";

// ── The custom-op vocabulary ───────────────────────────────────────────────
//
// The JS half of `vxn1b_engine::vocab`, and the only copy of it outside Rust:
// the page sends NAMES, these tables turn them into the ordinals the wire and
// the controller carry. `vocab-agreement.test.mjs` asserts all four against the
// built controller wasm, because every failure here is silent — a renamed name
// makes the lookup `undefined` and `routeOpcode` drops the op (the knob moves,
// the sound does not), and a reordered ordinal is worse: the op lands, on the
// wrong field.

/// Scope tap names → `vxn1b_engine::ScopeTap::code()`.
export const SCOPE_TAP = { off: 0, upper: 1, lower: 2 };

/// Layer names → wire layer index (`Layer`'s discriminant).
export const LAYER = { upper: LAYER_L1, lower: LAYER_L2 };

/// Matrix field names → wire field index. Keys are the wire spellings the
/// matrix panel sends (`assets/panels/matrix.js`'s `wire:` column) and the
/// values are `vxn1b_engine::vocab::MATRIX_FIELD_NAMES` positions — including
/// the hyphenated `scale-shape`, which is why this one entry is quoted.
///
/// The ordinals are deliberately not in reading order: `scale` sits at 3 and
/// `shape` at 4 because 0..3 predate the polarity/shape split and are frozen.
export const MATRIX_FIELD = {
  source: MATRIX_FIELD_SOURCE,
  dest: MATRIX_FIELD_DEST,
  polarity: MATRIX_FIELD_POLARITY,
  scale: MATRIX_FIELD_SCALE_SRC,
  shape: MATRIX_FIELD_SHAPE,
  "scale-shape": MATRIX_FIELD_SCALE_SHAPE,
  enabled: MATRIX_FIELD_ENABLED,
};

/// The same seven fields keyed by SNAPSHOT property instead of wire name, for
/// `_resendMatrix`'s diff. A second table rather than a reuse of `MATRIX_FIELD`
/// because the two vocabularies genuinely differ: the wire says `scale-shape`,
/// the snapshot (`vxn1b_ui_web::slots_json`, and `controller.mjs`'s decode of
/// the same record) says `scaleShape`. An array, so the resend order is the
/// ordinal order and a test can pin the whole list in one `deepEqual`.
const RESEND_FIELDS = [
  ["source", MATRIX_FIELD_SOURCE],
  ["dest", MATRIX_FIELD_DEST],
  ["polarity", MATRIX_FIELD_POLARITY],
  ["scale", MATRIX_FIELD_SCALE_SRC],
  ["shape", MATRIX_FIELD_SHAPE],
  ["scaleShape", MATRIX_FIELD_SCALE_SHAPE],
  ["enabled", MATRIX_FIELD_ENABLED],
];

/// Look a name up in one of the vocabulary tables above, `undefined` unless the
/// table OWNS that key.
///
/// A plain `TABLE[name]` inherits from `Object.prototype`, so `"constructor"`
/// and `"__proto__"` come back truthy, sail past the `=== undefined` guards
/// below, and are then coerced by `field >>> 0` to 0 — turning a junk opcode
/// into an edit of slot N's *source*. That is precisely the silent mis-route the
/// rest of this file is built to prevent, so every lookup goes through here.
/// `hasOwnProperty.call` rather than `Object.hasOwn` only to keep the floor at
/// the Safari version the audio path already needs.
function vocabLookup(table, name) {
  return Object.prototype.hasOwnProperty.call(table, name) ? table[name] : undefined;
}

/// Split-point slider range and default, mirrored from `vxn1b_engine::vocab`
/// (`SPLIT_POINT_MIN` / `MAX` / `DEFAULT_SPLIT_POINT`). The page stamps the
/// input's `min`/`max`/`value` from here rather than hard-coding them in HTML.
export const SPLIT_POINT = { min: 12, max: 96, default: 60 };

/// Opcodes the page posts that have no controller handler, by design.
/// `set_edit_layer` is handled in-page — the faceplate rebinds its cells
/// locally and nothing downstream needs the news — so dropping it is correct,
/// not a gap. Listed rather than silently ignored so an unrouted opcode still
/// warns. (`reset_layer` was here too until 0307 made it live.)
const KNOWN_UNHANDLED = new Set(["set_edit_layer"]);

/// Meter frame layout — `MeterTap` order, from vxn-core-utils::meter. The page
/// wants the named shape `vxn1b_ui_web::serialise_custom_payload` produces, so
/// the flat telemetry region is mapped here rather than in the page.
export function meterEvent(frame) {
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
export function scopeEvent(frame) {
  const s = new Array(frame.length);
  for (let i = 0; i < frame.length; i++) {
    s[i] = Math.round(Math.min(2, Math.max(-2, frame[i])) * 1000) / 1000;
  }
  return { kind: "scope", s };
}

/// One entry per opcode the page can post, keyed by its `op` string. Each
/// handler takes the whole routing context as one record and returns whether it
/// handled the op, so adding an opcode is adding one entry — the shape
/// `routeOpcode` used to spell out as a 13-arm switch.
///
/// `ctx` is `{ctrl, coord, msg, hooks}`: the controller wasm, the ring
/// coordinator (used by exactly two opcodes), the decoded message, and the
/// page-side hooks.
///
/// Null-prototyped so a bare lookup cannot reach an inherited member: an `op`
/// of `"constructor"` has to miss, not hand back a callable.
const OPCODE_HANDLERS = {
  __proto__: null,

  // ---- controller only: params + gestures ------------------------------
  set_param: ({ ctrl, msg }) => {
    ctrl.setParam(msg.id, msg.plain);
    return true;
  },
  set_param_norm: ({ ctrl, msg }) => {
    ctrl.setParamNorm(msg.id, msg.norm);
    return true;
  },
  begin_gesture: ({ ctrl, msg }) => {
    ctrl.beginGesture(msg.id);
    return true;
  },
  end_gesture: ({ ctrl, msg }) => {
    ctrl.endGesture(msg.id);
    return true;
  },
  ready: ({ ctrl }) => {
    ctrl.editorReady();
    return true;
  },

  // ---- controller only: non-param state --------------------------------
  //
  // These have no CLAP id, so the store mirror cannot carry them — but they
  // DO live in the model, so the pump's echo resend puts them on the ring on
  // the next frame. Pushing here as well would double every edit.
  set_key_mode: ({ ctrl, msg }) => {
    ctrl.setKeyMode(msg.mode | 0);
    return true;
  },
  set_split_point: ({ ctrl, msg }) => {
    ctrl.setSplitPoint(msg.note | 0);
    return true;
  },
  set_lfo2_link: ({ ctrl, msg }) => {
    ctrl.setLfo2Link(!!msg.on);
    return true;
  },
  set_matrix: ({ ctrl, msg }) => {
    const layer = vocabLookup(LAYER, msg.layer);
    const field = vocabLookup(MATRIX_FIELD, msg.field);
    if (layer === undefined || field === undefined) return false;
    ctrl.setMatrix(layer, msg.slot | 0, field, msg.value | 0);
    return true;
  },

  // ---- controller only: bulk patch ops ---------------------------------
  copy_layer: ({ ctrl, msg }) => {
    const from = vocabLookup(LAYER, msg.from);
    const to = vocabLookup(LAYER, msg.to);
    if (from === undefined || to === undefined) return false;
    // Params reach the engine through the mirror; topology through the echo
    // resend in the pump. Nothing to push here.
    ctrl.copyLayer(from, to);
    return true;
  },
  reset_layer: ({ ctrl, msg }) => {
    const layer = vocabLookup(LAYER, msg.layer);
    if (layer === undefined) return false;
    // Same route as copy_layer: params reach the engine through the mirror,
    // topology through the echo resend in the pump.
    ctrl.resetLayer(layer);
    return true;
  },

  // ---- controller only: presets + folders ------------------------------
  load_factory: ({ ctrl, msg }) => {
    ctrl.loadFactory(msg.index | 0);
    return true;
  },
  load_user: ({ ctrl, msg }) => {
    ctrl.loadUser(msg.path);
    return true;
  },
  step_preset: ({ ctrl, msg }) => {
    ctrl.stepPreset(msg.delta | 0);
    return true;
  },
  save_preset: ({ ctrl, msg }) => {
    ctrl.savePreset(msg.name, msg.folder ?? null);
    return true;
  },
  rename_preset: ({ ctrl, msg }) => {
    ctrl.renamePreset(msg.path, msg.new_name);
    return true;
  },
  delete_preset: ({ ctrl, msg }) => {
    ctrl.deletePreset(msg.path);
    return true;
  },
  move_preset: ({ ctrl, msg }) => {
    ctrl.movePreset(msg.path, msg.dest_folder ?? null);
    return true;
  },
  new_folder: ({ ctrl, msg }) => {
    ctrl.newFolder(msg.suggested);
    return true;
  },
  rename_folder: ({ ctrl, msg }) => {
    ctrl.renameFolder(msg.old_name, msg.new_name);
    return true;
  },
  delete_folder: ({ ctrl, msg }) => {
    ctrl.deleteFolder(msg.name);
    return true;
  },

  // ---- ring only -------------------------------------------------------
  set_tempo: ({ coord, msg }) => {
    // No host transport in a browser, so BPM comes from a UI control
    // (E045 delta 5). Ring-only: `sync.rs` resolves subdivisions against it
    // on the audio side, and it is not part of the patch — a preset must not
    // carry the tempo you happened to be at.
    const bpm = Number(msg.bpm);
    if (!Number.isFinite(bpm) || bpm <= 0) return false;
    if (coord) coord.setTempo(bpm);
    return true;
  },
  set_scope_source: ({ coord, msg }) => {
    const tap = vocabLookup(SCOPE_TAP, msg.source);
    if (tap === undefined) return false;
    if (coord) coord.setScopeTap(tap);
    return true;
  },

  // ---- neither: answered in-page ---------------------------------------
  request_text_input: ({ msg, hooks }) => {
    // The native opcode exists because the plugin editor needs an NSWindow
    // outside the host's event monitor. A page can prompt itself, so this
    // never reaches the controller — whose OpenTextInput / TextInputResult
    // variants 0290 deliberately does not pack.
    if (hooks.promptText) hooks.promptText(msg.id, msg.title ?? "", msg.initial ?? "");
    return true;
  },
};

/// Route one page opcode. Pure with respect to its arguments — no DOM, no
/// timers — so the destination table is testable with fakes. `coord` is used
/// for exactly one opcode (the scope tap); everything else with a model
/// presence reaches the engine through the pump's diffs.
///
/// Returns `true` if the opcode was handled. An unknown or non-string `op` is
/// dropped and returns `false` rather than being guessed at — VXN1b's page only
/// ever posts a string `op`.
export function routeOpcode(ctrl, coord, msg, hooks = {}) {
  if (!msg || typeof msg.op !== "string") return false;
  const handler = OPCODE_HANDLERS[msg.op];
  if (!handler) return false;
  return handler({ ctrl, coord, msg, hooks });
}

/// Drives the controller wasm and the page: one pump per animation frame.
export class FaceplateBridge {
  constructor({
    controller,
    coordinator = null,
    win = globalThis,
    onFlushJournal = null,
    onModelChanged = null,
    raf = null,
  } = {}) {
    if (!controller) throw new Error("FaceplateBridge needs a controller");
    this.controller = controller;
    this.coordinator = coordinator;
    this.win = win;
    // Journal flush hook (0293). Called once per pump; the OWNER drains, because
    // `PresetPersistence.flush()` calls `takeJournal()` itself and also owns the
    // write chaining and the storage-availability flag. If the pump drained too,
    // it would steal the ops and persistence would write nothing.
    this.onFlushJournal = onFlushJournal;
    /// Called on a pump whose batch carried a MODEL change (0293). Autosave
    /// debounces a write behind it. Telemetry deliberately does not count: meter
    /// and scope frames arrive every frame while sound plays, and treating them
    /// as changes would rewrite the state blob forever.
    this.onModelChanged = onModelChanged;
    this._raf =
      raf || (win && win.requestAnimationFrame ? win.requestAnimationFrame.bind(win) : null);
    this._running = false;

    // What the RING was last told, so the echo-driven resend pushes only drift.
    // Null = "tell it everything next time", which is also the boot state.
    this._sentMatrix = null;
    this._sentKey = null;

    // In-page text input.
    this._prompt = null;
    /// The on-screen piano, once mounted. The bridge only ever tells it about
    /// the split; note lighting is the producers' tap.
    this.piano = null;
  }

  /// Install the `window.ipc` shim the page posts through. Replaces whatever is
  /// there — under wry the page gets a real one; in a tab, this is it.
  install() {
    const self = this;
    this.win.ipc = {
      postMessage(json) {
        self._post(json);
      },
    };
    return this.drainBootQueue();
  }

  _post(json) {
    let msg = null;
    try {
      msg = JSON.parse(json);
    } catch (e) {
      console.warn("vxn: unparseable opcode", e);
      return;
    }
    this.handle(msg);
  }

  /// Drain the opcodes the page buffered before this module loaded.
  ///
  /// `WEB_BOOT_HEAD` installs a synchronous queuing `window.ipc` during page
  /// parse, because the faceplate's `init()` fires `ready` (and whatever else
  /// binding produces) long before an async wasm boot can finish. Those live in
  /// `window.__VXN_UI_QUEUE__` as raw JSON strings. Dropping them would cost the
  /// `ready` opcode, and with it the full re-broadcast that paints every control
  /// — the page would come up blank and stay blank until something moved.
  drainBootQueue() {
    const q = this.win.__VXN_UI_QUEUE__;
    if (Array.isArray(q)) {
      // Splice rather than reassign: the stub closed over this exact array.
      const pending = q.splice(0, q.length);
      for (const json of pending) this._post(json);
    }
    return this;
  }

  /// Tell the engine everything again: re-push the whole topology and key
  /// record, and rewrite every param slot.
  ///
  /// Called by the gesture gate once `host.start()` resolves — which is BEFORE
  /// the worklet posts `ready`, not after. Two things make it necessary, both
  /// consequences of the faceplate being live before the audio gesture:
  /// `WebHost.start()` seeds the store with the ENGINE's defaults (clobbering
  /// anything edited while waiting), and ring pushes made before audio existed
  /// can have been refused if the ring filled, with the memo believing they
  /// landed.
  resyncEngine() {
    // Clearing this side's memos is necessary but NOT sufficient: the resend
    // only runs when a matrix / key record arrives in the batch, and the
    // controller has memos of its own that have not moved — so it would emit
    // nothing and the resend would never fire. `EditorReady` is the mechanism
    // that already exists for "something needs seeding from scratch": it
    // re-broadcasts every param and clears the controller's echo memos, so both
    // records land on the next tick.
    this._sentMatrix = null;
    this._sentKey = null;
    this.controller.invalidateMirror();
    this.controller.editorReady();
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
    if (this._prompt) {
      const value = this._prompt(title, initial);
      this._deliver([{ kind: "text_input_result", id, value: value ?? null }]);
      return;
    }
    const doc = this.win.document;
    if (!doc) {
      // Headless: answer immediately rather than leaving the page's callback
      // pending forever.
      this._deliver([{ kind: "text_input_result", id, value: null }]);
      return;
    }
    // The `.vxn-ti-*` styles ship in WEB_BOOT_HEAD for exactly this: the
    // desktop build opens a native NSWindow (outside the host's key-event
    // monitor, so Space types instead of starting transport); a page has no such
    // problem and builds the box itself.
    const backdrop = doc.createElement("div");
    backdrop.className = "vxn-ti-backdrop";
    const box = doc.createElement("div");
    box.className = "vxn-ti-box";
    const label = doc.createElement("div");
    label.className = "vxn-ti-title";
    label.textContent = title || "";
    const input = doc.createElement("input");
    input.className = "vxn-ti-input";
    input.type = "text";
    input.value = initial || "";
    box.append(label, input);
    backdrop.append(box);
    doc.body.append(backdrop);
    input.focus();
    input.select();

    let done = false;
    const finish = (value) => {
      if (done) return; // fire-once: Enter then blur must not answer twice
      done = true;
      backdrop.remove();
      this._deliver([{ kind: "text_input_result", id, value }]);
    };
    input.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") {
        ev.preventDefault();
        finish(input.value);
      } else if (ev.key === "Escape") {
        ev.preventDefault();
        finish(null);
      }
      // Every other key stops here: the faceplate binds single-key shortcuts on
      // the document, and typing a preset name must not trigger them.
      ev.stopPropagation();
    });
    backdrop.addEventListener("pointerdown", (ev) => {
      if (ev.target === backdrop) finish(null); // click-outside cancels
    });
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
        for (const [key, field] of RESEND_FIELDS) {
          if (was && was[key] === now[key]) continue;
          // `enabled` is a bool in the snapshot and 0/1 on the ring; the rest
          // are already the wire `u8`. Same conversion the panel's `_edit` does.
          const v = now[key];
          this.coordinator.setMatrix(layer, slot, field, v === true ? 1 : v === false ? 0 : v);
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

    // (1) Engine resync from the echoes, BEFORE the mirror at (2). Topology
    // first, depths second — and that order is LOAD-BEARING.
    //
    // Slot DEPTH is a CLAP param (store SAB); slot TOPOLOGY is a ring event. A
    // load moves both, and nothing makes the two writes atomic against a block
    // boundary, so a block can see one and not the other.
    //
    // The worklet reads the store FIRST and the ring SECOND within one quantum
    // (`audio-host.mjs` process(): applyStoreToEngine at (1), drainRawInto at
    // (2)). Given that, pushing the ring BEFORE mirroring the store makes the
    // harmful tear impossible: for a block to see new depths with old topology
    // it would need the mirror (second) to land before the store read AND the
    // ring push (first) to land after the ring read — i.e. the first write
    // after the second. The only reachable tear is new-topology-with-old-depth,
    // which is the right route at a stale amount rather than a stale route at a
    // new amount.
    //
    // **If audio-host.mjs ever reads the ring before the store, this inverts
    // and nothing here will fail — it will just occasionally click on preset
    // load.** The two orderings are load-bearing as a pair.
    //
    // This is a small effect and not a browser-only one: the native plugin has
    // the same window (`SharedParams::restore_from_bytes` writes params, then
    // topology, then the reload flag, while the audio thread folds params every
    // block), and the click-prone destinations are smoothed anyway
    // (`mod_smoothing.rs`, 0208).
    let corpusDirty = false;
    // Whether anything in the PATCH moved. Computed here, before the telemetry
    // frames are appended below, so a sounding note never reads as an edit.
    let modelMoved = false;
    for (const ev of events) {
      if (ev.kind === "matrix") {
        this._resendMatrix(ev.slots);
        modelMoved = true;
      } else if (ev.kind === "keys") {
        this._resendKey(ev);
        // Shade the on-screen keys below the split — but only in Split mode
        // (2), where the boundary means something. The widget takes a bare note
        // number, so nothing about layers or key modes reaches it; this is the
        // per-synth half, and it rides the echo the model already sends.
        if (this.piano) this.piano.setSplit(ev.mode === 2 ? ev.split : null);
        modelMoved = true;
      } else if (ev.kind === "preset_corpus_changed") corpusDirty = true;
      else if (ev.kind === "param_changed" || ev.kind === "preset_loaded") modelMoved = true;
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

    // (4) Persistence ops off the tick. With a flush hook the owner drains; with
    // none, drain and drop anyway, or the wasm journal grows without bound in a
    // page that has no storage.
    if (this.onFlushJournal) this.onFlushJournal();
    else this.controller.takeJournal();

    // (5) Autosave, debounced behind a real patch change.
    if (modelMoved && this.onModelChanged) this.onModelChanged();

    return events;
  }

  start() {
    if (this._running) return this;
    if (!this._raf) {
      // Every browser has rAF, so this means a host that isn't one. Say so:
      // a pump that never runs looks exactly like a page that booted fine and
      // then froze.
      console.warn("vxn: no requestAnimationFrame — the pump will not run; call pump() yourself");
      return this;
    }
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

// ── Boot ────────────────────────────────────────────────────────────────────
//
// The generated page loads THIS module (`<script type="module"
// src="./faceplate-bridge.mjs">`, spliced into `__WEB_BOOT_LOADER__` just after
// the faceplate's inline script), so it has to stand the whole thing up.

/// Wire the audio host, the controller wasm and the bridge together, and start
/// pumping. Resolves once the UI is live — which is BEFORE audio is: the
/// faceplate is fully interactive while the page waits for the gesture that
/// autoplay policy requires, and edits made in that window are held in the model
/// and the ring until the first live quantum.
export async function boot({
  win = globalThis,
  wasmUrl,
  controllerWasmUrl,
  fetchImpl,
  autoGesture = true,
  autoInputs = true,
  autoPiano = true,
  autoCpuMeter = true,
  autoPersist = true,
  adapters = null,
} = {}) {
  // Dynamic so a headless importer (the node suites) never pulls the audio
  // stack in just to exercise the router.
  const { WebHost } = await import("./coordinator.mjs");

  // Created before the host, so the very first `cpu` message has somewhere to
  // land. A stub until the shared module resolves.
  let cpuMeter = { update() {}, el: null };
  const host = new WebHost({
    ...(wasmUrl ? { wasmUrl } : {}),
    ...(fetchImpl ? { fetchImpl } : {}),
    onCpu: (load, peak) => cpuMeter.update(load, peak),
    onTrap: (err, count) => {
      // 0297: a trap goes silent and REPORTS. It does not re-instantiate — a
      // rebuilt engine loses key mode, split, LFO 2 link and the whole
      // topology, so "recovered" audio would play the wrong patch with nothing
      // on screen saying so. Reloading is the honest answer.
      console.error(`vxn: render trap #${count} — audio stopped, reload the page`, err);
    },
  });

  const controller = await new WebController({
    ...(controllerWasmUrl ? { wasmUrl: controllerWasmUrl } : {}),
    ...(fetchImpl ? { fetchImpl } : {}),
    // ONE store: the host allocated it with the rest of the transport, and the
    // controller mirrors the model into it. Two would silently diverge.
    store: host.store,
  }).instantiate();

  const bridge = new FaceplateBridge({ controller, coordinator: host, win });

  // Install before the first pump so the opcodes the page queued during parse —
  // `ready` among them — are routed into this tick rather than the next.
  // Persistence FIRST, before the queued opcodes are flushed. `ready` is in that
  // queue and triggers the re-broadcast that paints every control and seeds the
  // param SAB — so hydrating and restoring now means the restored patch is what
  // gets painted. Install afterwards and the page paints defaults, then quietly
  // disagrees with the model. The boot stub keeps queuing meanwhile, which is
  // what it is for.
  let persistence = null;
  let autosave = null;
  if (autoPersist) {
    // Bounded, NOT unbounded. Hydration wants to finish before `install()`
    // flushes the queued `ready` (0293), so the restored patch is what paints —
    // but IndexedDB can simply never answer: a blocked profile, private mode, or
    // a headless browser with no storage backend. Awaiting that forever takes
    // the whole instrument down with it — no install, so nothing paints; no
    // input attach and no gesture gate, so no sound at all. Persistence is
    // convenience ([[0297]]); it does not get to gate the synth.
    const pending = attachPersistence(win, controller, bridge, adapters);
    const settled = await withTimeout(pending, PERSIST_BOOT_TIMEOUT_MS);
    if (settled === TIMED_OUT) {
      console.warn(
        `vxn: storage did not answer in ${PERSIST_BOOT_TIMEOUT_MS}ms — ` +
          "carrying on without it; presets may appear late",
      );
      // If it does eventually land, the corpus arrived after the publish below,
      // so republish then rather than leaving the browser panel empty.
      pending
        .then((r) => {
          persistence = r.persistence;
          autosave = r.autosave;
          bridge.publishCorpus();
        })
        .catch(() => {});
    } else {
      ({ persistence, autosave } = settled);
    }
  }

  bridge.install();
  // After hydration, so the browser panel gets factory AND the user's folders.
  bridge.publishCorpus();

  // The piano mounts BEFORE the inputs, so the note tap below can exist: an
  // adapter attached first would capture the untapped host and its notes would
  // sound without lighting anything.
  let piano = null;
  if (autoPiano) piano = await attachPiano(win, host, adapters);
  bridge.piano = piano;

  // Everything that plays a note goes through here, so MIDI and the computer
  // keyboard light the on-screen keys exactly as a click does.
  const noteHost = pianoNoteTap(host, piano);

  // The computer keyboard attaches NOW, before audio exists. A keypress then
  // does both jobs at once: it satisfies the gesture gate and its note lands in
  // the ring, which the runner applies on the first live quantum
  // (silence-until-ready). Attaching it after `start()` would eat the very
  // keystroke the player used to wake the thing up.
  let inputs = null;
  if (autoInputs) inputs = await attachKeyboardInput(win, noteHost, adapters);


  // Render-load badge (0309). Shared widget, fed by the worklet's `cpu` port
  // messages through `onCpu` above. Worth having on a demo: this port runs 32
  // voices with an oversampled ladder and a full FX chain in single-threaded
  // wasm, and E045 flags worst-case performance as an open question rather than
  // a known-good — so the number belongs on screen, not in a profiler.
  if (autoCpuMeter && win.document && win.document.body) {
    try {
      const create = await sharedAdapter(adapters, "cpu-meter.mjs", "createCpuMeter");
      cpuMeter = create(win.document);
    } catch (e) {
      console.warn("vxn: CPU meter unavailable", e && e.message);
    }
  }

  // Web MIDI waits for the gesture: asking for the permission prompt on page
  // load, before the player has touched anything, is rude and easy to deny by
  // reflex.
  // Pump LAST. Its first tick carries the boot `keys` record, and that is what
  // sets the initial split shading — start it before the piano is wired and the
  // shading is missing until the key mode next changes.
  bridge.start();

  if (autoGesture) attachGestureGate(win, host, bridge, { autoInputs, adapters, noteHost });

  const wired = { host, controller, bridge, inputs, piano, cpuMeter, persistence, autosave };
  // Reachable from the console. The auto-boot path discards this object
  // otherwise, which leaves a running page with no handle on anything — no way
  // to ask whether a note reached the ring, or what the gate thinks it is doing.
  const vxn = win.__vxn || (win.__vxn = {});
  vxn.debug = wired;
  return wired;
}

/// How long boot waits for storage before giving up on ordering and coming up
/// anyway. Long enough that a normal IndexedDB open wins comfortably, short
/// enough that a blocked one is a blink rather than a hang.
const PERSIST_BOOT_TIMEOUT_MS = 2000;

const TIMED_OUT = Symbol("timed-out");

/// Resolve `promise`, or `TIMED_OUT` if it takes longer than `ms`. The timer is
/// cleared when the promise wins, so a fast path leaves nothing pending — a
/// dangling 2s timer would hold a test runner's loop open after the run.
function withTimeout(promise, ms) {
  let timer = null;
  const deadline = new Promise((resolve) => {
    timer = setTimeout(() => resolve(TIMED_OUT), ms);
  });
  return Promise.race([
    promise.then((v) => {
      if (timer !== null) clearTimeout(timer);
      return v;
    }),
    deadline,
  ]);
}

/// VXN1b's IndexedDB identity. Its own name, so the three synths' corpora never
/// collide in one origin (`vxn1-presets` / `vxn2-presets` are the siblings).
export const DB_ID = { name: "vxn1b-presets", version: 1 };

/// Browser persistence (0293): user presets in IndexedDB, full-state autosave,
/// and the patch export / import / share helpers.
///
/// Every path is best-effort by design ([[0297]]): persistence here is
/// convenience — your patch is still there next visit — not durability. Private
/// mode, a blocked IndexedDB, a quota eviction: log it once and carry on with a
/// playable instrument at defaults. Nothing may throw out of boot.
async function attachPersistence(win, controller, bridge, injected) {
  let persistence = null;
  let autosave = null;
  try {
    const PresetPersistence = await sharedAdapter(
      injected, "preset-persistence.mjs", "PresetPersistence");
    persistence = new PresetPersistence({ controller, dbId: DB_ID });
    await persistence.hydrate();
    persistence.attachFlushOnHide(win, win.document);
    // The pump asks for a flush each frame; `flush()` drains the journal itself
    // and chains the IndexedDB write off the tick.
    bridge.onFlushJournal = () => persistence.flush();
  } catch (e) {
    console.warn("vxn: user-preset persistence unavailable", e && e.message);
  }

  try {
    const StateAutosave = await sharedAdapter(injected, "state-autosave.mjs", "StateAutosave");
    const patchIo = injected && injected.patchIo ? injected.patchIo : await import("./patch-io.mjs");
    autosave = new StateAutosave({ controller, dbId: DB_ID });
    // A share link wins over the autosaved session: it is an explicit thing the
    // user followed, and restoring last time's patch over it would silently
    // discard what they clicked.
    const fromShare = patchIo.applyShareLinkOnBoot(controller, {
      location: win.location,
      history: win.history,
    });
    if (!fromShare) await autosave.restore();
    autosave.attachFlushOnHide(win, win.document);
    // Autosave watches the model through the bridge's pump rather than a timer
    // of its own: every tick that produced view events is a change worth
    // debouncing a write behind.
    bridge.onModelChanged = () => autosave.schedule();

    // No faceplate button for these yet, so expose them on the page surface —
    // usable from the console and ready for a UI without touching this again.
    const vxn = win.__vxn || (win.__vxn = {});
    vxn.exportPatch = (name) =>
      patchIo.exportPatchFile(controller, { name, product: "VXN-1b", doc: win.document });
    vxn.importPatch = (onResult) =>
      patchIo.importPatchFile(controller, { product: "VXN-1b", doc: win.document, onResult });
    vxn.shareLink = () => patchIo.shareLinkFor(controller, win.location);
  } catch (e) {
    console.warn("vxn: state autosave unavailable", e && e.message);
  }
  // Returned so a caller can flush and stop them; a pending debounce timer that
  // fires after the controller is torn down would trap the wasm.
  return { persistence, autosave };
}

/// Resolve a shared adapter (0284). In `dist/` everything is FLAT, so
/// `./keyboard-input.mjs` sits beside this file and the dynamic import just
/// works; in the source tree it lives under `crates/vxn-core-web/assets`, two
/// roots away, so a headless caller injects it instead. Same seam vxn-2 uses,
/// and the reason the import is dynamic rather than static.
async function sharedAdapter(injected, name, symbol) {
  if (injected && typeof injected[symbol] === "function") return injected[symbol];
  const mod = await import(`./${name}`);
  return mod[symbol];
}

/// Computer keyboard → ring. Shared adapter; it calls noteOn/noteOff only, so it
/// needs no VXN1b-specific handling.
async function attachKeyboardInput(win, host, injected) {
  if (!win.document) return null;
  try {
    const attach = await sharedAdapter(injected, "keyboard-input.mjs", "attachKeyboard");
    return attach(host, { target: win.document });
  } catch (e) {
    console.warn("vxn: keyboard input unavailable", e);
    return null;
  }
}

/// Wrap the coordinator so notes played by OTHER producers light the on-screen
/// keys. The input adapters get this instead of the bare host; every call still
/// reaches the real one, and `noteOn` / `noteOff` additionally paint.
///
/// A Proxy rather than a hand-written delegate because the adapters call a
/// surface that grows: pitch bend, mod wheel, both pressure messages, and
/// whatever a later adapter reaches for. Listing them by hand would silently
/// drop the next one added.
///
/// The piano is NOT given this wrapper — it paints its own presses directly, and
/// routing it through here would only paint the same key twice.
export function pianoNoteTap(host, piano) {
  if (!piano || typeof piano.setActive !== "function") return host;
  return new Proxy(host, {
    get(target, prop) {
      const value = Reflect.get(target, prop, target);
      if (typeof value !== "function") return value;
      // Painting must NEVER cost a note. It is a cosmetic side effect on the
      // audio path, so anything it throws is swallowed here rather than taking
      // the note-on down with it — a lit key with no sound is the worst of both
      // (it looks like the synth heard you), and it is exactly what an
      // unguarded side effect in front of `noteOn` produces.
      const light = (note, on) => {
        try {
          piano.setActive(note, on);
        } catch (e) {
          console.warn("vxn: piano paint failed", e && e.message);
        }
      };
      if (prop === "noteOn") {
        return (note, velocity, offset, channel) => {
          light(note, true);
          return value.call(target, note, velocity, offset, channel);
        };
      }
      if (prop === "noteOff") {
        return (note, offset, channel) => {
          light(note, false);
          return value.call(target, note, offset, channel);
        };
      }
      return value.bind(target);
    },
  });
}

/// On-screen piano → ring. Shared widget; a pure note producer that calls the
/// same `noteOn` / `noteOff` the computer keyboard does, so the engine cannot
/// tell which played it.
async function attachPiano(win, host, injected) {
  if (!win.document || !win.document.body) return null;
  try {
    const create = await sharedAdapter(injected, "piano-keyboard.mjs", "createPianoKeyboard");
    return create(win.document, host);
  } catch (e) {
    console.warn("vxn: on-screen piano unavailable", e && e.message);
    return null;
  }
}

/// Web MIDI → ring. Resolves even when access is denied or Web MIDI is absent
/// (Safari): the adapter reports `state.granted === false` rather than throwing,
/// and the computer keyboard is already attached as the fallback.
async function attachMidiInput(host, injected) {
  try {
    const attachMidi = await sharedAdapter(injected, "midi-input.mjs", "attachMidi");
    const midi = await attachMidi(host, {
      onError: (err) => console.warn("vxn: Web MIDI unavailable", err),
    });
    if (midi && midi.state && midi.state.granted === false) {
      console.info("vxn: no Web MIDI — use the computer keyboard");
    }
    return midi;
  } catch (e) {
    console.warn("vxn: Web MIDI attach failed", e);
    return null;
  }
}

/// Start audio on the first user gesture, then resync the engine.
///
/// Autoplay policy requires `ctx.resume()` to happen inside a gesture call
/// stack; without this there is no sound at all, which is why 0297 kept it while
/// cutting the rest of the lifecycle machinery. Listeners are one-shot and cover
/// both pointer and keyboard, so a player who reaches for the computer-keyboard
/// octave first is not left in silence wondering.
export function attachGestureGate(win, host, bridge, { autoInputs = true, adapters = null, noteHost = host } = {}) {
  const doc = win.document;
  if (!doc) return () => {};
  let started = false;
  const onGesture = async () => {
    if (started) return;
    started = true;
    detach();
    try {
      await host.start();
      // The store now holds the ENGINE's defaults (`_seedStoreFromDefaults`),
      // which may have overwritten edits made before the gesture, and any ring
      // push made while the ring had no consumer may have been refused. Tell the
      // engine everything again — see `resyncEngine`.
      bridge.resyncEngine();
      if (autoInputs) await attachMidiInput(noteHost, adapters);
    } catch (e) {
      console.error("vxn: audio failed to start", e);
    }
  };
  const detach = () => {
    doc.removeEventListener("pointerdown", onGesture);
    doc.removeEventListener("keydown", onGesture);
  };
  doc.addEventListener("pointerdown", onGesture);
  doc.addEventListener("keydown", onGesture);
  return detach;
}

// Auto-boot when loaded as the page's module, and only then: the node suites
// import this file for `routeOpcode` / `FaceplateBridge` and must not stand up
// an AudioContext to do it. `__VXN_NO_AUTOBOOT__` is the escape hatch for a
// browser test that wants to drive `boot()` itself.
if (
  typeof window !== "undefined" &&
  typeof document !== "undefined" &&
  !globalThis.__VXN_NO_AUTOBOOT__
) {
  boot().catch((e) => console.error("vxn: boot failed", e));
}
