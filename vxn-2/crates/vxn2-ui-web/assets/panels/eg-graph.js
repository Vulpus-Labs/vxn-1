// panels/eg-graph.js — per-op envelope graph + ramp-curve toggle (split out of
// op-row.js in ticket 0141).
//
// Two builders, both taking the op-row binding context `b`:
//   createGraph(parent, b)       — the draggable 4-segment EG graph, bound to
//                                   the op's eg-r1..r4 / eg-l1..l4 CLAP params
//                                   via the shared `vxn.panels.graph` primitive.
//   createCurveToggle(parent, b) — the Exp/Lin ramp-shape toggle (ticket 0128),
//                                   non-CLAP patch state dispatched as
//                                   `set_eg_curve`. Returns `{ apply }` so the
//                                   coordinator can repaint on an EgCurveSnapshot.
//
// `b` carries: b.op (1-indexed), b.vxn (window.__vxn), b.dispatch, and
// b.register(id, prim, wrap) which both registers the prim and tracks it for
// teardown on the next op-detail re-render.
(function () {
  window.__vxn = window.__vxn || {};
  window.__vxn.panels = window.__vxn.panels || {};

  function createGraph(parent, b) {
    const vxn = b.vxn;
    const op = b.op;
    const rateNames = ["eg-r1", "eg-r2", "eg-r3", "eg-r4"]
      .map(function (s) { return "op" + op + "-" + s; });
    const levelNames = ["eg-l1", "eg-l2", "eg-l3", "eg-l4"]
      .map(function (s) { return "op" + op + "-" + s; });
    const rateDescs = rateNames.map(function (n) { return vxn.paramsByName[n] || null; });
    const levelDescs = levelNames.map(function (n) { return vxn.paramsByName[n] || null; });
    if (rateDescs.indexOf(null) >= 0 || levelDescs.indexOf(null) >= 0) return;
    const rateIds = rateDescs.map(function (d) { return d.id; });
    const levelIds = levelDescs.map(function (d) { return d.id; });

    const wrap = document.createElement("div");
    wrap.className = "graph op-eg-graph";
    wrap.style.height = "108px";
    wrap.innerHTML = '<svg viewBox="0 0 240 108" preserveAspectRatio="none"></svg>';
    parent.appendChild(wrap);

    const graphCtx = {
      rateIds: rateIds, levelIds: levelIds,
      rateDescs: rateDescs, levelDescs: levelDescs,
      beginGesture: function (id) { b.dispatch("begin_gesture", { id: id }); },
      setNorm: function (id, n) { b.dispatch("set_param_norm", { id: id, norm: n }); },
      endGesture: function (id) { b.dispatch("end_gesture", { id: id }); },
    };
    const prim = vxn.panels.graph.create(wrap, graphCtx);
    for (let i = 0; i < 4; i++) {
      const setRate = (function (idx) { return { set: function (plain) { prim.setRate(idx, plain); } }; })(i);
      const setLevel = (function (idx) { return { set: function (plain) { prim.setLevel(idx, plain); } }; })(i);
      b.register(rateIds[i], setRate, wrap);
      b.register(levelIds[i], setLevel, wrap);
    }
  }

  // Per-op EG ramp-shape toggle (ticket 0128): Exp (DX7 log/exponential) vs
  // Lin (legacy). Non-CLAP patch state — dispatched as `set_eg_curve` and
  // echoed back via an `EgCurveSnapshot`. Mirrors the KS-curve shape toggles'
  // non-automatable opcode path. Returns `{ apply }` for the coordinator.
  function createCurveToggle(parent, b) {
    const vxn = b.vxn;
    const opIdx = b.op - 1; // egCurves cache is 0-based
    const wrap = document.createElement("div");
    wrap.className = "op-eg-curve";
    const legend = document.createElement("span");
    legend.className = "op-eg-curve-legend";
    legend.textContent = "RAMP";
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "op-eg-curve-btn";
    function cached() {
      return (vxn.egCurves && (vxn.egCurves[opIdx] | 0)) || 0;
    }
    function paint() {
      const c = cached();
      btn.textContent = c === 1 ? "lin" : "exp";
      btn.title =
        c === 1
          ? "Linear (legacy) envelope ramps"
          : "Exponential (DX7) envelope ramps";
    }
    btn.addEventListener("click", function () {
      const next = cached() === 1 ? 0 : 1;
      if (vxn.egCurves) vxn.egCurves[opIdx] = next;
      b.dispatch("set_eg_curve", { op: opIdx, curve: next });
      paint();
    });
    wrap.appendChild(legend);
    wrap.appendChild(btn);
    parent.appendChild(wrap);
    paint();
    return { apply: paint };
  }

  window.__vxn.panels.egGraph = {
    createGraph: createGraph,
    createCurveToggle: createCurveToggle,
  };
})();
