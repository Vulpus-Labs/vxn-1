// VXN2 generic rotary dial — small SVG knob for continuous float / int params.
//
// Vertical drag → throttled `set_norm` per animation frame → `end_gesture` on
// pointer-up. Shift drag = 1/10 sensitivity. Double-click pops the native
// numeric-entry popup. Shares the value-pop singleton + taper math with
// `panels/fader.js`; the dial is the same gesture / echo contract in a
// rotary shape, used where panel space is tight (E028 Dynamics pane).
//
// Bound via `.dial[data-vxn-param]` markup; `main.js`'s `bindDials` constructs
// it through `window.__vxn.panels.dial.create(el, ctx)`.

(function () {
  // The shared value-pop lives in fader.js (it's a single DOM element appended
  // to body). We re-implement the same trio here so dial can stand on its own
  // load order, but read/write the same `.value-pop` node so a fader and a
  // dial don't fight over two singletons.
  let popEl = null;
  function ensurePop() {
    if (popEl) return popEl;
    popEl = document.querySelector(".value-pop");
    if (!popEl) {
      popEl = document.createElement("div");
      popEl.className = "value-pop";
      document.body.appendChild(popEl);
    }
    return popEl;
  }
  function showPop(text, x, y) {
    const el = ensurePop();
    el.textContent = text;
    el.style.left = (x + 12) + "px";
    el.style.top = (y - 8) + "px";
    el.style.display = "block";
  }
  function updatePop(text) { if (popEl) popEl.textContent = text; }
  function hidePop() { if (popEl) popEl.style.display = "none"; }

  // Reuse the fader's taper math so a dial-bound param and a fader-bound param
  // round-trip the same way through the host. The fader panel registers itself
  // on `window.__vxn.panels.fader` (load order is bootstrap → knob → fader →
  // ... in lib.rs), so by the time a `.dial` is bound the helpers are present.
  function paramToNorm(desc, plain) {
    const fader = window.__vxn && window.__vxn.panels && window.__vxn.panels.fader;
    return fader ? fader.paramToNorm(desc, plain) : 0;
  }
  function normToParam(desc, norm) {
    const fader = window.__vxn && window.__vxn.panels && window.__vxn.panels.fader;
    return fader ? fader.normToParam(desc, norm) : 0;
  }
  function formatDisplay(desc, plain) {
    const fader = window.__vxn && window.__vxn.panels && window.__vxn.panels.fader;
    return fader ? fader.formatDisplay(desc, plain) : String(plain);
  }

  // 270° arc, indicator points up at norm=0.5 (12 o'clock).
  const ARC_START_DEG = -135;
  const ARC_SWEEP_DEG = 270;
  function normToDeg(norm) {
    return ARC_START_DEG + norm * ARC_SWEEP_DEG;
  }

  // SVG geometry. Small footprint: 36 px square, dial radius 12, indicator
  // line from centre to just inside the rim. The arc lives outside the dial
  // body so the indicator overprints cleanly.
  const SIZE = 36;
  const CX = SIZE / 2;
  const CY = SIZE / 2;
  const DIAL_R = 12;
  const ARC_R = 15;

  function describeArc(cx, cy, r, startDeg, endDeg) {
    const s = (startDeg - 90) * Math.PI / 180;
    const e = (endDeg - 90) * Math.PI / 180;
    const sx = cx + r * Math.cos(s);
    const sy = cy + r * Math.sin(s);
    const ex = cx + r * Math.cos(e);
    const ey = cy + r * Math.sin(e);
    const large = (endDeg - startDeg) <= 180 ? 0 : 1;
    return "M " + sx.toFixed(2) + " " + sy.toFixed(2)
         + " A " + r + " " + r + " 0 " + large + " 1 "
         + ex.toFixed(2) + " " + ey.toFixed(2);
  }

  function render(el, label) {
    const trackArc = describeArc(CX, CY, ARC_R, ARC_START_DEG, ARC_START_DEG + ARC_SWEEP_DEG);
    let html = "";
    if (label) html += '<div class="dial-label">' + label + "</div>";
    html += '<svg class="dial-svg" width="' + SIZE + '" height="' + SIZE
         + '" viewBox="0 0 ' + SIZE + " " + SIZE + '">';
    html += '<path class="dial-track" d="' + trackArc + '" />';
    html += '<path class="dial-fill" d="' + trackArc + '" />';
    html += '<circle class="dial-face" cx="' + CX + '" cy="' + CY + '" r="' + DIAL_R + '" />';
    html += '<g class="dial-indicator-g" transform="rotate(0 ' + CX + " " + CY + ')">';
    html += '<line class="dial-indicator-line" x1="' + CX + '" y1="' + CY
         + '" x2="' + CX + '" y2="' + (CY - DIAL_R + 1) + '" />';
    html += "</g></svg>";
    el.innerHTML = html;
  }

  function labelOf(el, desc) {
    const explicit = el.getAttribute("data-label");
    if (explicit) return explicit;
    // Fall back to the short half of the descriptor's display name ("Dyn
    // Threshold" → "Threshold", "Dyn Mix" → "Mix"). Authors can override
    // entirely with `data-label`.
    if (desc && desc.label) {
      const parts = desc.label.split(" ");
      return parts.length > 1 ? parts.slice(1).join(" ") : desc.label;
    }
    return "";
  }

  function create(el, ctx) {
    const desc = ctx.desc;
    if (!desc) return { set: function () {} };

    render(el, labelOf(el, desc));

    const fillPath = el.querySelector(".dial-fill");
    const indicatorG = el.querySelector(".dial-indicator-g");
    const svg = el.querySelector("svg");

    let currentPlain = desc.default;
    let currentNorm = paramToNorm(desc, currentPlain);
    let hovered = false;

    function displayText() { return formatDisplay(desc, currentPlain); }

    function paint() {
      if (indicatorG) {
        indicatorG.setAttribute(
          "transform",
          "rotate(" + normToDeg(currentNorm).toFixed(2) + " " + CX + " " + CY + ")"
        );
      }
      if (fillPath) {
        // Re-draw the fill arc up to the current norm; cap at >= ~0 so a
        // norm of 0 still renders a usable zero-length stub (no NaN path).
        const sweep = Math.max(0.001, currentNorm) * ARC_SWEEP_DEG;
        fillPath.setAttribute(
          "d",
          describeArc(CX, CY, ARC_R, ARC_START_DEG, ARC_START_DEG + sweep)
        );
      }
      if (hovered || dragging) updatePop(displayText());
    }

    let dragging = false;
    let pointerId = null;
    let startY = 0;
    let startNorm = 0;
    let pendingNorm = null;
    let rafScheduled = false;

    paint();

    function postNorm(n) {
      const clamped = n < 0 ? 0 : n > 1 ? 1 : n;
      ctx.setNorm(clamped);
      currentNorm = clamped;
      currentPlain = normToParam(desc, clamped);
      paint();
    }
    function flushPending() {
      rafScheduled = false;
      if (pendingNorm !== null) {
        postNorm(pendingNorm);
        pendingNorm = null;
      }
    }

    function onPointerEnter(ev) {
      hovered = true;
      if (!dragging) showPop(displayText(), ev.clientX, ev.clientY);
    }
    function onPointerLeave() {
      hovered = false;
      if (!dragging) hidePop();
    }
    function onPointerDown(ev) {
      if (ev.button !== undefined && ev.button !== 0) return;
      ev.preventDefault();
      dragging = true;
      pointerId = ev.pointerId;
      startY = ev.clientY;
      startNorm = currentNorm;
      el.classList.add("dragging");
      el.dataset.dragging = "1";
      if (svg && svg.setPointerCapture && pointerId !== undefined) {
        try { svg.setPointerCapture(pointerId); } catch (_) {}
      }
      showPop(displayText(), ev.clientX, ev.clientY);
      ctx.beginGesture();
    }
    function onPointerMove(ev) {
      if (!dragging) return;
      ev.preventDefault();
      const dy = startY - ev.clientY;
      const range = 200; // same per-norm travel as the fader (200 px = full).
      const sens = ev.shiftKey ? 0.1 : 1.0;
      const next = startNorm + (dy / range) * sens;
      pendingNorm = next < 0 ? 0 : next > 1 ? 1 : next;
      if (!rafScheduled) {
        rafScheduled = true;
        window.requestAnimationFrame(flushPending);
      }
    }
    function onPointerUp(ev) {
      if (!dragging) return;
      ev.preventDefault();
      if (pendingNorm !== null) {
        postNorm(pendingNorm);
        pendingNorm = null;
      }
      dragging = false;
      el.classList.remove("dragging");
      delete el.dataset.dragging;
      if (svg && svg.releasePointerCapture && pointerId !== undefined) {
        try { svg.releasePointerCapture(pointerId); } catch (_) {}
      }
      if (!hovered) hidePop();
      ctx.endGesture();
      pointerId = null;
    }
    function onDoubleClick(ev) {
      ev.preventDefault();
      ctx.requestTextInput(formatDisplay(desc, currentPlain));
    }

    el.addEventListener("pointerenter", onPointerEnter);
    el.addEventListener("pointerleave", onPointerLeave);
    el.addEventListener("pointerdown", onPointerDown);
    el.addEventListener("pointermove", onPointerMove);
    el.addEventListener("pointerup", onPointerUp);
    el.addEventListener("pointercancel", onPointerUp);
    el.addEventListener("dblclick", onDoubleClick);

    return {
      set: function (plain) {
        if (dragging) return; // user gesture wins over host echoes
        currentPlain = plain;
        currentNorm = paramToNorm(desc, plain);
        paint();
      },
    };
  }

  window.__vxn = window.__vxn || {};
  window.__vxn.panels = window.__vxn.panels || {};
  window.__vxn.panels.dial = { create: create };
})();
