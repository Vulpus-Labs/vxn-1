// VXN2 op-row coordinator (ticket 0027; split into modules in 0141).
//
// Owns the active (algo, op) cursor and the four interacting widgets that
// share it. The heavy sub-widgets live in sibling modules:
//   panels/algo-data.js — ALGO_CARRIERS / ALGO_FB_OPS / OP_PARAMS / isCarrier
//   panels/ks-graph.js  — the key-scaling graph + 3-handle drag protocol
//   panels/eg-graph.js  — the envelope graph + ramp-curve toggle
//   panels/op-faders.js — the per-op fader factory + Ratio/Fixed selector
// This file keeps the cursor state, the algo picker overlay + grid, the op-tabs
// strip, the tuning column, and the op-detail layout that stitches them.
//
//   1. Algorithm picker overlay — 32 4x8 cells, each a mini algo diagram.
//      Click writes the `algo` CLAP id and closes the overlay. The overlay
//      owner (this file) wires its own open/close buttons (0141) — main.js no
//      longer touches them.
//   2. Op-row badge SVG — full algo diagram, repainted on algo / op change.
//   3. Op-tabs strip (op1..op6) — discrete view-only selection; dispatches the
//      `set_op_tab` custom UI event. Each tab's carrier/modulator badge is
//      sourced from algo-data's ALGO_CARRIERS.
//   4. Op-detail panel — 22 per-op CLAP params across four columns. Re-rendered
//      on op flip; primitives are un-registered from `boundById` before the new
//      set binds.
//
// Per [ADR 0002] dual-layer voicing is gone — every param id is flat.
(function () {
  const vxn = window.__vxn;

  function bind(root, ctx) {
    const algoData = vxn.panels.algoData;
    const isCarrier = algoData.isCarrier;
    const ALGO_FB_OPS = algoData.ALGO_FB_OPS;

    // ── State ──
    let currentOp = 1;
    let currentAlgo = 1;
    let opDetailPrims = [];
    // Live sub-widget hooks for the current op, cleared on each op-detail
    // re-render. Let a KsCurveSnapshot / EgCurveSnapshot repaint without a param.
    let ksGraphApi = null;
    let egCurveApi = null;

    const algoSvg = root.querySelector('[data-vxn-section="algo-svg"]');
    const algoNumEl = root.querySelector('[data-vxn-param="algo"]');
    const algoGrid = root.querySelector('[data-vxn-section="algo-grid"]');
    const algoOverlay = root.querySelector('[data-vxn-section="algo-overlay"]');
    const opTabsEl = root.querySelector('[data-vxn-section="op-tabs"]');
    const opDetailEl = root.querySelector('[data-vxn-section="op-detail"]');
    const fbTargetNumEl = root.querySelector('[data-vxn-section="fb-target-num"]');

    // Per-render binding context handed to the sub-widget modules. `register`
    // both registers the prim with the host echo pump and tracks it for
    // teardown on the next op-detail re-render.
    function bindCtx() {
      return {
        op: currentOp,
        vxn: vxn,
        dispatch: ctx.dispatch,
        makeCtxForId: ctx.makeCtxForId,
        register: function (id, prim, wrap) {
          ctx.register(id, prim, wrap);
          opDetailPrims.push({ id: id, prim: prim });
        },
      };
    }

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

    // The algo picker overlay owns its open/close wiring (0141). main.js's
    // bindCustoms no longer handles open_algo_picker / close_algo_picker, so
    // plain bubble-phase listeners suffice — no capture-phase race to win.
    function wireOverlayButtons() {
      const openers = root.querySelectorAll('[data-vxn-custom="open_algo_picker"]');
      for (let i = 0; i < openers.length; i++) {
        openers[i].addEventListener("click", function (ev) {
          ev.preventDefault();
          openOverlay();
        });
      }
      const closers = root.querySelectorAll('[data-vxn-custom="close_algo_picker"]');
      for (let i = 0; i < closers.length; i++) {
        closers[i].addEventListener("click", function (ev) {
          ev.preventDefault();
          closeOverlay();
        });
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

    function renderOpDetail() {
      if (!opDetailEl) return;
      clearOpDetailPrims();
      ksGraphApi = null;
      egCurveApi = null;
      opDetailEl.innerHTML = "";

      const b = bindCtx();
      const makeFader = vxn.panels.opFaders.makeFader;

      // Column 1: Tuning. Sliders on top (Hz rightmost, greyed in Ratio
      // mode); the Ratio/Fixed selector sits below them. Slider tracks are
      // tall enough (.op-tuning-row) that their bottoms line up with the
      // envelope graph's bottom in the next column. Geometry is CSS (0141).
      const col1 = document.createElement("div");
      col1.className = "op-col op-tuning op-col-tuning";
      col1.innerHTML = '<div class="op-col-title">Tuning</div>';
      const tRow = document.createElement("div");
      tRow.className = "op-col-row op-tuning-row";
      col1.appendChild(tRow);
      const numW = makeFader(tRow, "Num", "num", b);
      const denW = makeFader(tRow, "Den", "denom", b);
      const fineW = makeFader(tRow, "Fine", "fine", b);
      const centsW = makeFader(tRow, "Cents", "detune", b);
      const hzW = makeFader(tRow, "Hz", "fixed-hz", b);
      vxn.panels.opFaders.makeRatioButtonGroup(col1, {
        ratio: [hzW], // inert in Ratio mode
        fixed: [numW, denW, fineW, centsW], // inert in Fixed mode
      }, b);
      opDetailEl.appendChild(col1);

      // Column 2: EG graph + ramp-curve toggle.
      const col2 = document.createElement("div");
      col2.className = "op-col op-col-graph";
      col2.innerHTML = '<div class="op-col-title">Envelope</div>';
      vxn.panels.egGraph.createGraph(col2, b);
      egCurveApi = vxn.panels.egGraph.createCurveToggle(col2, b);
      opDetailEl.appendChild(col2);

      // Column 3: KS graph.
      const col3 = document.createElement("div");
      col3.className = "op-col op-col-graph";
      col3.innerHTML = '<div class="op-col-title">Key scaling</div>';
      ksGraphApi = vxn.panels.ksGraph.create(col3, b);
      opDetailEl.appendChild(col3);

      // Column 4: Sensitivity + Output. Geometry is CSS (0141): col4 is a row
      // of two sub-columns — Sensitivity (Vel / KsRt) and Output (Out / Pan /
      // Phase) — with Output taking the slack so its three faders fit.
      const col4 = document.createElement("div");
      col4.className = "op-col op-col-senout";
      const sens = document.createElement("div");
      sens.className = "op-col-sens";
      sens.innerHTML = '<div class="op-col-title">Sensitivity</div>';
      const sRow = document.createElement("div");
      sRow.className = "op-col-row op-col-row-start";
      sens.appendChild(sRow);
      makeFader(sRow, "Vel", "vel-sens", b);
      const ksRtW = makeFader(sRow, "KsRt", "ks-rate", b);
      if (ksRtW) {
        // KsRt scales EG *speed* (not level) and pivots independently at A3 —
        // the level graph in col 3 carries an A3 marker to make this visible.
        ksRtW.title = "Rate scaling — envelope speed, pivots A3";
      }
      col4.appendChild(sens);
      const out = document.createElement("div");
      out.className = "op-col-out";
      out.innerHTML = '<div class="op-col-title">Output</div>';
      const oRow = document.createElement("div");
      oRow.className = "op-col-row op-col-row-start";
      out.appendChild(oRow);
      makeFader(oRow, "Out", "level", b);
      // Pan is carrier-only — FM is mono in the engine, so modulator pan
      // has no audible effect (see PARAMETERS.md). Disable when the
      // current op isn't a carrier under the current algorithm.
      const panWrap = makeFader(oRow, "Pan", "pan", b);
      if (panWrap) {
        panWrap.classList.toggle("disabled", !isCarrier(currentAlgo, currentOp));
      }
      // Per-op note-on phase offset (0074): a fraction of a cycle. Shapes the
      // additive sum (algo 32) — set even harmonics to 0.5 for a saw. Inaudible
      // on a lone steady carrier (phase-deaf), so most useful on stacked/additive
      // patches; left enabled for all ops since it also affects modulator phase.
      const phaseW = makeFader(oRow, "Phase", "phase", b);
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
  };
})();
