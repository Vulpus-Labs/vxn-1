// VXN2 ADSR / pitch-EG graph primitive.
//
// Lands the segment-graph widget — 4 (rate, level) handles, draggable
// in 2D, batched gesture per handle drag. 0026 binds the Pitch-EG
// instance (`peg-r1..r4`, `peg-l1..l4`). Op-detail EG / KS bindings
// land in 0027 — they reuse the same primitive with different
// (rateIds, levelIds) tuples.
//
// Drag protocol:
//   pointerdown on a handle → begin_gesture on (rateId, levelId)
//   pointermove → set_param_norm on both, throttled per animation frame
//   pointerup   → end_gesture on both
//
// Y-axis drag changes level; X-axis drag changes rate. Shift = 1/10
// sensitivity.

(function () {
  const fader = window.__vxn.panels.fader;

  function clamp01(x) { return x < 0 ? 0 : x > 1 ? 1 : x; }

  // ─ Segment-graph layout (mirror of mockup drawEgGraph) ─
  function layout(W, H, rateNorms) {
    const widths = rateNorms.map(function (rn) { return (1.0 - rn) * 100 + 10; });
    const total = widths.reduce(function (a, b) { return a + b; }, 0);
    const xs = [0];
    widths.forEach(function (w) { xs.push(xs[xs.length - 1] + w); });
    const scale = (W - 12) / total;
    return xs.map(function (x) { return 6 + x * scale; });
  }

  function levelToY(levelNorm, H) {
    return (H - 6) - clamp01(levelNorm) * (H - 14);
  }

  function create(el, ctx) {
    const svg = el.querySelector("svg");
    if (!svg) return { set: function () {} };

    const rateIds = ctx.rateIds || [];          // [r1, r2, r3, r4] CLAP ids
    const levelIds = ctx.levelIds || [];        // [l1, l2, l3, l4] CLAP ids
    const rateDescs = ctx.rateDescs || [];
    const levelDescs = ctx.levelDescs || [];

    const rateNorms = rateDescs.map(function (d) { return d ? fader.paramToNorm(d, d.default) : 0; });
    const levelNorms = levelDescs.map(function (d) { return d ? fader.paramToNorm(d, d.default) : 0; });

    function viewBox() {
      const vb = svg.getAttribute("viewBox") || "0 0 200 90";
      const parts = vb.split(/\s+/).map(parseFloat);
      return { x: parts[0] || 0, y: parts[1] || 0, w: parts[2] || 200, h: parts[3] || 90 };
    }

    // SVG skeleton (grid + axis + path + handles) is built once; subsequent
    // paint()s only update attributes. Rewriting innerHTML mid-drag would
    // destroy the pointer-captured handle and kill the gesture.
    let pathEl = null;
    let handleEls = [];
    let built = false;

    function build() {
      const { w: W, h: H } = viewBox();
      let grid = "";
      for (let i = 1; i < 4; i++) {
        const y = 6 + i * (H - 12) / 4;
        grid += '<line class="graph-grid" x1="6" y1="' + y.toFixed(2) + '" x2="' + (W - 6).toFixed(2) + '" y2="' + y.toFixed(2) + '" />';
      }
      grid += '<line class="graph-axis" x1="6" y1="' + (H - 6).toFixed(2) + '" x2="' + (W - 6).toFixed(2) + '" y2="' + (H - 6).toFixed(2) + '" />';

      let handles = "";
      for (let i = 0; i < 4; i++) {
        handles += '<circle class="graph-handle" r="3" data-eg-pt="' + i + '" />';
      }

      svg.innerHTML = grid + '<path class="graph-curve" d="" />' + handles;
      pathEl = svg.querySelector(".graph-curve");
      handleEls = svg.querySelectorAll("[data-eg-pt]");
      bindHandles();
      built = true;
    }

    function paint() {
      if (!built) build();
      const { w: W, h: H } = viewBox();
      const ptsX = layout(W, H, rateNorms);
      // The displayed curve uses 4 points starting from the floor (L4),
      // ramping through L1, L2, L3 over R1..R3, then dropping back to L4
      // over R4. For Pitch-EG we approximate floor as 0.5 (centre line).
      const floor = 0.5;
      const ptsY = [
        levelToY(floor, H),
        levelToY(levelNorms[0] !== undefined ? levelNorms[0] : floor, H),
        levelToY(levelNorms[1] !== undefined ? levelNorms[1] : floor, H),
        levelToY(levelNorms[2] !== undefined ? levelNorms[2] : floor, H),
        levelToY(levelNorms[3] !== undefined ? levelNorms[3] : floor, H),
      ];

      let path = "M " + ptsX[0].toFixed(2) + " " + ptsY[0].toFixed(2);
      for (let i = 1; i < ptsY.length; i++) {
        path += " L " + ptsX[i].toFixed(2) + " " + ptsY[i].toFixed(2);
      }
      pathEl.setAttribute("d", path);
      for (let i = 0; i < handleEls.length; i++) {
        handleEls[i].setAttribute("cx", ptsX[i + 1].toFixed(2));
        handleEls[i].setAttribute("cy", ptsY[i + 1].toFixed(2));
      }
    }

    // ─ Per-handle drag ─
    let activeIdx = -1;
    let startClientX = 0, startClientY = 0;
    let startRateNorm = 0, startLevelNorm = 0;
    let pendingRate = null, pendingLevel = null;
    let raf = false;

    function flush() {
      raf = false;
      if (pendingRate !== null) {
        ctx.setNorm(rateIds[activeIdx], pendingRate);
        rateNorms[activeIdx] = pendingRate;
        pendingRate = null;
      }
      if (pendingLevel !== null) {
        ctx.setNorm(levelIds[activeIdx], pendingLevel);
        levelNorms[activeIdx] = pendingLevel;
        pendingLevel = null;
      }
      paint();
    }

    function onDown(handleEl, ev) {
      ev.preventDefault();
      const idx = parseInt(handleEl.getAttribute("data-eg-pt"), 10);
      if (isNaN(idx)) return;
      activeIdx = idx;
      startClientX = ev.clientX;
      startClientY = ev.clientY;
      startRateNorm = rateNorms[idx] || 0;
      startLevelNorm = levelNorms[idx] !== undefined ? levelNorms[idx] : 0;
      if (handleEl.setPointerCapture) {
        try { handleEl.setPointerCapture(ev.pointerId); } catch (_) {}
      }
      ctx.beginGesture(rateIds[idx]);
      ctx.beginGesture(levelIds[idx]);
    }

    function onMove(ev) {
      if (activeIdx < 0) return;
      ev.preventDefault();
      const sens = ev.shiftKey ? 0.1 : 1.0;
      // Rate is "speed" — higher rate ⇒ shorter segment ⇒ handle further
      // LEFT. So dragging right must lower the rate to make the handle
      // track the cursor.
      const dx = (ev.clientX - startClientX) / 200 * sens;
      const dy = (startClientY - ev.clientY) / 200 * sens;
      pendingRate = clamp01(startRateNorm - dx);
      pendingLevel = clamp01(startLevelNorm + dy);
      if (!raf) {
        raf = true;
        window.requestAnimationFrame(flush);
      }
    }

    function onUp(ev) {
      if (activeIdx < 0) return;
      ev.preventDefault();
      if (pendingRate !== null) {
        ctx.setNorm(rateIds[activeIdx], pendingRate);
        rateNorms[activeIdx] = pendingRate;
        pendingRate = null;
      }
      if (pendingLevel !== null) {
        ctx.setNorm(levelIds[activeIdx], pendingLevel);
        levelNorms[activeIdx] = pendingLevel;
        pendingLevel = null;
      }
      ctx.endGesture(rateIds[activeIdx]);
      ctx.endGesture(levelIds[activeIdx]);
      activeIdx = -1;
      paint();
    }

    function bindHandles() {
      const handles = svg.querySelectorAll("[data-eg-pt]");
      for (let i = 0; i < handles.length; i++) {
        const h = handles[i];
        h.addEventListener("pointerdown", function (ev) { onDown(h, ev); });
        h.addEventListener("pointermove", onMove);
        h.addEventListener("pointerup", onUp);
        h.addEventListener("pointercancel", onUp);
      }
    }

    paint();

    return {
      setRate: function (idx, plain) {
        if (activeIdx === idx) return;
        const d = rateDescs[idx];
        if (!d) return;
        rateNorms[idx] = fader.paramToNorm(d, plain);
        paint();
      },
      setLevel: function (idx, plain) {
        if (activeIdx === idx) return;
        const d = levelDescs[idx];
        if (!d) return;
        levelNorms[idx] = fader.paramToNorm(d, plain);
        paint();
      },
    };
  }

  window.__vxn.panels.graph = { create: create };
})();
