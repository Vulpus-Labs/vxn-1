// panels/ks-graph.js — per-op key-scaling graph + drag protocol (split out of
// op-row.js in ticket 0141).
//
// `create(parent, b)` builds the KS level graph for one operator and wires its
// three handles (break-point, left-depth, right-depth) plus the two Lin/Exp
// shape toggles. Returns `{ applyCurves }` so the op-row coordinator can repaint
// it when a `KsCurveSnapshot` lands, or null if the op's KS params are missing.
//
// The per-render binding context `b` carries:
//   b.op        — 1-indexed current op
//   b.vxn       — window.__vxn (paramsByName / ksCurves / noteName)
//   b.dispatch  — (opcode, payload) -> void
//   b.register  — (id, prim, wrap) -> void; registers the prim AND tracks it
//                 for teardown on the next op-detail re-render
//
// The drag math (relative delta, 0.1x shift, panel ×0.5 gain, dragging-gate,
// per-id gesture brackets) rides the shared `wireDrag` primitive (0140).
(function () {
  window.__vxn = window.__vxn || {};
  window.__vxn.panels = window.__vxn.panels || {};

  function create(parent, b) {
    const vxn = b.vxn;
    const op = b.op;
    const bpName = "op" + op + "-ks-break-pt";
    const lDepthName = "op" + op + "-ks-l-depth";
    const rDepthName = "op" + op + "-ks-r-depth";
    const rateName = "op" + op + "-ks-rate";
    const bpDesc = vxn.paramsByName[bpName];
    const lDesc = vxn.paramsByName[lDepthName];
    const rDesc = vxn.paramsByName[rDepthName];
    const rateDesc = vxn.paramsByName[rateName];
    if (!bpDesc || !lDesc || !rDesc || !rateDesc) return null;

    const wrap = document.createElement("div");
    wrap.className = "graph op-ks-graph";
    wrap.style.height = "108px";
    wrap.innerHTML =
      '<svg viewBox="0 0 240 108" preserveAspectRatio="none"></svg>' +
      '<div class="op-ks-readout" data-ks-readout></div>' +
      '<div class="op-ks-controls">' +
        '<button type="button" class="op-ks-shape" data-ks-shape="l"></button>' +
        '<span class="op-ks-legend">LEVEL · drag ↑ boost · ↓ cut</span>' +
        '<button type="button" class="op-ks-shape" data-ks-shape="r"></button>' +
      '</div>';
    parent.appendChild(wrap);
    const svg = wrap.querySelector("svg");
    const readout = wrap.querySelector("[data-ks-readout]");

    const opIdx = op - 1; // snapshot/curve cache is 0-based
    let bp = bpDesc.default;
    let lDepth = lDesc.default;
    let rDepth = rDesc.default;
    // Per-side curve selectors (KsCurve discriminant: bit0 = sign,
    // 1 = boost / 0 = cut; bit1 = shape, 1 = exp / 0 = lin). Seeded from
    // the cached snapshot (default left NegLin=0, right NegExp=2) and kept
    // live by applyCurvesFromCache.
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
      b.dispatch("set_ks_curve", { op: opIdx, side: side, curve: curve });
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
            b.dispatch("begin_gesture", { id: id });
          },
          onMove: function (_ev, info) {
            const startVal = info.ctx.startVal;
            if (which === "bp") {
              const dx = info.dx * 0.5;
              bp = Math.max(0, Math.min(127, Math.round(startVal + dx)));
              b.dispatch("set_param", { id: id, plain: bp });
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
                b.dispatch("set_param", { id: id, plain: lDepth });
              } else {
                const nc = (rCurve & 2) | posBit;
                if (nc !== rCurve) { setSideCurve(1, nc); }
                rDepth = depth;
                b.dispatch("set_param", { id: id, plain: rDepth });
              }
            }
            paint();
            setReadout(liveReadout(which));
          },
          onUp: function () {
            delete wrap.dataset.dragging;
            setReadout();
            b.dispatch("end_gesture", { id: id });
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
    b.register(bpDesc.id, setBp, wrap);
    b.register(lDesc.id, setL, wrap);
    b.register(rDesc.id, setR, wrap);
    b.register(rateDesc.id, setRate, wrap);

    paint();
    setReadout();

    // Returned to the op-row coordinator so a KsCurveSnapshot (boot, preset
    // load, host state restore) repaints this live graph.
    return { applyCurves: applyCurvesFromCache };
  }

  window.__vxn.panels.ksGraph = { create: create };
})();
