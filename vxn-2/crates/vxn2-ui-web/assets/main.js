// VXN2 main bootstrap — wires DOM data-vxn-* attributes to the panel
// primitives, installs the real applyViewEvents dispatcher, and posts
// `ready` once binding is done.
//
// Resolution model (post ADR 0002: single patch surface):
//   - data-vxn-param="<machine-id>" -> resolveParamId() -> CLAP id. Ids are
//     flat and unprefixed; no per-layer demux.
//   - data-vxn-section="<name>" -> top-level renderer dispatch (for
//     panels whose binding is more than just "wire each child fader").
//   - data-vxn-custom="<key>" -> fires the matching IPC opcode. Most are
//     thin shims that 0027 / 0028 / 0029 hook into.
//
// applyViewEvents fans out param_changed events to each bound
// primitive's `set()`. text_input_result rounds back to dispatch
// set_param when the popup commits.

(function () {
  const vxn = window.__vxn;
  const panels = vxn.panels;
  const dispatch = vxn.dispatch;

  // ── Native-popup capability ──
  // macOS + Windows ship a native NSWindow / WS_POPUP text-input popup
  // in `vxn-core-ui-web::text_input`; on Linux the core no-op-cancels
  // immediately, so the JS-side `<dialog>` fallback (below) owns the
  // round-trip there.
  const NATIVE_POPUP = /(Mac|Macintosh|Windows|Win64|Win32)/i.test(
    navigator.userAgent || ""
  );

  // ── Promise-based text-input dispatcher ──
  // dispatchTextInput(title, initial) → Promise<string | null>.
  // Resolves with the committed string on Enter, `null` on cancel.
  // Backed by `pendingTextInputs[id] = resolve`; the `text_input_result`
  // arm of `applyEvent` and the fallback `<dialog>` commit handler both
  // resolve via the same registry — there's no other code path.
  let nextTextInputSeq = 0;
  function dispatchTextInput(title, initial) {
    const reqId = "t" + (nextTextInputSeq++) + ":" + Date.now().toString(36);
    return new Promise(function (resolve) {
      vxn.pendingTextInputs[reqId] = resolve;
      if (NATIVE_POPUP) {
        dispatch("request_text_input", {
          id: reqId,
          title: title || "",
          initial: initial == null ? "" : String(initial),
        });
      } else {
        showFallbackDialog(reqId, title || "", initial == null ? "" : String(initial));
      }
    });
  }
  vxn.dispatchTextInput = dispatchTextInput;

  function resolveTextInput(id, value) {
    const resolve = vxn.pendingTextInputs[id];
    if (!resolve) return false;
    delete vxn.pendingTextInputs[id];
    resolve(value == null ? null : String(value));
    return true;
  }

  // ── Linux in-page <dialog> fallback ──
  // Created lazily on first use. Single shared element — only one popup
  // can be in flight at a time, matching the native popup's behaviour.
  let fallbackDialog = null;
  let fallbackInput = null;
  let fallbackTitle = null;
  let fallbackActiveId = null;

  function ensureFallbackDialog() {
    if (fallbackDialog) return;
    fallbackDialog = document.createElement("dialog");
    fallbackDialog.className = "vxn-text-input-fallback";
    fallbackTitle = document.createElement("div");
    fallbackTitle.className = "vxn-text-input-fallback-title";
    fallbackInput = document.createElement("input");
    fallbackInput.type = "text";
    fallbackInput.className = "vxn-text-input-fallback-field";
    const form = document.createElement("form");
    form.method = "dialog";
    form.appendChild(fallbackTitle);
    form.appendChild(fallbackInput);
    fallbackDialog.appendChild(form);
    document.body.appendChild(fallbackDialog);

    fallbackInput.addEventListener("keydown", function (ev) {
      if (ev.key === "Enter") {
        ev.preventDefault();
        commitFallback(fallbackInput.value);
      } else if (ev.key === "Escape") {
        ev.preventDefault();
        commitFallback(null);
      }
    });
    fallbackDialog.addEventListener("click", function (ev) {
      // Click on the backdrop dismisses; clicks on the form bubble
      // here too, so guard on the target being the dialog itself.
      if (ev.target === fallbackDialog) commitFallback(null);
    });
    fallbackDialog.addEventListener("close", function () {
      // Belt-and-braces: if anything else closed the dialog (e.g. host
      // navigates away) and we still have an active id, cancel.
      if (fallbackActiveId !== null) commitFallback(null);
    });
  }

  function showFallbackDialog(id, title, initial) {
    ensureFallbackDialog();
    if (fallbackActiveId !== null) {
      // Another popup is in flight. Cancel it first so the registry
      // stays tidy; the new one takes over.
      const prior = fallbackActiveId;
      fallbackActiveId = null;
      resolveTextInput(prior, null);
      if (fallbackDialog.open) fallbackDialog.close();
    }
    fallbackActiveId = id;
    fallbackTitle.textContent = title;
    fallbackInput.value = initial;
    if (typeof fallbackDialog.showModal === "function") {
      fallbackDialog.showModal();
    } else {
      fallbackDialog.setAttribute("open", "");
    }
    // Select-all so typing replaces the initial value, matching the
    // native popup.
    try { fallbackInput.select(); } catch (_) {}
  }

  function commitFallback(value) {
    const id = fallbackActiveId;
    if (id === null) return;
    fallbackActiveId = null;
    if (fallbackDialog && fallbackDialog.open) fallbackDialog.close();
    resolveTextInput(id, value);
  }

  // ── Param-id resolution ──
  function resolveParamId(name) {
    const desc = vxn.paramsByName[name];
    return desc ? desc.id : -1;
  }

  function resolveDesc(name) {
    return vxn.paramsByName[name] || null;
  }

  // ── Tempo-sync rate/time faders ──
  // Mirror of `vxn2_engine::sync::sync_pairs`: each rate/time param and its
  // sync-toggle partner. While the partner is on, the fader *is* the
  // subdivision selector (matching VXN1) — its position picks a division and
  // the readout shows that label instead of Hz/ms. The engine derives the
  // same index from the same normalised position, so audio and label agree.
  const SYNC_PAIR_NAMES = [
    ["lfo1-rate", "lfo1-sync"],
    ["delay-time", "delay-sync"],
    ["lfo2-rate", "lfo2-sync"],
  ];
  // rate CLAP id → sync-toggle CLAP id.
  const syncPartnerByRateId = Object.create(null);
  for (let i = 0; i < SYNC_PAIR_NAMES.length; i++) {
    const rateId = resolveParamId(SYNC_PAIR_NAMES[i][0]);
    const syncId = resolveParamId(SYNC_PAIR_NAMES[i][1]);
    if (rateId >= 0 && syncId >= 0) syncPartnerByRateId[rateId] = syncId;
  }

  // Subdivision label a normalised [0,1] fader position selects. Mirrors
  // `vxn2_dsp::lfo::index_from_norm`: round(norm * (N-1)).
  function subdivisionLabel(norm) {
    const t = vxn.subdivisions || [];
    if (!t.length) return "";
    const n = norm < 0 ? 0 : norm > 1 ? 1 : norm;
    return t[Math.round(n * (t.length - 1))];
  }

  // ── Filter cutoff "Tuned" toggle (VXN-1 parity) ──
  // When `filter-cutoff-tuned` is on, the cutoff fader is read/displayed as a
  // musical note over C0..C4 (MIDI 12..60, semitone-snapped) instead of Hz. The
  // stored param stays Hz — only the fader's norm↔value map and readout change —
  // so the DSP and DAW automation are unaffected (the engine never reads the
  // toggle). Mirrors VXN-1's `panels.js` cutoff-tuned helpers.
  const CUTOFF_TUNED_MIDI_MIN = 12; // C0
  const CUTOFF_TUNED_MIDI_MAX = 60; // C4
  const NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
  function midiToHz(m) { return 440 * Math.pow(2, (m - 69) / 12); }
  function hzToMidi(hz) { return 69 + 12 * Math.log2(Math.max(1e-6, hz) / 440); }
  function noteName(m) {
    const n = Math.round(m);
    return NOTE_NAMES[((n % 12) + 12) % 12] + (Math.floor(n / 12) - 1);
  }
  // norm [0,1] → MIDI 12..60 (semitone-snapped) → Hz.
  function cutoffTunedNormToHz(norm) {
    const span = CUTOFF_TUNED_MIDI_MAX - CUTOFF_TUNED_MIDI_MIN;
    const c = norm < 0 ? 0 : norm > 1 ? 1 : norm;
    return midiToHz(Math.round(CUTOFF_TUNED_MIDI_MIN + c * span));
  }
  function cutoffTunedHzToNorm(hz) {
    const span = CUTOFF_TUNED_MIDI_MAX - CUTOFF_TUNED_MIDI_MIN;
    const c = (Math.round(hzToMidi(hz)) - CUTOFF_TUNED_MIDI_MIN) / span;
    return c < 0 ? 0 : c > 1 ? 1 : c;
  }
  function cutoffTunedNoteName(hz) {
    const m = Math.max(CUTOFF_TUNED_MIDI_MIN, Math.min(CUTOFF_TUNED_MIDI_MAX, Math.round(hzToMidi(hz))));
    return noteName(m);
  }
  // cutoff CLAP id → tuned-toggle CLAP id, and the inverse.
  const tunedToggleByCutoffId = Object.create(null);
  const cutoffByTunedToggleId = Object.create(null);
  {
    const cutoffId = resolveParamId("filter-cutoff");
    const tunedId = resolveParamId("filter-cutoff-tuned");
    if (cutoffId >= 0 && tunedId >= 0) {
      tunedToggleByCutoffId[cutoffId] = tunedId;
      cutoffByTunedToggleId[tunedId] = cutoffId;
    }
  }

  // ── Bound primitives, indexed by CLAP id ──
  // Each entry is an array (a param can drive several DOM controls if
  // a future layout duplicates it; today it's always <= 1 but no point
  // hard-coding that).
  const boundById = Object.create(null);
  // Last-known plain value per CLAP id. Populated by param_changed events
  // so newly-registered primitives (e.g. after an op-tab switch rebuilds
  // op-detail DOM) can hydrate to the current value instead of `default`.
  // Pre-seeded from `vxn.defaultPatch` (the engine's `default_patch`
  // table). In the running plugin the engine's all-ones dirty-bitset
  // broadcast at boot overwrites these with authoritative values; in the
  // offline HTML dump the seed is the only source of per-op variation.
  const livePlain = Object.create(null);
  if (vxn.defaultPatch && vxn.defaultPatch.length) {
    for (let i = 0; i < vxn.defaultPatch.length; i++) {
      livePlain[i] = vxn.defaultPatch[i];
    }
  }

  // ── Mid-drag suppression (ADR 0003 / ticket 0060) ──
  // The dirty-bitset pump is dumb: it pushes every drift. The view
  // filters per-widget. While the user is focused on / dragging a
  // primitive, drop the incoming `set` — the page is the source of
  // truth for that widget until the gesture releases.
  //
  // `bindGestureGated(gateEl, setFn)` wraps `setFn` so it's dropped
  // when either:
  //   - `document.activeElement === gateEl` (focused range input,
  //     <input>, <select>, etc.), OR
  //   - `gateEl.dataset.dragging === "1"` (pointer-drag widgets that
  //     don't take focus — fader, knob).
  // Widgets in the second class flip the dataset flag on their own
  // pointerdown / pointerup handlers.
  function bindGestureGated(gateEl, setFn) {
    if (!gateEl || typeof setFn !== "function") return setFn;
    return function (plain, norm, display) {
      if (document.activeElement === gateEl) return;
      if (gateEl.dataset && gateEl.dataset.dragging === "1") return;
      return setFn(plain, norm, display);
    };
  }
  vxn.bindGestureGated = bindGestureGated;

  function register(id, primitive, gateEl) {
    if (id < 0 || !primitive) return;
    let entry = primitive;
    if (gateEl && typeof primitive.set === "function") {
      // Wrap once at registration time so every drain-pump update for
      // this id is gated by the same predicate. Preserves any other
      // methods on the primitive (e.g. `setRate` on the pitch-EG
      // graph) by copying own keys onto a surrogate.
      const inner = primitive.set.bind(primitive);
      const wrapped = bindGestureGated(gateEl, inner);
      const ownKeys = Object.keys(primitive);
      const surrogate = {};
      for (let i = 0; i < ownKeys.length; i++) {
        const k = ownKeys[i];
        surrogate[k] = primitive[k];
      }
      surrogate.set = wrapped;
      // `_original` lets `unregister(id, primitive)` find the surrogate
      // even when the caller only holds the pre-wrap reference.
      surrogate._original = primitive;
      entry = surrogate;
    }
    if (!boundById[id]) boundById[id] = [];
    boundById[id].push(entry);
    if (id in livePlain) {
      try { entry.set(livePlain[id]); }
      catch (e) { console.error("vxn2 hydrate set() failed", e); }
    }
  }
  function unregister(id, primitive) {
    if (id < 0 || !primitive) return;
    const list = boundById[id];
    if (!list) return;
    for (let i = list.length - 1; i >= 0; i--) {
      const entry = list[i];
      if (entry === primitive || entry._original === primitive) {
        list.splice(i, 1);
        break;
      }
    }
    if (list.length === 0) delete boundById[id];
  }

  // ── Context factories ──
  // Each primitive gets a small context with the descriptor, the CLAP
  // id, and the gesture helpers. Centralised so the throttle / bracket
  // protocol stays consistent.
  function makeCtxForId(desc, id) {
    const syncId = syncPartnerByRateId[id];
    return {
      desc: desc,
      id: id,
      // Returns the subdivision label for `norm` when this is a rate/time
      // fader whose sync toggle is currently on; otherwise null (caller
      // falls back to the descriptor's unit display). Reads `livePlain` so
      // it tracks the toggle without re-binding.
      syncLabel: syncId == null ? null : function (norm) {
        return (livePlain[syncId] || 0) >= 0.5 ? subdivisionLabel(norm) : null;
      },
      // Cutoff "Tuned" override: when the toggle is on, the fader maps its
      // position to a semitone-snapped note (C0..C4) and reads out the note
      // name, while still storing/sending Hz so the DSP is unaffected. `null`
      // for every fader except `filter-cutoff`.
      tuned: tunedToggleByCutoffId[id] == null ? null : {
        active: function () { return (livePlain[tunedToggleByCutoffId[id]] || 0) >= 0.5; },
        toNorm: cutoffTunedHzToNorm,
        fromNorm: cutoffTunedNormToHz,
        display: cutoffTunedNoteName,
      },
      beginGesture: function () { if (id >= 0) dispatch("begin_gesture", { id: id }); },
      setNorm: function (n) { if (id >= 0) dispatch("set_param_norm", { id: id, norm: n }); },
      setParam: function (plain) { if (id >= 0) dispatch("set_param", { id: id, plain: plain }); },
      endGesture: function () { if (id >= 0) dispatch("end_gesture", { id: id }); },
      requestTextInput: function (initial) {
        if (id < 0 || !desc) return;
        const title = desc.label || desc.name || "";
        dispatchTextInput(title, initial == null ? "" : initial)
          .then(function (raw) {
            if (raw == null) return;
            const parsed = parseTextValue(desc, raw);
            if (parsed === null) return;
            const clamped = clampPlain(desc, parsed);
            dispatch("begin_gesture", { id: id });
            dispatch("set_param", { id: id, plain: clamped });
            dispatch("end_gesture", { id: id });
          });
      },
    };
  }
  function makeCtx(name) {
    const desc = resolveDesc(name);
    const id = resolveParamId(name);
    return makeCtxForId(desc, id);
  }

  // ── Bind data-vxn-param controls ──
  function bindFaders(root) {
    const faders = root.querySelectorAll(".fader[data-vxn-param]");
    for (let i = 0; i < faders.length; i++) {
      const el = faders[i];
      const name = el.getAttribute("data-vxn-param");
      const ctx = makeCtx(name);
      if (!ctx.desc) continue;
      const prim = panels.fader.create(el, ctx);
      register(ctx.id, prim, el);
    }
  }

  function bindWaveKnobs(root) {
    const knobs = root.querySelectorAll(".wave-knob[data-vxn-param]");
    for (let i = 0; i < knobs.length; i++) {
      const el = knobs[i];
      const name = el.getAttribute("data-vxn-param");
      const ctx = makeCtx(name);
      if (!ctx.desc) continue;
      const prim = panels.knob.create(el, ctx);
      register(ctx.id, prim, el);
    }
  }

  function bindButtonGroups(root) {
    const rows = root.querySelectorAll(".bgrp-row[data-vxn-param]");
    for (let i = 0; i < rows.length; i++) {
      const el = rows[i];
      const name = el.getAttribute("data-vxn-param");
      const ctx = makeCtx(name);
      if (!ctx.desc) continue;
      const prim = panels.buttonGroup.createRow(el, ctx);
      register(ctx.id, prim, el);
    }
  }

  function bindBoolToggles(root) {
    const btns = root.querySelectorAll(".bgrp-toggle[data-vxn-param]");
    for (let i = 0; i < btns.length; i++) {
      const el = btns[i];
      const name = el.getAttribute("data-vxn-param");
      const ctx = makeCtx(name);
      if (!ctx.desc) continue;
      const prim = panels.buttonGroup.createBoolToggle(el, ctx);
      register(ctx.id, prim, el);
    }
  }

  function bindToggleHeaders(root) {
    const headers = root.querySelectorAll(".panel-header.toggleable[data-vxn-param]");
    for (let i = 0; i < headers.length; i++) {
      const el = headers[i];
      const name = el.getAttribute("data-vxn-param");
      const ctx = makeCtx(name);
      if (!ctx.desc) continue;
      const prim = panels.buttonGroup.createToggleHeader(el, ctx);
      register(ctx.id, prim, el);
    }
  }

  function bindPitchEg(root) {
    const svg = root.querySelector('[data-vxn-section="peg-svg"]');
    if (!svg) return;
    const rateNames = ["peg-r1", "peg-r2", "peg-r3", "peg-r4"];
    const levelNames = ["peg-l1", "peg-l2", "peg-l3", "peg-l4"];
    const rateIds = rateNames.map(resolveParamId);
    const levelIds = levelNames.map(resolveParamId);
    const rateDescs = rateNames.map(resolveDesc);
    const levelDescs = levelNames.map(resolveDesc);
    if (rateIds.indexOf(-1) >= 0 || levelIds.indexOf(-1) >= 0) return;
    const graphCtx = {
      rateIds: rateIds, levelIds: levelIds,
      rateDescs: rateDescs, levelDescs: levelDescs,
      beginGesture: function (id) { dispatch("begin_gesture", { id: id }); },
      setNorm: function (id, n) { dispatch("set_param_norm", { id: id, norm: n }); },
      endGesture: function (id) { dispatch("end_gesture", { id: id }); },
    };
    // Use the parent .graph wrapper so the primitive can locate the svg.
    const parent = svg.closest(".graph") || svg.parentNode;
    const prim = panels.graph.create(parent, graphCtx);
    for (let i = 0; i < 4; i++) {
      register(
        rateIds[i],
        { set: (function (idx) { return function (plain) { prim.setRate(idx, plain); }; })(i) },
        parent,
      );
      register(
        levelIds[i],
        { set: (function (idx) { return function (plain) { prim.setLevel(idx, plain); }; })(i) },
        parent,
      );
    }
  }

  // ── data-vxn-custom dispatch ──
  // Most customs are thin: forward the opcode with no payload. The
  // panel-level handlers (algo picker, mod-matrix overlay, preset bar)
  // are layered in 0027 / 0028 / 0029.
  const CUSTOM_OPS = {
    "preset_prev":      "step_preset",
    "preset_next":      "step_preset",
    "open_algo_picker": "open_algo_picker",
    "close_algo_picker":"close_algo_picker",
    "open_mod_matrix":  "open_mod_matrix",
    "close_mod_matrix": "close_mod_matrix",
  };
  const CUSTOM_PAYLOAD = {
    "preset_prev": { delta: -1 },
    "preset_next": { delta:  1 },
  };
  // preset_save / preset_save_as / preset_browse are owned by the
  // preset-bar panel — it dispatches save_preset / request_text_input
  // and toggles the browse <dialog> directly.

  function bindCustoms(root) {
    const nodes = root.querySelectorAll("[data-vxn-custom]");
    for (let i = 0; i < nodes.length; i++) {
      const el = nodes[i];
      const key = el.getAttribute("data-vxn-custom");
      if (!CUSTOM_OPS[key]) continue;
      el.addEventListener("click", function (ev) {
        ev.preventDefault();
        const opcode = CUSTOM_OPS[key];
        const payload = CUSTOM_PAYLOAD[key];
        if (key === "open_algo_picker") {
          const overlay = document.querySelector('[data-vxn-section="algo-overlay"]');
          if (overlay) { overlay.removeAttribute("hidden"); overlay.classList.add("open"); }
          return;
        }
        if (key === "close_algo_picker") {
          const overlay = document.querySelector('[data-vxn-section="algo-overlay"]');
          if (overlay) { overlay.setAttribute("hidden", ""); overlay.classList.remove("open"); }
          return;
        }
        if (key === "open_mod_matrix") {
          const overlay = document.querySelector('[data-vxn-section="mod-matrix"]');
          if (overlay) { overlay.removeAttribute("hidden"); overlay.classList.add("open"); }
          // Pull a fresh snapshot — host state restore / CLAP automation
          // can move matrix rows out from under the page's local cache
          // between opens. Keeps the overlay display in lockstep.
          dispatch("request_matrix_snapshot");
          return;
        }
        if (key === "close_mod_matrix") {
          const overlay = document.querySelector('[data-vxn-section="mod-matrix"]');
          if (overlay) { overlay.setAttribute("hidden", ""); overlay.classList.remove("open"); }
          return;
        }
        dispatch(opcode, payload);
      });
    }
  }

  // ── ViewEvent dispatcher (overrides bootstrap stub) ──
  function applyEvent(ev) {
    if (!ev || !ev.kind) return;
    if (ev.kind === "param_changed") {
      livePlain[ev.id] = ev.plain;
      const bound = boundById[ev.id];
      // Notify op-row for algo tracking even if the patch-level toggle
      // handler already updated its local state.
      if (vxn._opRow) {
        const desc = vxn.params[ev.id];
        if (desc && desc.name === "algo") {
          vxn._opRow.onAlgoChanged(ev.plain);
        }
      }
      // Cutoff "Tuned" toggled: repaint the partner cutoff fader so it picks
      // up the new mode (note-name readout + semitone map, or Hz default). The
      // fader isn't bound to the toggle id, so it needs an explicit nudge.
      const cutoffId = cutoffByTunedToggleId[ev.id];
      if (cutoffId != null && boundById[cutoffId] && cutoffId in livePlain) {
        const cf = boundById[cutoffId];
        for (let i = 0; i < cf.length; i++) {
          try { cf[i].set(livePlain[cutoffId]); } catch (_) {}
        }
      }
      if (!bound) return;
      for (let i = 0; i < bound.length; i++) {
        try { bound[i].set(ev.plain, ev.norm, ev.display); }
        catch (e) { console.error("vxn2 set() failed", e, ev); }
      }
      return;
    }
    if (ev.kind === "text_input_result") {
      // The result is the *raw* string the popup committed (or null on
      // cancel). All caller-specific work — parsing, clamping, gesture
      // bracketing, save-preset dispatch — lives in the awaiter that
      // called dispatchTextInput, not here.
      resolveTextInput(ev.id, ev.value);
      return;
    }
    if (ev.kind === "preset_loaded" || ev.kind === "status") {
      if (panels.presetBar && panels.presetBar.onView) {
        panels.presetBar.onView(ev);
      }
      if (ev.kind === "preset_loaded" && panels.presetBrowser &&
          panels.presetBrowser.setCurrentSource) {
        panels.presetBrowser.setCurrentSource(ev.source || null);
      }
      return;
    }
    if (ev.kind === "preset_corpus_changed") {
      // The corpus itself rides a separate `applyPresetCorpus` call the
      // backend fires alongside this event. After a move/rename the backend
      // sets `follow` to the preset's new path; jump the browser to it.
      if (ev.follow && panels.presetBrowser && panels.presetBrowser.followPath) {
        panels.presetBrowser.followPath(ev.follow);
      }
      return;
    }
    if (ev.kind === "op_tab_changed") {
      if (vxn._opRow) vxn._opRow.onOpTabChanged(ev.op);
      return;
    }
    if (ev.kind === "matrix_snapshot") {
      if (Array.isArray(ev.rows)) vxn.matrix.rows = ev.rows.slice(0, 16);
      if (panels.modMatrix && panels.modMatrix.onSnapshot) panels.modMatrix.onSnapshot();
      return;
    }
    if (ev.kind === "ks_curve_snapshot") {
      // [[l, r]; 6] per-side KS curve discriminants. Cache for the op-row
      // graph (which may not be built yet) and repaint the live op.
      if (Array.isArray(ev.curves)) {
        vxn.ksCurves = ev.curves.map(function (s) { return [s[0] | 0, s[1] | 0]; });
      }
      if (vxn._opRow && vxn._opRow.onKsCurveSnapshot) vxn._opRow.onKsCurveSnapshot();
      return;
    }
  }

  // Clamp a parsed plain value to the descriptor's [min, max], with
  // integer rounding for stepped kinds. Mirrors `ParamDesc::clamp` on
  // the Rust side so a popup commit can't drive the engine outside
  // its declared range even if the host's automation precision differs.
  function clampPlain(desc, v) {
    if (desc.kind === "bool") return v >= 0.5 ? 1 : 0;
    if (desc.kind === "enum") {
      const n = (desc.variants && desc.variants.length) || 1;
      return Math.max(0, Math.min(n - 1, Math.round(v) | 0));
    }
    const min = typeof desc.min === "number" ? desc.min : v;
    const max = typeof desc.max === "number" ? desc.max : v;
    let clamped = Math.max(min, Math.min(max, v));
    if (desc.kind === "int") clamped = Math.round(clamped);
    return clamped;
  }

  function parseTextValue(desc, raw) {
    const s = String(raw).trim();
    if (!s) return null;
    if (desc.kind === "bool") {
      const lc = s.toLowerCase();
      if (lc === "on" || lc === "1" || lc === "true" || lc === "yes") return 1;
      if (lc === "off" || lc === "0" || lc === "false" || lc === "no") return 0;
      return null;
    }
    if (desc.kind === "enum") {
      const variants = desc.variants || [];
      const lc = s.toLowerCase();
      for (let i = 0; i < variants.length; i++) {
        if (variants[i].toLowerCase() === lc) return i;
      }
      const n = parseInt(s, 10);
      if (!isNaN(n) && n >= 0 && n < variants.length) return n;
      return null;
    }
    // float / int: strip trailing unit / whitespace.
    const cleaned = s.replace(/[^\-0-9.eE]/g, "");
    if (!cleaned) return null;
    const v = parseFloat(cleaned);
    if (isNaN(v)) return null;
    return desc.kind === "int" ? Math.round(v) : v;
  }

  vxn.applyViewEvents = function (events) {
    if (!Array.isArray(events)) return;
    for (let i = 0; i < events.length; i++) {
      applyEvent(events[i]);
    }
  };

  // Real corpus handler (overrides the bootstrap stub). The backend pushes
  // this on first flush and after every disk-mutating preset op; forward it
  // to the browser panel. Pushes that arrive before the panel binds are
  // retained inside the panel (its setCorpus stores then renders on bind).
  vxn.applyPresetCorpus = function (corpus) {
    if (panels.presetBrowser && panels.presetBrowser.setCorpus) {
      panels.presetBrowser.setCorpus(corpus);
    } else {
      vxn._lastCorpus = corpus;
    }
  };

  // ── Boot ──
  function boot() {
    const root = document;
    bindFaders(root);
    bindWaveKnobs(root);
    bindButtonGroups(root);
    bindBoolToggles(root);
    bindToggleHeaders(root);
    bindPitchEg(root);
    bindCustoms(root);

    // Op-row binding (algo picker, op tabs, op detail).
    if (panels.opRow) {
      panels.opRow.bind(root, {
        dispatch: dispatch,
        register: register,
        unregister: unregister,
        makeCtxForId: makeCtxForId,
        resolveParamId: resolveParamId,
        resolveDesc: resolveDesc,
      });
    }

    // Mod-matrix overlay binding. Reads window.__vxn.matrix directly;
    // ctx is forward-compat surface only.
    if (panels.modMatrix && panels.modMatrix.bind) {
      panels.modMatrix.bind(root, { dispatch: dispatch });
    }

    // Preset bar: name display, toast, browse <dialog>, save-as round
    // trip. Save / Save As / Browse buttons are owned by this panel
    // (removed from CUSTOM_OPS above).
    if (panels.presetBar && panels.presetBar.bind) {
      panels.presetBar.bind(root, { dispatch: dispatch });
    }

    // Preset browser: renders the corpus list into the same browse <dialog>
    // and dispatches load_factory / load_user + the user-management opcodes.
    if (panels.presetBrowser && panels.presetBrowser.bind) {
      panels.presetBrowser.bind(root, { dispatch: dispatch });
    }
    // Replay any corpus snapshot the bootstrap stub captured before bind.
    if (vxn._lastCorpus) {
      vxn.applyPresetCorpus(vxn._lastCorpus);
      vxn._lastCorpus = null;
    }

    // Flush any view events that arrived between bootstrap and main.
    const pending = vxn._pendingBatch;
    if (pending && pending.length) {
      vxn.applyViewEvents(pending);
      vxn._pendingBatch = null;
    }

    dispatch("ready");
    // Ask the engine to flip every dirty bit so the next main-thread
    // tick re-broadcasts the full param table + a fresh MatrixSnapshot.
    // The page binds primitives before this point, so the broadcast
    // populates every fader + the mod-matrix overlay without depending
    // on the initial SharedParams::new seed surviving until after bind.
    dispatch("request_full_rebroadcast");
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot, { once: true });
  } else {
    boot();
  }
})();
