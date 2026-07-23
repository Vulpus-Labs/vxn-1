// Mod-matrix overlay panel (ticket 0028; depth-dispatch collapsed
// by ticket 0059 / ADR 0003).
//
// Renders 16 rows × {source, dest, depth, curve, active} into the
// [data-vxn-section="mm-overlay-list"] container, wires each row's
// field changes back to the controller, and re-renders rows in
// response to matrix_snapshot (pushed by the dirty-bitset pump on the
// next tick).
//
// Depth seam (one opcode per slot range, never both — ADR 0003):
//   - Slots 1-8 depth-only edit  → `set_param { id: mtxN-depth }`
//     The CLAP id exists, host automation rides this path, and
//     `set_matrix_row_raw` on the engine side mirrors depth into the
//     CLAP `values[OFF_MTX + slot]` anyway.
//   - Slots 9-16 depth-only edit → `set_matrix_row { row }`
//     No CLAP id; depth lives in the engine's `matrix_extra_depth`.
//   - Topology edit (source, dest, curve, active) on any slot →
//     `set_matrix_row { row }` with depth riding inside `row`.
// PARAMETERS.md §"CLAP exposure" / E005.

(function () {
  var SLOT_COUNT = 16;
  var CLAP_SLOT_COUNT = 8;

  function el(tag, attrs, children) {
    var node = document.createElement(tag);
    if (attrs) {
      for (var k in attrs) {
        if (k === "class") node.className = attrs[k];
        else if (k === "dataset") {
          for (var dk in attrs[k]) node.dataset[dk] = attrs[k][dk];
        } else if (k.indexOf("on") === 0 && typeof attrs[k] === "function") {
          node.addEventListener(k.substring(2), attrs[k]);
        } else if (attrs[k] === true) {
          node.setAttribute(k, "");
        } else if (attrs[k] === false || attrs[k] == null) {
          // skip
        } else {
          node.setAttribute(k, attrs[k]);
        }
      }
    }
    if (children) {
      for (var i = 0; i < children.length; i++) {
        var c = children[i];
        if (c == null) continue;
        if (typeof c === "string") node.appendChild(document.createTextNode(c));
        else node.appendChild(c);
      }
    }
    return node;
  }

  // Display order for the dest dropdown (E022 0070): keep each
  // "opN-stack-pitch" immediately after its "opN-pitch" so the per-op vs
  // whole-stack pitch relationship reads at a glance. Option *values* stay the
  // engine wire id (entry.id) — only DOM order changes, so the route-edit
  // opcode is unaffected. Everything else keeps the engine's enum order.
  function destDisplayOrder(list) {
    var stackByOp = {};
    var rest = [];
    for (var i = 0; i < list.length; i++) {
      var e = list[i];
      var m = /^op([1-6])-stack-pitch$/.exec(e.name || "");
      if (m) stackByOp[m[1]] = e;
      else rest.push(e);
    }
    var out = [];
    for (var j = 0; j < rest.length; j++) {
      out.push(rest[j]);
      var pm = /^op([1-6])-pitch$/.exec(rest[j].name || "");
      if (pm && stackByOp[pm[1]]) out.push(stackByOp[pm[1]]);
    }
    return out;
  }

  // Custom div-based dropdown — NOT a native <select>. On macOS (WKWebView, the
  // wry backend used in-DAW) a native <select> opens an NSMenu that takes
  // first-responder from the webview; when it closes, the FIRST pointerdown
  // anywhere in the page is eaten restoring mouse tracking — so the first
  // click/drag on the amount fader right after picking a source/dest/etc. did
  // nothing. A DOM-only dropdown never spawns an NSMenu, so focus stays in the
  // webview and the next click lands. Returns an element that mimics enough of
  // the <select> surface the rest of the panel uses: a settable `.value`
  // (option id as string), a "change" event on commit, `.title`, `.dataset`,
  // `.classList`, `.blur()`, and focusability (so the activeElement repaint
  // guard still works).
  function buildSelect(list, idAttr) {
    var btn = document.createElement("div");
    btn.className = "vxn-mm-combo";
    btn.tabIndex = 0;
    if (idAttr) btn.dataset.field = idAttr;
    var labelSpan = el("span", { class: "vxn-mm-combo-label" }, []);
    var caret = el("span", { class: "vxn-mm-combo-caret" }, ["▾"]);
    btn.appendChild(labelSpan);
    btn.appendChild(caret);

    var curId = list.length ? String(list[0].id) : "0";
    function labelFor(id) {
      for (var i = 0; i < list.length; i++) {
        if (String(list[i].id) === String(id)) return list[i].label;
      }
      return "";
    }
    function render() { labelSpan.textContent = labelFor(curId); }
    render();

    Object.defineProperty(btn, "value", {
      get: function () { return curId; },
      set: function (v) { curId = String(v); render(); },
    });

    var popup = null;
    function closePopup() {
      if (popup) { popup.remove(); popup = null; }
      document.removeEventListener("mousedown", onDocDown, true);
      document.removeEventListener("keydown", onKey, true);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", closePopup, true);
      btn.classList.remove("open");
    }
    function onDocDown(e) {
      if (popup && !popup.contains(e.target) && !btn.contains(e.target)) closePopup();
    }
    // Close only when something OTHER than the popup scrolls (the panel behind
    // it) — the fixed popup would otherwise detach from its button. Scrolling
    // the popup's own overflow must NOT close it.
    function onScroll(e) {
      if (popup && !popup.contains(e.target)) closePopup();
    }
    function onKey(e) {
      if (e.key === "Escape" || e.keyCode === 27) {
        // Swallow so the first Escape dismisses only the dropdown, not the
        // whole overlay (the overlay's own keydown listener also watches Esc).
        e.preventDefault();
        e.stopPropagation();
        closePopup();
        btn.blur();
      }
    }
    function pick(entry) {
      var changed = String(entry.id) !== curId;
      curId = String(entry.id);
      render();
      closePopup();
      btn.blur();
      if (changed) btn.dispatchEvent(new Event("change"));
    }
    function openPopup() {
      popup = el("div", { class: "vxn-mm-combo-pop" }, []);
      for (var i = 0; i < list.length; i++) {
        (function (entry) {
          var opt = el("div", { class: "vxn-mm-combo-opt" }, [entry.label]);
          if (String(entry.id) === curId) opt.classList.add("sel");
          opt.addEventListener("mousedown", function (e) {
            e.preventDefault(); // keep webview focus; no text-selection
            pick(entry);
          });
          popup.appendChild(opt);
        })(list[i]);
      }
      document.body.appendChild(popup);
      var r = btn.getBoundingClientRect();
      popup.style.position = "fixed";
      popup.style.left = r.left + "px";
      popup.style.top = r.bottom + "px";
      popup.style.minWidth = r.width + "px";
      // Flip above if it would overflow the bottom of the viewport.
      var pr = popup.getBoundingClientRect();
      if (pr.bottom > window.innerHeight) {
        popup.style.top = Math.max(0, r.top - pr.height) + "px";
      }
      btn.classList.add("open");
      document.addEventListener("mousedown", onDocDown, true);
      document.addEventListener("keydown", onKey, true);
      window.addEventListener("scroll", onScroll, true);
      window.addEventListener("resize", closePopup, true);
    }
    btn.addEventListener("mousedown", function (e) {
      e.preventDefault(); // keep focus in the webview
      btn.focus();
      if (popup) closePopup();
      else openPopup();
    });
    btn.addEventListener("keydown", function (e) {
      if ((e.key === "Enter" || e.key === " " || e.keyCode === 13 || e.keyCode === 32) && !popup) {
        e.preventDefault();
        openPopup();
      }
    });
    return btn;
  }

  // Resolve "mtxN-depth" → CLAP id via the hydrated params model.
  // Returns null if not found (slot >= 8 or ParamModel not populated
  // yet); the dispatch path checks for that.
  function depthClapId(slot) {
    if (slot >= CLAP_SLOT_COUNT) return null;
    var name = "mtx" + (slot + 1) + "-depth";
    var desc = window.__vxn.paramsByName && window.__vxn.paramsByName[name];
    if (!desc || typeof desc.id !== "number") return null;
    return desc.id;
  }

  function clamp(v, lo, hi) {
    if (v < lo) return lo;
    if (v > hi) return hi;
    return v;
  }

  function bind(root, ctx) {
    var list = root.querySelector('[data-vxn-section="mm-overlay-list"]');
    if (!list) return null;
    var overlay = root.querySelector('[data-vxn-section="mod-matrix"]');
    var sourcesList = window.__vxn.matrix.sources;
    var destsList = window.__vxn.matrix.dests;
    var curvesList = window.__vxn.matrix.curves;
    // Flat coherence[srcId][dstId] verdict table exported by the engine
    // (E008 0090). The UI reads the verdict — it never re-derives the rule,
    // so engine and faceplate can't drift. "ok" (and any missing entry) is
    // coherent; the others flag the row red with a reason tooltip.
    var coherenceTable = window.__vxn.matrix.coherence || [];
    var COHERENCE_REASON = {
      "tier-collapse":
        "per-lane/-stack source can't drive a coarser target — value collapses to lane 0",
      "self-rate": "an LFO can't modulate its own rate",
      "degenerate": "voice-idx reads 0 at the collapsed lane — no effect",
    };

    // Row DOM cache keyed by slot.
    var rows = new Array(SLOT_COUNT);

    // Look up the engine's coherence verdict for a source/dest pair.
    // Empty/unknown → "ok". Empty slots (source or dest = none, id 0) are
    // never flagged — the table reports "ok" for them, but guard anyway.
    function verdictFor(source, dest) {
      if (!source || !dest) return "ok";
      var srcRow = coherenceTable[source | 0];
      if (!srcRow) return "ok";
      return srcRow[dest | 0] || "ok";
    }

    // Toggle the red-text flag + reason tooltip for a row's current pairing.
    // Reads the exported verdict (never recomputes the rule). Does not block
    // the edit — the slot still dispatches; only the visual flag changes.
    function validateRow(slot, row) {
      var r = rows[slot];
      if (!r) return;
      var reason = COHERENCE_REASON[verdictFor(row.source | 0, row.dest | 0)];
      var invalid = !!reason;
      r.node.classList.toggle("vxn-mm-invalid", invalid);
      var title = invalid ? reason : "";
      r.source.title = title;
      r.dest.title = title;
    }

    function dispatchRow(slot, partial) {
      var current = window.__vxn.matrix.rows[slot] || {
        source: 0, dest: 0, curve: 0, active: false, depth: 0.0, scale: 0,
      };
      var next = {
        source: partial.source != null ? partial.source : current.source,
        dest: partial.dest != null ? partial.dest : current.dest,
        curve: partial.curve != null ? partial.curve : current.curve,
        active: partial.active != null ? partial.active : current.active,
        depth: partial.depth != null ? partial.depth : current.depth,
        // E033 secondary scale source (VCA on depth). Topology, like curve.
        scale: partial.scale != null ? partial.scale : (current.scale || 0),
      };
      // Local optimistic update so the UI doesn't flash before the
      // pump's next-tick MatrixSnapshot lands.
      window.__vxn.matrix.rows[slot] = next;
      // Re-validate immediately on the edit (source/dest change, bin clear)
      // rather than waiting for the snapshot echo.
      validateRow(slot, next);

      var topologyChanged = partial.source != null
        || partial.dest != null
        || partial.curve != null
        || partial.active != null
        || partial.scale != null;

      if (topologyChanged) {
        // Any topology field carries the whole row (depth included).
        // For slot 1-8 the engine mirrors depth into the CLAP
        // values[OFF_MTX + slot] inside set_matrix_row_raw, so we don't
        // need a chaser set_param. (See ADR 0003 §"Removed".)
        window.__vxn.dispatch("set_matrix_row", { slot: slot, row: next });
        return;
      }

      // Depth-only edit: pick the path that matches the slot range.
      if (partial.depth != null) {
        var clapId = depthClapId(slot);
        if (clapId != null) {
          // Slot 1-8: ride the CLAP id so host automation + gesture
          // brackets + the per-id dirty bit all flow through one path.
          window.__vxn.dispatch("set_param", { id: clapId, plain: next.depth });
        } else {
          // Slot 9-16: no CLAP id — write through the custom opcode.
          window.__vxn.dispatch("set_matrix_row", { slot: slot, row: next });
        }
      }
    }

    function buildRow(slot) {
      var sourceSel = buildSelect(sourcesList, "source");
      var destSel = buildSelect(destDisplayOrder(destsList), "dest");
      var curveSel = buildSelect(curvesList, "curve");
      // E033: secondary scale source. Reuses the full source roster; index 0
      // ("—") is the None default so an unscaled slot reads as off at a glance.
      var scaleSel = buildSelect(sourcesList, "scale");
      scaleSel.classList.add("vxn-mm-scale");
      scaleSel.title = "Scale depth by (secondary source)";

      // Bipolar depth fader (E008 0096): center-tick + signed fill, value-pop
      // readout, double-click numeric entry, shift-drag fine — built on the
      // shared fader primitive (`createBipolar`) so it matches every other
      // slider. Replaces the bare `<input type=range>`.
      var depth = el("div", { class: "fader vxn-mm-depth", dataset: { field: "depth" } }, [
        el("div", { class: "fader-track" }, [
          el("div", { class: "vxn-mm-depth-center" }, []),
          el("div", { class: "fader-track-fill" }, []),
          el("div", { class: "fader-thumb" }, []),
        ]),
      ]);
      var depthFader = window.__vxn.panels.fader.createBipolar(depth, {
        value: function () {
          var cur = window.__vxn.matrix.rows[slot];
          return cur ? (+cur.depth || 0) : 0;
        },
        commit: function (d) {
          dispatchRow(slot, { depth: clamp(d, -1.0, 1.0) });
        },
        format: function (d) {
          return (d >= 0 ? "+" : "") + d.toFixed(2);
        },
        requestText: function () {
          if (!window.__vxn.dispatchTextInput) return;
          var cur = (window.__vxn.matrix.rows[slot] || {}).depth || 0;
          var initial = (cur >= 0 ? "+" : "") + (+cur).toFixed(2);
          window.__vxn.dispatchTextInput("Depth", initial).then(function (raw) {
            if (raw == null) return;
            var v = parseFloat(raw);
            if (!isFinite(v)) return;
            dispatchRow(slot, { depth: clamp(v, -1.0, 1.0) });
          });
        },
      });

      var active = el("input", {
        type: "checkbox",
        title: "Enable slot",
        "aria-label": "Enable slot",
        dataset: { field: "active" },
      });

      // CLAP-automatable slots (1-8) are grouped under the "automatable"
      // divider rather than per-row badged — see buildScaffold().
      var bin = el("button", {
        type: "button",
        class: "vxn-mm-bin",
        title: "Clear slot",
        "aria-label": "Clear slot",
      }, ["✕"]);

      var slotLabel = el("span", { class: "vxn-mm-slot-num" }, [String(slot + 1)]);

      var node = el(
        "li",
        {
          class: "vxn-mm-row",
          dataset: { slot: String(slot), clap: slot < CLAP_SLOT_COUNT ? "1" : "0" },
        },
        [
          slotLabel,
          el("label", { class: "vxn-mm-active" }, [active]),
          sourceSel,
          destSel,
          depth,
          curveSel,
          scaleSel,
          bin,
        ]
      );

      sourceSel.addEventListener("change", function () {
        var newSource = parseInt(sourceSel.value, 10) | 0;
        var current = window.__vxn.matrix.rows[slot];
        var partial = { source: newSource };
        // First-time source select: bare row sat at source=None inactive;
        // user picks a real source → auto-activate so the slot routes
        // without a second click. Only fires on the None→non-None edge so
        // a manually disabled slot stays disabled when retuned.
        if (newSource !== 0 && current && current.source === 0 && !current.active) {
          partial.active = true;
        }
        dispatchRow(slot, partial);
        // Drop focus so the next pointerdown on a sibling (e.g. the amount
        // fader) isn't swallowed dismissing the still-focused native select
        // (WebView quirk — first click was being eaten).
        sourceSel.blur();
      });
      destSel.addEventListener("change", function () {
        dispatchRow(slot, { dest: parseInt(destSel.value, 10) | 0 });
        destSel.blur();
      });
      curveSel.addEventListener("change", function () {
        dispatchRow(slot, { curve: parseInt(curveSel.value, 10) | 0 });
        curveSel.blur();
      });
      scaleSel.addEventListener("change", function () {
        dispatchRow(slot, { scale: parseInt(scaleSel.value, 10) | 0 });
        scaleSel.blur();
      });
      active.addEventListener("change", function () {
        dispatchRow(slot, { active: !!active.checked });
      });
      // Depth edits flow through the bipolar fader's `commit` callback above.
      bin.addEventListener("click", function () {
        // Clear in place: slot resets to defaults, rows below stay put.
        // Avoids silently re-binding host automation lanes that point at
        // mtxN-depth CLAP ids on slots 1-8.
        dispatchRow(slot, {
          source: 0, dest: 0, curve: 0, active: false, depth: 0.0, scale: 0,
        });
      });

      return {
        node: node,
        source: sourceSel,
        dest: destSel,
        depth: depth,
        depthFader: depthFader,
        curve: curveSel,
        scale: scaleSel,
        active: active,
      };
    }

    // Column header + group dividers. Header labels align to the row grid
    // (slot-num and bin columns are unlabelled spacers). The top-8 slots are
    // CLAP-automatable: an orange "automatable" line opens the group and a
    // plain orange line closes it, segregating slots 9-16 below.
    function buildHeader() {
      function h(txt, cls) {
        return el("span", { class: cls ? "vxn-mm-h " + cls : "vxn-mm-h" }, [txt]);
      }
      return el("div", { class: "vxn-mm-header" }, [
        // "Active" spans the slot-num + checkbox columns so the label has room
        // and sits left of "Source" (a 24px column can't hold the word).
        h("Active", "vxn-mm-h-active"),
        h("Source"),
        h("Destination"),
        h("Amount"),
        h("Scaling"),
        h("Scale By"),
        el("span", {}, []),
      ]);
    }

    function buildDivider(label) {
      if (label) {
        return el("div", { class: "vxn-mm-divider-labeled" }, [
          el("span", { class: "vxn-mm-divider-label" }, [label]),
        ]);
      }
      return el("div", { class: "vxn-mm-divider" }, []);
    }

    function buildScaffold() {
      list.appendChild(buildHeader());
      list.appendChild(buildDivider("automatable"));
      for (var i = 0; i < CLAP_SLOT_COUNT; i++) ensureRowDom(i);
      list.appendChild(buildDivider(null));
      for (var j = CLAP_SLOT_COUNT; j < SLOT_COUNT; j++) ensureRowDom(j);
    }

    function ensureRowDom(slot) {
      if (!rows[slot]) {
        rows[slot] = buildRow(slot);
        list.appendChild(rows[slot].node);
      }
      return rows[slot];
    }

    function paintRow(slot, row) {
      var r = ensureRowDom(slot);
      if (document.activeElement !== r.source) {
        r.source.value = String((row.source | 0));
      }
      if (document.activeElement !== r.dest) {
        r.dest.value = String((row.dest | 0));
      }
      if (document.activeElement !== r.curve) {
        r.curve.value = String((row.curve | 0));
      }
      if (document.activeElement !== r.scale) {
        r.scale.value = String((row.scale | 0));
      }
      // The bipolar fader's `set` no-ops while its own drag-gate is active,
      // so a snapshot echo can't stomp an in-progress depth drag.
      r.depthFader.set(clamp(+row.depth || 0, -1, 1));
      if (document.activeElement !== r.active) {
        r.active.checked = !!row.active;
      }
      r.node.dataset.active = row.active ? "1" : "0";
      // Flag incoherent routings on every repaint (snapshot echo, preset
      // load, initial renderAll) so a loaded patch shows red immediately.
      validateRow(slot, row);
    }

    function renderAll() {
      var table = window.__vxn.matrix.rows;
      for (var i = 0; i < SLOT_COUNT; i++) {
        var row = (table && table[i]) || {
          source: 0, dest: 0, curve: 0, active: false, depth: 0.0, scale: 0,
        };
        paintRow(i, row);
      }
    }

    function isOpen() {
      return overlay && !overlay.hasAttribute("hidden");
    }

    function close() {
      if (overlay) overlay.setAttribute("hidden", "");
    }

    function onKeyDown(e) {
      if (!isOpen()) return;
      if (e.key === "Escape" || e.keyCode === 27) {
        e.preventDefault();
        close();
      }
    }
    window.addEventListener("keydown", onKeyDown);

    if (overlay) {
      var backdrop = overlay.querySelector('[data-vxn-role="mm-overlay-backdrop"]');
      if (backdrop) {
        backdrop.addEventListener("mousedown", function (e) {
          if (e.target === backdrop) close();
        });
      }
      var closeBtn = overlay.querySelector('[data-vxn-role="mm-overlay-close"]');
      if (closeBtn) {
        closeBtn.addEventListener("click", function () {
          close();
        });
      }
    }

    var api = {
      renderAll: renderAll,
      onSnapshot: function () { renderAll(); },
      onRowChanged: function (slot, row) {
        paintRow(slot, row);
      },
    };

    // Expose api on the panel singleton so main.js applyEvent's
    // `panels.modMatrix.onSnapshot` / `onRowChanged` lookups resolve.
    // Without this the matrix_snapshot handler silently no-ops and the
    // overlay stays at the boot-time empty render.
    window.__vxn.panels.modMatrix.onSnapshot = api.onSnapshot;
    window.__vxn.panels.modMatrix.onRowChanged = api.onRowChanged;
    window.__vxn.panels.modMatrix.renderAll = api.renderAll;

    buildScaffold();
    renderAll();

    return api;
  }

  window.__vxn = window.__vxn || {};
  window.__vxn.panels = window.__vxn.panels || {};
  window.__vxn.panels.modMatrix = { bind: bind };
})();
