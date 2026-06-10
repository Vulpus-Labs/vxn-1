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

    function makeRatioButtonGroup(parent) {
      const cgrp = document.createElement("div");
      cgrp.className = "cgrp";
      cgrp.innerHTML =
        '<div class="bgrp"><div class="bgrp-row"><button class="bgrp-btn active" data-op-tuning="ratio">Ratio</button><button class="bgrp-btn" data-op-tuning="fixed">Fixed</button></div></div>';
      parent.appendChild(cgrp);
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
      wrap.innerHTML = '<svg viewBox="0 0 240 108" preserveAspectRatio="none"></svg>';
      parent.appendChild(wrap);
      const svg = wrap.querySelector("svg");

      let bp = bpDesc.default;
      let lDepth = lDesc.default;
      let rDepth = rDesc.default;

      let bpLineEl = null, leftPathEl = null, rightPathEl = null;
      let bpHandle = null, lHandle = null, rHandle = null;
      let built = false;
      const W = 240, H = 108;
      const cy = H / 2;
      function xAt(m) { return 6 + (m / 127) * (W - 12); }

      function build() {
        let grid = "";
        for (let oct = 0; oct < 11; oct++) {
          const x = xAt(oct * 12);
          grid += '<line class="graph-grid" x1="' + x + '" y1="6" x2="' + x + '" y2="' + (H - 6) + '" />';
        }
        grid += '<line class="graph-axis" x1="6" y1="' + cy + '" x2="' + (W - 6) + '" y2="' + cy + '" />';
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
        bindKsHandles();
        built = true;
      }

      function paint() {
        if (!built) build();
        const bpX = xAt(bp);
        const lEndX = xAt(0);
        const lEndY = cy + (lDepth / 99) * (H / 2 - 8);
        const rEndX = xAt(127);
        const rEndY = cy - (rDepth / 99) * (H / 2 - 8);

        bpLineEl.setAttribute("x1", bpX);
        bpLineEl.setAttribute("y1", 6);
        bpLineEl.setAttribute("x2", bpX);
        bpLineEl.setAttribute("y2", H - 6);
        leftPathEl.setAttribute("d", "M " + bpX + " " + cy + " L " + lEndX + " " + lEndY);
        rightPathEl.setAttribute("d", "M " + bpX + " " + cy + " L " + rEndX + " " + rEndY);
        bpHandle.setAttribute("cx", bpX);
        bpHandle.setAttribute("cy", cy);
        lHandle.setAttribute("cx", lEndX);
        lHandle.setAttribute("cy", lEndY);
        rHandle.setAttribute("cx", rEndX);
        rHandle.setAttribute("cy", rEndY);
      }

      function bindKsHandles() {
        const handles = svg.querySelectorAll("[data-ks-pt]");
        for (let i = 0; i < handles.length; i++) {
          const h = handles[i];
          const which = h.getAttribute("data-ks-pt");
          let dragging = false;
          let startX = 0, startY = 0, startVal = 0;
          let id = -1;
          h.addEventListener("pointerdown", function (ev) {
            ev.preventDefault();
            dragging = true;
            startX = ev.clientX;
            startY = ev.clientY;
            if (which === "bp") {
              startVal = bp; id = bpDesc.id;
            } else if (which === "l") {
              startVal = lDepth; id = lDesc.id;
            } else {
              startVal = rDepth; id = rDesc.id;
            }
            if (h.setPointerCapture) {
              try { h.setPointerCapture(ev.pointerId); } catch (_) {}
            }
            // Bind-helper gate: while the wrap is "dragging", the
            // gated `set` callbacks drop incoming param_changed echoes
            // so the live drag value isn't overwritten by the pump.
            wrap.dataset.dragging = "1";
            ctx.dispatch("begin_gesture", { id: id });
          });
          h.addEventListener("pointermove", function (ev) {
            if (!dragging) return;
            ev.preventDefault();
            const sens = ev.shiftKey ? 0.1 : 1.0;
            if (which === "bp") {
              const dx = (ev.clientX - startX) * sens * 0.5;
              bp = Math.max(0, Math.min(127, Math.round(startVal + dx)));
              ctx.dispatch("set_param", { id: id, plain: bp });
            } else if (which === "l") {
              const dy = (ev.clientY - startY) * sens * 0.5;
              lDepth = Math.max(0, Math.min(99, Math.round(startVal + dy)));
              ctx.dispatch("set_param", { id: id, plain: lDepth });
            } else {
              const dy = (startY - ev.clientY) * sens * 0.5;
              rDepth = Math.max(0, Math.min(99, Math.round(startVal + dy)));
              ctx.dispatch("set_param", { id: id, plain: rDepth });
            }
            paint();
          });
          function up(ev) {
            if (!dragging) return;
            ev.preventDefault();
            dragging = false;
            if (h.releasePointerCapture) {
              try { h.releasePointerCapture(ev.pointerId); } catch (_) {}
            }
            delete wrap.dataset.dragging;
            ctx.dispatch("end_gesture", { id: id });
          }
          h.addEventListener("pointerup", up);
          h.addEventListener("pointercancel", up);
        }
      }

      const setBp = { set: function (plain) { bp = plain; paint(); } };
      const setL = { set: function (plain) { lDepth = plain; paint(); } };
      const setR = { set: function (plain) { rDepth = plain; paint(); } };
      const setRate = { set: function (_plain) { /* rate has no visual; numeric label could surface */ } };
      ctx.register(bpDesc.id, setBp, wrap);
      ctx.register(lDesc.id, setL, wrap);
      ctx.register(rDesc.id, setR, wrap);
      ctx.register(rateDesc.id, setRate, wrap);
      opDetailPrims.push({ id: bpDesc.id, prim: setBp });
      opDetailPrims.push({ id: lDesc.id, prim: setL });
      opDetailPrims.push({ id: rDesc.id, prim: setR });
      opDetailPrims.push({ id: rateDesc.id, prim: setRate });

      paint();
    }

    function renderOpDetail() {
      if (!opDetailEl) return;
      clearOpDetailPrims();
      opDetailEl.innerHTML = "";

      // Column 1: Tuning
      const col1 = document.createElement("div");
      col1.className = "op-col";
      col1.style.cssText = "width: 160px; flex: 0 0 160px;";
      col1.innerHTML = '<div class="op-col-title">Tuning</div>';
      makeRatioButtonGroup(col1);
      const tRow = document.createElement("div");
      tRow.className = "op-col-row";
      col1.appendChild(tRow);
      makeFader(tRow, "Num", "num");
      makeFader(tRow, "Den", "denom");
      makeFader(tRow, "Hz", "fixed-hz");
      makeFader(tRow, "Fine", "fine");
      makeFader(tRow, "Cents", "detune");
      opDetailEl.appendChild(col1);

      // Column 2: EG graph
      const col2 = document.createElement("div");
      col2.className = "op-col";
      col2.style.cssText = "flex: 1.4 1 0; min-width: 180px;";
      col2.innerHTML = '<div class="op-col-title">Envelope</div>';
      makeEgGraph(col2);
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
      sens.style.cssText = "flex: 0 0 104px;";
      sens.innerHTML = '<div class="op-col-title">Sensitivity</div>';
      const sRow = document.createElement("div");
      sRow.className = "op-col-row";
      sRow.style.cssText = "justify-content: flex-start;";
      sens.appendChild(sRow);
      makeFader(sRow, "Vel", "vel-sens");
      makeFader(sRow, "KsRt", "ks-rate");
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

    vxn._opRow = {
      onAlgoChanged: onAlgoChanged,
      onOpTabChanged: onOpTabChanged,
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
