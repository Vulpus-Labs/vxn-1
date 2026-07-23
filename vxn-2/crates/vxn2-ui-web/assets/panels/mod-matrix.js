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

  function buildSelect(list, idAttr) {
    var sel = document.createElement("select");
    if (idAttr) sel.dataset.field = idAttr;
    for (var i = 0; i < list.length; i++) {
      var entry = list[i];
      var opt = document.createElement("option");
      opt.value = String(entry.id);
      opt.textContent = entry.label;
      sel.appendChild(opt);
    }
    return sel;
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

      // Slot 1-8 carry the CLAP badge; slot 9-16 emit an empty placeholder
      // so the bin button below lands in the same grid column on every row.
      var badge = slot < CLAP_SLOT_COUNT
        ? el("span", { class: "vxn-mm-badge", title: "CLAP-automatable depth" }, ["automatable"])
        : el("span", { class: "vxn-mm-badge-spacer" }, []);

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
          badge,
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
      });
      destSel.addEventListener("change", function () {
        dispatchRow(slot, { dest: parseInt(destSel.value, 10) | 0 });
      });
      curveSel.addEventListener("change", function () {
        dispatchRow(slot, { curve: parseInt(curveSel.value, 10) | 0 });
      });
      scaleSel.addEventListener("change", function () {
        dispatchRow(slot, { scale: parseInt(scaleSel.value, 10) | 0 });
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

    renderAll();

    return api;
  }

  window.__vxn = window.__vxn || {};
  window.__vxn.panels = window.__vxn.panels || {};
  window.__vxn.panels.modMatrix = { bind: bind };
})();
