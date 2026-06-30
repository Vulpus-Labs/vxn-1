// VXN2 op-row binder (ticket 0027).
//
// Owns four interacting widgets sharing the active (algo, op) cursor:
//   1. Algorithm picker overlay — 32 4x8 cells, each a mini algo
//      diagram. Click writes the `algo` CLAP id and closes the overlay.
//   2. Op-row badge SVG — full algo diagram rendered into the
//      always-visible block, repainted on algo / op change.
//   3. Op-tabs strip (op1..op6) — discrete view-only selection;
//      dispatches the `set_op_tab` custom UI event. Each tab carries
//      a carrier / modulator badge sourced from ALGO_CARRIERS.
//   4. Op-detail panel — 22 per-op CLAP params (num / denom /
//      fixed-hz / fine / detune / level / vel-sens / EG
//      r1..r4 + l1..l4 / KS bp+l-depth+r-depth+rate / pan).
//      Re-rendered on op flip; primitives are un-registered from
//      `boundById` before the new set binds.
//
// Per [ADR 0002] dual-layer voicing is gone — every param id is flat.
//
// ALGO_CARRIERS is the 32 x 6 boolean carrier table. Source of truth
// is `vxn2_dsp::algo::ALGOS` — drift is caught by the
// `op_row_carriers_match_engine_table` Rust test in lib.rs.

(function () {
  const vxn = window.__vxn;

  // 1-indexed op that carries the algorithm's structural feedback loop, by
  // algo number (1-indexed). Mirrors `AlgoSpec::structural_fb_op` in
  // `vxn2_dsp::algo::ALGOS`. Drift is caught by the
  // `op_row_fb_ops_match_engine_table` Rust test in lib.rs.
  const ALGO_FB_OPS = [
    6, 2, 6, 4, 6, 5, 6, 4, 2, 3, 6, 2, 6, 6, 2, 6,
    2, 3, 6, 3, 6, 6, 6, 6, 6, 6, 3, 5, 6, 5, 6, 6,
  ];

  // 32 algos x 6 ops; true if op (1-indexed) is a carrier in algo
  // (1-indexed). Mirrors the carrier mask of `vxn2_dsp::algo::ALGOS`.
  const ALGO_CARRIERS = [
    // 1:  carriers {1,3}
    [true,  false, true,  false, false, false],
    // 2:  carriers {1,3}
    [true,  false, true,  false, false, false],
    // 3:  carriers {1,4}
    [true,  false, false, true,  false, false],
    // 4:  carriers {1,4}
    [true,  false, false, true,  false, false],
    // 5:  carriers {1,3,5}
    [true,  false, true,  false, true,  false],
    // 6:  carriers {1,3,5}
    [true,  false, true,  false, true,  false],
    // 7:  carriers {1,3}
    [true,  false, true,  false, false, false],
    // 8:  carriers {1,3}
    [true,  false, true,  false, false, false],
    // 9:  carriers {1,3}
    [true,  false, true,  false, false, false],
    // 10: carriers {1,4}
    [true,  false, false, true,  false, false],
    // 11: carriers {1,4}
    [true,  false, false, true,  false, false],
    // 12: carriers {1,3}
    [true,  false, true,  false, false, false],
    // 13: carriers {1,3}
    [true,  false, true,  false, false, false],
    // 14: carriers {1,3}
    [true,  false, true,  false, false, false],
    // 15: carriers {1,3}
    [true,  false, true,  false, false, false],
    // 16: carriers {1}
    [true,  false, false, false, false, false],
    // 17: carriers {1}
    [true,  false, false, false, false, false],
    // 18: carriers {1}
    [true,  false, false, false, false, false],
    // 19: carriers {1,4,5}
    [true,  false, false, true,  true,  false],
    // 20: carriers {1,2,4}
    [true,  true,  false, true,  false, false],
    // 21: carriers {1,2,4,5}
    [true,  true,  false, true,  true,  false],
    // 22: carriers {1,3,4,5}
    [true,  false, true,  true,  true,  false],
    // 23: carriers {1,2,4,5}
    [true,  true,  false, true,  true,  false],
    // 24: carriers {1,2,3,4,5}
    [true,  true,  true,  true,  true,  false],
    // 25: carriers {1,2,3,4,5}
    [true,  true,  true,  true,  true,  false],
    // 26: carriers {1,2,4}
    [true,  true,  false, true,  false, false],
    // 27: carriers {1,2,4}
    [true,  true,  false, true,  false, false],
    // 28: carriers {1,3,6}
    [true,  false, true,  false, false, true ],
    // 29: carriers {1,2,3,5}
    [true,  true,  true,  false, true,  false],
    // 30: carriers {1,2,3,6}
    [true,  true,  true,  false, false, true ],
    // 31: carriers {1,2,3,4,5}
    [true,  true,  true,  true,  true,  false],
    // 32: carriers {1,2,3,4,5,6}
    [true,  true,  true,  true,  true,  true ],
  ];

  const OP_PARAMS = {
    num:      { kind: "fader" },
    denom:    { kind: "fader" },
    "fixed-hz":{ kind: "fader" },
    fine:     { kind: "fader" },
    detune:   { kind: "fader" },
    level:    { kind: "fader" },
    "vel-sens":{ kind: "fader" },
    pan:      { kind: "fader" },
    phase:    { kind: "fader" },
    feedback: { kind: "fader" },
    "eg-r1":  { kind: "eg-rate", idx: 0 },
    "eg-r2":  { kind: "eg-rate", idx: 1 },
    "eg-r3":  { kind: "eg-rate", idx: 2 },
    "eg-r4":  { kind: "eg-rate", idx: 3 },
    "eg-l1":  { kind: "eg-level", idx: 0 },
    "eg-l2":  { kind: "eg-level", idx: 1 },
    "eg-l3":  { kind: "eg-level", idx: 2 },
    "eg-l4":  { kind: "eg-level", idx: 3 },
    "ks-break-pt": { kind: "ks-bp" },
    "ks-l-depth":  { kind: "ks-l-depth" },
    "ks-r-depth":  { kind: "ks-r-depth" },
    "ks-rate":     { kind: "ks-rate" },
    "ratio-mode":  { kind: "button-group" },
  };

  function isCarrier(algoNum, op) {
    if (algoNum < 1 || algoNum > 32) return false;
    if (op < 1 || op > 6) return false;
    return ALGO_CARRIERS[algoNum - 1][op - 1];
  }

  function bind(root, ctx) {
    // ── State ──
    let currentOp = 1;
    let currentAlgo = 1;
    let opDetailPrims = [];
    // Live KS graph hook for the current op (set by makeKsGraph, cleared on
    // each op-detail re-render). Lets KsCurveSnapshot repaint without a param.
    let ksGraphApi = null;
    let egCurveApi = null;

    const algoSvg = root.querySelector('[data-vxn-section="algo-svg"]');
    const algoNumEl = root.querySelector('[data-vxn-param="algo"]');
    const algoGrid = root.querySelector('[data-vxn-section="algo-grid"]');
    const algoOverlay = root.querySelector('[data-vxn-section="algo-overlay"]');
    const opTabsEl = root.querySelector('[data-vxn-section="op-tabs"]');
    const opDetailEl = root.querySelector('[data-vxn-section="op-detail"]');
    const fbTargetNumEl = root.querySelector('[data-vxn-section="fb-target-num"]');

    function paintFbTarget() {
      if (fbTargetNumEl) {
        fbTargetNumEl.textContent = String(ALGO_FB_OPS[currentAlgo - 1]);
      }
    }

    function paintAlgoBadge() {
      if (algoSvg && vxn.panels.algoDiagram) {
        vxn.panels.algoDiagram.renderFull(algoSvg, currentAlgo, currentOp);
      }
      if (algoNumEl) algoNumEl.textContent = String(currentAlgo);
    }

    function wireAlgoBadgeClicks() {
      if (!algoSvg) return;
      algoSvg.addEventListener("click", function (ev) {
        const target = ev.target instanceof Element ? ev.target.closest("[data-op]") : null;
        if (!target) return;
        ev.preventDefault();
        const op = parseInt(target.getAttribute("data-op"), 10);
        if (!Number.isFinite(op) || op < 1 || op > 6 || op === currentOp) return;
        currentOp = op;
        paintOpTabs();
        paintAlgoBadge();
        renderOpDetail();
        ctx.dispatch("set_op_tab", { op: op });
      });
    }

    function paintAlgoGrid() {
      if (!algoGrid) return;
      let html = "";
      for (let n = 1; n <= 32; n++) {
        const cls = (n === currentAlgo) ? "algo-grid-cell active" : "algo-grid-cell";
        html += '<div class="' + cls + '" data-algo-pick="' + n + '">'
          + '<div class="algo-grid-num">' + n + '</div>'
          + '<svg preserveAspectRatio="xMidYMid meet"></svg>'
          + '</div>';
      }
      algoGrid.innerHTML = html;
      const cells = algoGrid.querySelectorAll("[data-algo-pick]");
      for (let i = 0; i < cells.length; i++) {
        const cell = cells[i];
        const n = parseInt(cell.getAttribute("data-algo-pick"), 10);
        const svg = cell.querySelector("svg");
        if (svg && vxn.panels.algoDiagram) {
          vxn.panels.algoDiagram.renderMini(svg, n);
        }
        cell.addEventListener("click", function (ev) {
          ev.preventDefault();
          const desc = vxn.paramsByName["algo"];
          if (!desc) return;
          currentAlgo = n;
          paintAlgoBadge();
          paintAlgoGrid();
          paintOpTabs();
          paintFbTarget();
          ctx.dispatch("set_param", { id: desc.id, plain: n });
          closeOverlay();
        });
      }
    }

    function openOverlay() {
      paintAlgoGrid();
      if (algoOverlay) {
        algoOverlay.removeAttribute("hidden");
        algoOverlay.classList.add("open");
      }
    }
    function closeOverlay() {
      if (algoOverlay) {
        algoOverlay.setAttribute("hidden", "");
        algoOverlay.classList.remove("open");
      }
    }

    function wireOverlayButtons() {
      const openers = root.querySelectorAll('[data-vxn-custom="open_algo_picker"]');
      for (let i = 0; i < openers.length; i++) {
        openers[i].addEventListener("click", function (ev) {
          ev.preventDefault();
          ev.stopImmediatePropagation();
          openOverlay();
        }, true);
      }
      const closers = root.querySelectorAll('[data-vxn-custom="close_algo_picker"]');
      for (let i = 0; i < closers.length; i++) {
        closers[i].addEventListener("click", function (ev) {
          ev.preventDefault();
          ev.stopImmediatePropagation();
          closeOverlay();
        }, true);
      }
      document.addEventListener("keydown", function (ev) {
        if (ev.key === "Escape" && algoOverlay && !algoOverlay.hasAttribute("hidden")) {
          ev.preventDefault();
          closeOverlay();
        }
      });
    }

    function paintOpTabs() {
      if (!opTabsEl) return;
      let html = "";
      for (let op = 1; op <= 6; op++) {
        const role = isCarrier(currentAlgo, op) ? "carrier" : "modulator";
        const active = (op === currentOp) ? " active" : "";
        html += '<div class="op-tab ' + role + active + '" data-op-tab="' + op + '">OP' + op + '</div>';
      }
      opTabsEl.innerHTML = html;
      const tabs = opTabsEl.querySelectorAll("[data-op-tab]");
      for (let i = 0; i < tabs.length; i++) {
        const t = tabs[i];
        t.addEventListener("click", function (ev) {
          ev.preventDefault();
          const op = parseInt(t.getAttribute("data-op-tab"), 10);
          if (op === currentOp) return;
          currentOp = op;
          paintOpTabs();
          paintAlgoBadge();
          renderOpDetail();
          ctx.dispatch("set_op_tab", { op: op });
        });
      }
    }

    function clearOpDetailPrims() {
      for (let i = 0; i < opDetailPrims.length; i++) {
        ctx.unregister(opDetailPrims[i].id, opDetailPrims[i].prim);
      }
      opDetailPrims = [];
    }

    function makeFader(parent, label, opUnprefixed) {
      const name = "op" + currentOp + "-" + opUnprefixed;
      const desc = vxn.paramsByName[name];
      if (!desc) return null;
      const wrap = document.createElement("div");
      wrap.className = "fader";
      wrap.setAttribute("data-vxn-param", name);
      wrap.innerHTML =
        '<div class="fader-label">' + label + '</div>' +
        '<div class="fader-track"><div class="fader-track-fill"></div><div class="fader-thumb"></div></div>';
      parent.appendChild(wrap);
      const localCtx = ctx.makeCtxForId(desc, desc.id);
      const prim = vxn.panels.fader.create(wrap, localCtx);
      ctx.register(desc.id, prim, wrap);
      opDetailPrims.push({ id: desc.id, prim: prim });
      return wrap;
    }

    // Ratio / Fixed tuning selector for the current op. Bound to the
    // `op{n}-ratio-mode` CLAP enum (0 = Ratio, 1 = Fixed). `faders` carries
    // the wraps to grey per mode: `.ratio` (Hz) inert in Ratio mode,
    // `.fixed` (num/den/fine/cents) inert in Fixed mode.
    function makeRatioButtonGroup(parent, faders) {
      const name = "op" + currentOp + "-ratio-mode";
      const desc = vxn.paramsByName[name];
      const cgrp = document.createElement("div");
      cgrp.className = "op-tuning-mode";
      cgrp.innerHTML =
        '<div class="bgrp"><div class="bgrp-row op-tuning-mode-row">' +
        '<button class="bgrp-btn" data-op-tuning="0">Ratio</button>' +
        '<button class="bgrp-btn" data-op-tuning="1">Fixed</button>' +
        '</div></div>';
      parent.appendChild(cgrp);
      const btns = cgrp.querySelectorAll("[data-op-tuning]");

      function apply(modeIdx) {
        for (let i = 0; i < btns.length; i++) {
          const idx = parseInt(btns[i].getAttribute("data-op-tuning"), 10);
          btns[i].classList.toggle("active", idx === modeIdx);
        }
        const fixed = modeIdx === 1;
        for (let i = 0; i < faders.ratio.length; i++) {
          if (faders.ratio[i]) faders.ratio[i].classList.toggle("disabled", !fixed);
        }
        for (let i = 0; i < faders.fixed.length; i++) {
          if (faders.fixed[i]) faders.fixed[i].classList.toggle("disabled", fixed);
        }
      }

      if (!desc) { apply(0); return; }
      const localCtx = ctx.makeCtxForId(desc, desc.id);
      for (let i = 0; i < btns.length; i++) {
        const b = btns[i];
        b.addEventListener("click", function (ev) {
          ev.preventDefault();
          const idx = parseInt(b.getAttribute("data-op-tuning"), 10);
          localCtx.setParam(idx);
          apply(idx); // optimistic; host echo confirms via the registered prim
        });
      }
      const prim = { set: function (plain) { apply(Math.round(plain) === 1 ? 1 : 0); } };
      ctx.register(desc.id, prim, cgrp);
      opDetailPrims.push({ id: desc.id, prim: prim });
      apply(Math.round(desc.default) === 1 ? 1 : 0);
    }

    function makeEgGraph(parent) {
      const rateNames = ["eg-r1", "eg-r2", "eg-r3", "eg-r4"]
        .map(function (s) { return "op" + currentOp + "-" + s; });
      const levelNames = ["eg-l1", "eg-l2", "eg-l3", "eg-l4"]
        .map(function (s) { return "op" + currentOp + "-" + s; });
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
        beginGesture: function (id) { ctx.dispatch("begin_gesture", { id: id }); },
        setNorm: function (id, n) { ctx.dispatch("set_param_norm", { id: id, norm: n }); },
        endGesture: function (id) { ctx.dispatch("end_gesture", { id: id }); },
      };
      const prim = vxn.panels.graph.create(wrap, graphCtx);
      for (let i = 0; i < 4; i++) {
        const setRate = (function (idx) { return { set: function (plain) { prim.setRate(idx, plain); } }; })(i);
        const setLevel = (function (idx) { return { set: function (plain) { prim.setLevel(idx, plain); } }; })(i);
        ctx.register(rateIds[i], setRate, wrap);
        ctx.register(levelIds[i], setLevel, wrap);
        opDetailPrims.push({ id: rateIds[i], prim: setRate });
        opDetailPrims.push({ id: levelIds[i], prim: setLevel });
      }
    }

    // Per-op EG ramp-shape toggle (ticket 0128): Exp (DX7 log/exponential) vs
    // Lin (legacy). Non-CLAP patch state — dispatched as `set_eg_curve` and
    // echoed back via an `EgCurveSnapshot` (see onEgCurveSnapshot). Mirrors the
    // KS-curve shape toggles' non-automatable opcode path.
    function makeEgCurveToggle(parent) {
      egCurveApi = null;
      const opIdx = currentOp - 1; // egCurves cache is 0-based
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
        ctx.dispatch("set_eg_curve", { op: opIdx, curve: next });
        paint();
      });
      wrap.appendChild(legend);
      wrap.appendChild(btn);
      parent.appendChild(wrap);
      paint();
      egCurveApi = { apply: paint };
    }

    function makeKsGraph(parent) {
      const bpName = "op" + currentOp + "-ks-break-pt";
      const lDepthName = "op" + currentOp + "-ks-l-depth";
      const rDepthName = "op" + currentOp + "-ks-r-depth";
      const rateName = "op" + currentOp + "-ks-rate";
      const bpDesc = vxn.paramsByName[bpName];
      const lDesc = vxn.paramsByName[lDepthName];
      const rDesc = vxn.paramsByName[rDepthName];
      const rateDesc = vxn.paramsByName[rateName];
      if (!bpDesc || !lDesc || !rDesc || !rateDesc) return;

      const wrap = document.createElement("div");
      wrap.className = "graph op-ks-graph";
      wrap.style.height = "108px";
      wrap.innerHTML =
        '<svg viewBox="0 0 240 108" preserveAspectRatio="none"></svg>' +
        '<div class="op-ks-overlay" data-ks-overlay></div>' +
        '<div class="op-ks-readout" data-ks-readout></div>' +
        '<div class="op-ks-controls">' +
          '<button type="button" class="op-ks-shape" data-ks-shape="l"></button>' +
          '<span class="op-ks-legend">LEVEL · drag ↑ boost · ↓ cut</span>' +
          '<button type="button" class="op-ks-shape" data-ks-shape="r"></button>' +
        '</div>';
      parent.appendChild(wrap);
      const svg = wrap.querySelector("svg");
      const overlay = wrap.querySelector("[data-ks-overlay]");
      const readout = wrap.querySelector("[data-ks-readout]");

      const opIdx = currentOp - 1; // snapshot/curve cache is 0-based
      let bp = bpDesc.default;
      let lDepth = lDesc.default;
      let rDepth = rDesc.default;
      // Per-side curve selectors (KsCurve discriminant: bit0 = sign,
      // 1 = boost / 0 = cut; bit1 = shape, 1 = exp / 0 = lin). Seeded from
      // the cached snapshot (default left NegLin=0, right NegExp=2) and kept
      // live by onKsCurveSnapshot.
      function cachedCurves() {
        const c = (vxn.ksCurves && vxn.ksCurves[opIdx]) || [0, 2];
        return [c[0] | 0, c[1] | 0];
      }
      let lCurve = cachedCurves()[0];
      let rCurve = cachedCurves()[1];

      let bpLineEl = null, leftPathEl = null, rightPathEl = null;
      let bpHandle = null, lHandle = null, rHandle = null;
      let lShapeBtn = null, rShapeBtn = null;
      let built = false;

      // Apply a new curve discriminant to a side: update local + shared cache,
      // repaint the shape toggles, and tell the engine (non-CLAP opcode).
      function setSideCurve(side, curve) {
        if (side === 0) lCurve = curve; else rCurve = curve;
        if (vxn.ksCurves && vxn.ksCurves[opIdx]) vxn.ksCurves[opIdx][side] = curve;
        ctx.dispatch("set_ks_curve", { op: opIdx, side: side, curve: curve });
        paintControls();
      }
      // Update the Lin/Exp toggle labels to the live shape bit.
      function paintControls() {
        if (lShapeBtn) lShapeBtn.textContent = "L " + ((lCurve & 2) ? "exp" : "lin");
        if (rShapeBtn) rShapeBtn.textContent = "R " + ((rCurve & 2) ? "exp" : "lin");
      }
      // Re-seed the curves from the shared cache (KsCurveSnapshot landed).
      function applyCurvesFromCache() {
        const c = cachedCurves();
        lCurve = c[0]; rCurve = c[1];
        paintControls();
        paint();
        if (!wrap.dataset.dragging) setReadout();
      }
      const W = 240, H = 108;
      const cy = H / 2;
      const halfH = H / 2 - 8;
      // Rate scaling pivots at A3 (MIDI 57) — independent of the level break
      // point, and hardcoded in the DSP (ks::ks_rate_mult). Drawn so the panel
      // doesn't hide the second, differently-pivoted mechanism.
      const RATE_PIVOT = 57;
      function xAt(m) { return 6 + (m / 127) * (W - 12); }
      function pctAt(m) { return (xAt(m) / W) * 100; }

      // Port of ks::ks_level_mult with the live per-side curves. The curve
      // discriminant carries sign (bit0: 1 = boost, 0 = cut) and shape
      // (bit1: 1 = exp/quadratic, 0 = lin). Boost lifts the curve above the
      // unity midline; cut drops it below.
      function curveShape(curve, t) { return (curve & 2) ? t * t : t; }
      function curveSign(curve) { return (curve & 1) ? 1.0 : -1.0; }
      function ksLevelMult(key, breakPt, lDep, rDep) {
        const semis = key - breakPt;
        const t = Math.min(Math.abs(semis) / 12.0, 4.0) / 4.0;
        let mult;
        if (semis >= 0) {
          mult = 1.0 + curveSign(rCurve) * (rDep / 99.0) * curveShape(rCurve, t);
        } else {
          mult = 1.0 + curveSign(lCurve) * (lDep / 99.0) * curveShape(lCurve, t);
        }
        return mult < 0 ? 0 : mult;
      }
      // multiplier (≈[0,2], 1 = unity at BP) → graph Y. Boost above centre,
      // cut below.
      function yAtMult(mult) { return cy - (mult - 1.0) * halfH; }
      function dbStr(mult) {
        if (mult <= 0.0001) return "−∞ dB";
        const db = 20 * Math.log10(mult);
        return (db >= 0 ? "+" : "−") + Math.abs(db).toFixed(1) + " dB";
      }

      function build() {
        let grid = "";
        for (let oct = 0; oct < 11; oct++) {
          const x = xAt(oct * 12);
          grid += '<line class="graph-grid" x1="' + x + '" y1="6" x2="' + x + '" y2="' + (H - 6) + '" />';
        }
        grid += '<line class="graph-axis" x1="6" y1="' + cy + '" x2="' + (W - 6) + '" y2="' + cy + '" />';
        const rpX = xAt(RATE_PIVOT);
        grid += '<line class="graph-rate-pivot" x1="' + rpX + '" y1="6" x2="' + rpX + '" y2="' + (H - 6) + '" />';
        svg.innerHTML =
          grid +
          '<line class="graph-bp-line" data-ks-bp-line />' +
          '<path class="graph-curve" data-ks-left />' +
          '<path class="graph-curve" data-ks-right />' +
          '<circle class="graph-handle" r="4" data-ks-pt="bp" />' +
          '<circle class="graph-handle" r="4" data-ks-pt="l" />' +
          '<circle class="graph-handle" r="4" data-ks-pt="r" />';
        bpLineEl = svg.querySelector("[data-ks-bp-line]");
        leftPathEl = svg.querySelector("[data-ks-left]");
        rightPathEl = svg.querySelector("[data-ks-right]");
        bpHandle = svg.querySelector('[data-ks-pt="bp"]');
        lHandle = svg.querySelector('[data-ks-pt="l"]');
        rHandle = svg.querySelector('[data-ks-pt="r"]');
        // Octave note-name labels (every other octave, to stay legible) plus
        // the A3 rate-pivot tag. HTML overlay so text isn't stretched by the
        // svg's non-uniform preserveAspectRatio.
        let labels = "";
        for (let oct = 0; oct < 11; oct += 2) {
          labels += '<span class="op-ks-oct" style="left:' + pctAt(oct * 12).toFixed(2) +
            '%">' + vxn.noteName(oct * 12) + "</span>";
        }
        labels += '<span class="op-ks-rate-tag" style="left:' + pctAt(RATE_PIVOT).toFixed(2) +
          '%">RATE ▸ A3</span>';
        overlay.innerHTML = labels;
        bindKsHandles();
        // Per-side Lin/Exp shape toggles (the curve's bit1). Sign is set by
        // the handle drag; shape is this explicit pick — together they cover
        // all four DX7 curves per side.
        lShapeBtn = wrap.querySelector('[data-ks-shape="l"]');
        rShapeBtn = wrap.querySelector('[data-ks-shape="r"]');
        if (lShapeBtn) lShapeBtn.addEventListener("click", function () {
          setSideCurve(0, lCurve ^ 2); paint();
        });
        if (rShapeBtn) rShapeBtn.addEventListener("click", function () {
          setSideCurve(1, rCurve ^ 2); paint();
        });
        paintControls();
        built = true;
      }

      // Sample a side of the curve into an SVG polyline path so exponential
      // (right/NegExp) shows its real bend rather than a straight chord.
      function sidePath(fromKey, toKey) {
        const step = (toKey - fromKey) / 16;
        let d = "";
        for (let i = 0; i <= 16; i++) {
          const k = fromKey + step * i;
          const x = xAt(Math.max(0, Math.min(127, k)));
          const y = yAtMult(ksLevelMult(k, bp, lDepth, rDepth));
          d += (i === 0 ? "M " : " L ") + x.toFixed(2) + " " + y.toFixed(2);
        }
        return d;
      }

      function setReadout(html) { readout.innerHTML = html || defaultReadout(); }
      function defaultReadout() { return "BP " + vxn.noteName(bp); }
      // Live drag readout: break point shows the note it lands on; the L/R
      // handles show the resulting level multiplier (dB) at the keyboard
      // extreme they govern.
      function liveReadout(which) {
        if (which === "bp") return "BP " + vxn.noteName(bp);
        if (which === "l") return "L " + dbStr(ksLevelMult(0, bp, lDepth, rDepth)) + " @ " + vxn.noteName(0);
        return "R " + dbStr(ksLevelMult(127, bp, lDepth, rDepth)) + " @ " + vxn.noteName(127);
      }

      function paint() {
        if (!built) build();
        const bpX = xAt(bp);
        bpLineEl.setAttribute("x1", bpX);
        bpLineEl.setAttribute("y1", 6);
        bpLineEl.setAttribute("x2", bpX);
        bpLineEl.setAttribute("y2", H - 6);
        leftPathEl.setAttribute("d", sidePath(bp, 0));
        rightPathEl.setAttribute("d", sidePath(bp, 127));
        bpHandle.setAttribute("cx", bpX);
        bpHandle.setAttribute("cy", cy);
        lHandle.setAttribute("cx", xAt(0));
        lHandle.setAttribute("cy", yAtMult(ksLevelMult(0, bp, lDepth, rDepth)));
        rHandle.setAttribute("cx", xAt(127));
        rHandle.setAttribute("cy", yAtMult(ksLevelMult(127, bp, lDepth, rDepth)));
      }

      function bindKsHandles() {
        const handles = svg.querySelectorAll("[data-ks-pt]");
        for (let i = 0; i < handles.length; i++) {
          const h = handles[i];
          const which = h.getAttribute("data-ks-pt");
          // bp drags horizontally (break-point note); the l/r depth handles
          // drag vertically. On the shared wireDrag primitive (0140) with the
          // per-handle value math in the callbacks: relative drag, 0.1× shift
          // (wireDrag's default) then the panel's own ×0.5 gain, the
          // `wrap.dataset.dragging` echo-gate, and per-`id` gesture brackets.
          // No rAF (these dispatch straight through) and no value-pop.
          let id = -1; // resolved in downContext; read by onUp's end_gesture
          wireDrag(h, {
            target: h,
            axis: which === "bp" ? "x" : "y",
            downContext: function () {
              let startVal;
              if (which === "bp") {
                startVal = bp; id = bpDesc.id;
              } else if (which === "l") {
                // Drag works in *signed* depth (sign = boost/cut) so the handle
                // tracks the cursor across the midline; magnitude is the depth
                // param, sign is the curve's bit0.
                startVal = (lCurve & 1 ? 1 : -1) * lDepth; id = lDesc.id;
              } else {
                startVal = (rCurve & 1 ? 1 : -1) * rDepth; id = rDesc.id;
              }
              return { startVal: startVal };
            },
          }, {
            onDown: function () {
              // Bind-helper gate: while the wrap is "dragging", the gated `set`
              // callbacks drop incoming param_changed echoes so the live drag
              // value isn't overwritten by the pump.
              wrap.dataset.dragging = "1";
              setReadout(liveReadout(which));
              ctx.dispatch("begin_gesture", { id: id });
            },
            onMove: function (_ev, info) {
              const startVal = info.ctx.startVal;
              if (which === "bp") {
                const dx = info.dx * 0.5;
                bp = Math.max(0, Math.min(127, Math.round(startVal + dx)));
                ctx.dispatch("set_param", { id: id, plain: bp });
              } else {
                // Up = boost (positive), down = cut. `signed` carries the sign;
                // crossing the midline flips the curve's sign bit (bit0) while
                // preserving its shape bit (bit1 lin/exp).
                const up = -info.dy * 0.5;
                const signed = Math.max(-99, Math.min(99, startVal + up));
                const depth = Math.round(Math.abs(signed));
                const posBit = signed >= 0 ? 1 : 0;
                if (which === "l") {
                  const nc = (lCurve & 2) | posBit;
                  if (nc !== lCurve) { setSideCurve(0, nc); }
                  lDepth = depth;
                  ctx.dispatch("set_param", { id: id, plain: lDepth });
                } else {
                  const nc = (rCurve & 2) | posBit;
                  if (nc !== rCurve) { setSideCurve(1, nc); }
                  rDepth = depth;
                  ctx.dispatch("set_param", { id: id, plain: rDepth });
                }
              }
              paint();
              setReadout(liveReadout(which));
            },
            onUp: function () {
              delete wrap.dataset.dragging;
              setReadout();
              ctx.dispatch("end_gesture", { id: id });
            },
          });
        }
      }

      const setBp = { set: function (plain) { bp = plain; paint(); if (!wrap.dataset.dragging) setReadout(); } };
      const setL = { set: function (plain) { lDepth = plain; paint(); } };
      const setR = { set: function (plain) { rDepth = plain; paint(); } };
      // Rate has its own fader (KsRt) and its A3 pivot is drawn on the graph;
      // no per-value redraw needed here.
      const setRate = { set: function (_plain) {} };
      ctx.register(bpDesc.id, setBp, wrap);
      ctx.register(lDesc.id, setL, wrap);
      ctx.register(rDesc.id, setR, wrap);
      ctx.register(rateDesc.id, setRate, wrap);
      opDetailPrims.push({ id: bpDesc.id, prim: setBp });
      opDetailPrims.push({ id: lDesc.id, prim: setL });
      opDetailPrims.push({ id: rDesc.id, prim: setR });
      opDetailPrims.push({ id: rateDesc.id, prim: setRate });

      // Expose to the snapshot dispatcher so a KsCurveSnapshot (boot,
      // preset load, host state restore) repaints the live graph.
      ksGraphApi = { applyCurves: applyCurvesFromCache };

      paint();
      setReadout();
    }

    function renderOpDetail() {
      if (!opDetailEl) return;
      clearOpDetailPrims();
      ksGraphApi = null;
      opDetailEl.innerHTML = "";

      // Column 1: Tuning. Sliders on top (Hz rightmost, greyed in Ratio
      // mode); the Ratio/Fixed selector sits below them. Slider tracks are
      // tall enough (.op-tuning-row) that their bottoms line up with the
      // envelope graph's bottom in the next column.
      const col1 = document.createElement("div");
      col1.className = "op-col op-tuning";
      col1.style.cssText = "width: 160px; flex: 0 0 160px;";
      col1.innerHTML = '<div class="op-col-title">Tuning</div>';
      const tRow = document.createElement("div");
      tRow.className = "op-col-row op-tuning-row";
      col1.appendChild(tRow);
      const numW = makeFader(tRow, "Num", "num");
      const denW = makeFader(tRow, "Den", "denom");
      const fineW = makeFader(tRow, "Fine", "fine");
      const centsW = makeFader(tRow, "Cents", "detune");
      const hzW = makeFader(tRow, "Hz", "fixed-hz");
      makeRatioButtonGroup(col1, {
        ratio: [hzW], // inert in Ratio mode
        fixed: [numW, denW, fineW, centsW], // inert in Fixed mode
      });
      opDetailEl.appendChild(col1);

      // Column 2: EG graph
      const col2 = document.createElement("div");
      col2.className = "op-col";
      col2.style.cssText = "flex: 1.4 1 0; min-width: 180px;";
      col2.innerHTML = '<div class="op-col-title">Envelope</div>';
      makeEgGraph(col2);
      makeEgCurveToggle(col2);
      opDetailEl.appendChild(col2);

      // Column 3: KS graph
      const col3 = document.createElement("div");
      col3.className = "op-col";
      col3.style.cssText = "flex: 1.4 1 0; min-width: 180px;";
      col3.innerHTML = '<div class="op-col-title">Key scaling</div>';
      makeKsGraph(col3);
      opDetailEl.appendChild(col3);

      // Column 4: Sensitivity + Output
      const col4 = document.createElement("div");
      col4.className = "op-col";
      col4.style.cssText = "width: 188px; flex: 0 0 188px; flex-direction: row; gap: 6px;";
      const sens = document.createElement("div");
      // Sensitivity holds two faders (Vel / KsRt); give the Output sub-column
      // the slack so its three faders (Out / Pan / Phase) fit without spilling
      // past the op section's right edge. col4's total width is unchanged.
      sens.style.cssText = "flex: 0 0 78px;";
      sens.innerHTML = '<div class="op-col-title">Sensitivity</div>';
      const sRow = document.createElement("div");
      sRow.className = "op-col-row";
      sRow.style.cssText = "justify-content: flex-start;";
      sens.appendChild(sRow);
      makeFader(sRow, "Vel", "vel-sens");
      const ksRtW = makeFader(sRow, "KsRt", "ks-rate");
      if (ksRtW) {
        // KsRt scales EG *speed* (not level) and pivots independently at A3 —
        // the level graph in col 3 carries an A3 marker to make this visible.
        ksRtW.title = "Rate scaling — envelope speed, pivots A3";
      }
      col4.appendChild(sens);
      const out = document.createElement("div");
      out.style.cssText = "flex: 1 1 auto;";
      out.innerHTML = '<div class="op-col-title">Output</div>';
      const oRow = document.createElement("div");
      oRow.className = "op-col-row";
      oRow.style.cssText = "justify-content: flex-start;";
      out.appendChild(oRow);
      makeFader(oRow, "Out", "level");
      // Pan is carrier-only — FM is mono in the engine, so modulator pan
      // has no audible effect (see PARAMETERS.md). Disable when the
      // current op isn't a carrier under the current algorithm.
      const panWrap = makeFader(oRow, "Pan", "pan");
      if (panWrap) {
        panWrap.classList.toggle("disabled", !isCarrier(currentAlgo, currentOp));
      }
      // Per-op note-on phase offset (0074): a fraction of a cycle. Shapes the
      // additive sum (algo 32) — set even harmonics to 0.5 for a saw. Inaudible
      // on a lone steady carrier (phase-deaf), so most useful on stacked/additive
      // patches; left enabled for all ops since it also affects modulator phase.
      const phaseW = makeFader(oRow, "Phase", "phase");
      if (phaseW) {
        phaseW.title = "Note-on phase offset (fraction of a cycle); shapes additive sums";
      }
      col4.appendChild(out);
      opDetailEl.appendChild(col4);
    }

    function onAlgoChanged(plain) {
      const n = Math.round(plain);
      if (n === currentAlgo) return;
      currentAlgo = n;
      paintAlgoBadge();
      paintAlgoGrid();
      paintOpTabs();
      paintFbTarget();
      updatePanDisabled();
    }

    function updatePanDisabled() {
      if (!opDetailEl) return;
      const panWrap = opDetailEl.querySelector(
        '[data-vxn-param="op' + currentOp + '-pan"]'
      );
      if (panWrap) {
        panWrap.classList.toggle("disabled", !isCarrier(currentAlgo, currentOp));
      }
    }

    function onOpTabChanged(op) {
      if (op === currentOp) return;
      currentOp = op;
      paintOpTabs();
      paintAlgoBadge();
      renderOpDetail();
    }

    function onKsCurveSnapshot() {
      // The cache (vxn.ksCurves) is updated by main.js before this call;
      // repaint the current op's graph if it's live.
      if (ksGraphApi) ksGraphApi.applyCurves();
    }

    function onEgCurveSnapshot() {
      // vxn.egCurves is updated by main.js before this call; repaint the live
      // op's toggle (ticket 0128).
      if (egCurveApi) egCurveApi.apply();
    }

    vxn._opRow = {
      onAlgoChanged: onAlgoChanged,
      onOpTabChanged: onOpTabChanged,
      onKsCurveSnapshot: onKsCurveSnapshot,
      onEgCurveSnapshot: onEgCurveSnapshot,
      currentAlgo: function () { return currentAlgo; },
      currentOp: function () { return currentOp; },
    };

    const algoDesc = vxn.paramsByName["algo"];
    if (algoDesc) currentAlgo = Math.round(algoDesc.default);

    paintAlgoBadge();
    paintAlgoGrid();
    paintOpTabs();
    paintFbTarget();
    renderOpDetail();
    wireOverlayButtons();
    wireAlgoBadgeClicks();
  }

  window.__vxn.panels.opRow = {
    bind: bind,
    algoCarriers: ALGO_CARRIERS,
    opParams: OP_PARAMS,
    isCarrier: isCarrier,
  };
})();
