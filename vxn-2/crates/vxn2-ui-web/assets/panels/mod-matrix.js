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

    // Row DOM cache keyed by slot.
    var rows = new Array(SLOT_COUNT);

    function dispatchRow(slot, partial) {
      var current = window.__vxn.matrix.rows[slot] || {
        source: 0, dest: 0, curve: 0, active: false, depth: 0.0,
      };
      var next = {
        source: partial.source != null ? partial.source : current.source,
        dest: partial.dest != null ? partial.dest : current.dest,
        curve: partial.curve != null ? partial.curve : current.curve,
        active: partial.active != null ? partial.active : current.active,
        depth: partial.depth != null ? partial.depth : current.depth,
      };
      // Local optimistic update so the UI doesn't flash before the
      // pump's next-tick MatrixSnapshot lands.
      window.__vxn.matrix.rows[slot] = next;

      var topologyChanged = partial.source != null
        || partial.dest != null
        || partial.curve != null
        || partial.active != null;

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
      var destSel = buildSelect(destsList, "dest");
      var curveSel = buildSelect(curvesList, "curve");

      var depth = el("input", {
        type: "range",
        min: "-1",
        max: "1",
        step: "0.001",
        value: "0",
        dataset: { field: "depth" },
      });

      var active = el("input", {
        type: "checkbox",
        dataset: { field: "active" },
      });

      var badge = slot < CLAP_SLOT_COUNT
        ? el("span", { class: "vxn-mm-badge", title: "CLAP-automatable depth" }, ["automatable"])
        : null;

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
          badge,
        ]
      );

      sourceSel.addEventListener("change", function () {
        dispatchRow(slot, { source: parseInt(sourceSel.value, 10) | 0 });
      });
      destSel.addEventListener("change", function () {
        dispatchRow(slot, { dest: parseInt(destSel.value, 10) | 0 });
      });
      curveSel.addEventListener("change", function () {
        dispatchRow(slot, { curve: parseInt(curveSel.value, 10) | 0 });
      });
      active.addEventListener("change", function () {
        dispatchRow(slot, { active: !!active.checked });
      });
      depth.addEventListener("input", function () {
        var v = parseFloat(depth.value);
        if (!isFinite(v)) v = 0.0;
        dispatchRow(slot, { depth: clamp(v, -1.0, 1.0) });
      });

      return {
        node: node,
        source: sourceSel,
        dest: destSel,
        depth: depth,
        curve: curveSel,
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
      if (document.activeElement !== r.depth) {
        r.depth.value = String(clamp(+row.depth || 0, -1, 1));
      }
      if (document.activeElement !== r.active) {
        r.active.checked = !!row.active;
      }
      r.node.dataset.active = row.active ? "1" : "0";
    }

    function renderAll() {
      var table = window.__vxn.matrix.rows;
      for (var i = 0; i < SLOT_COUNT; i++) {
        var row = (table && table[i]) || {
          source: 0, dest: 0, curve: 0, active: false, depth: 0.0,
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
