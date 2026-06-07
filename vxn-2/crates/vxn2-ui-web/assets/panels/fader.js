// VXN2 fader primitive — vertical drag, taper-aware.
//
// Pointer-down inside .fader → begin_gesture → debounced set_param_norm
// per move (one per animation frame) → end_gesture on pointer-up. Shift
// drag = 1/10 sensitivity. Double-click pops the native text-input
// popup (handler in 0030).

(function () {
  // ── Taper math (mirror of vxn2-engine::params::taper_to/from_norm_exp) ──
  function linearToNorm(v, min, max) {
    if (max > min) {
      const t = (v - min) / (max - min);
      return t < 0 ? 0 : t > 1 ? 1 : t;
    }
    return 0;
  }
  function linearFromNorm(n, min, max) {
    const c = n < 0 ? 0 : n > 1 ? 1 : n;
    return min + c * (max - min);
  }
  function expToNorm(v, min, max, mid) {
    if (!(min > 0 && mid > min && max > mid)) {
      if (!(max > mid && mid > 0)) return linearToNorm(v, min, max);
      const r = max / mid - 1.0;
      if (r <= 0) return linearToNorm(v, min, max);
      const a = mid / (r - 1.0);
      const k = 2.0 * Math.log(r);
      if (!isFinite(k)) return linearToNorm(v, min, max);
      const t = Math.log(v / a + 1.0) / k;
      return t < 0 ? 0 : t > 1 ? 1 : t;
    }
    const cv = v < min ? min : v > max ? max : v;
    if (cv <= mid) return 0.5 * Math.log(cv / min) / Math.log(mid / min);
    return 0.5 + 0.5 * Math.log(cv / mid) / Math.log(max / mid);
  }
  function expFromNorm(n, min, max, mid) {
    const c = n < 0 ? 0 : n > 1 ? 1 : n;
    if (!(min > 0 && mid > min && max > mid)) {
      if (!(max > mid && mid > 0)) return linearFromNorm(c, min, max);
      const r = max / mid - 1.0;
      if (r <= 0) return linearFromNorm(c, min, max);
      const a = mid / (r - 1.0);
      const k = 2.0 * Math.log(r);
      if (!isFinite(k)) return linearFromNorm(c, min, max);
      return a * (Math.exp(k * c) - 1.0);
    }
    if (c <= 0.5) return min * Math.pow(mid / min, 2.0 * c);
    return mid * Math.pow(max / mid, 2.0 * c - 1.0);
  }

  function paramToNorm(desc, plain) {
    if (desc.kind === "float" && desc.taper && desc.taper.kind === "exp") {
      return expToNorm(plain, desc.min, desc.max, desc.taper.mid);
    }
    return linearToNorm(plain, desc.min, desc.max);
  }
  function normToParam(desc, norm) {
    if (desc.kind === "float" && desc.taper && desc.taper.kind === "exp") {
      return expFromNorm(norm, desc.min, desc.max, desc.taper.mid);
    }
    const v = linearFromNorm(norm, desc.min, desc.max);
    return (desc.kind === "int" || desc.kind === "bool") ? Math.round(v) : v;
  }

  // ── Display formatting (matches ParamDesc::display rules) ──
  function formatDisplay(desc, plain) {
    switch (desc.kind) {
      case "enum": {
        const i = Math.max(0, Math.min((desc.variants || []).length - 1, Math.round(plain)));
        return desc.variants && desc.variants[i] ? desc.variants[i] : String(i);
      }
      case "bool":
        return plain >= 0.5 ? "On" : "Off";
      case "int": {
        const n = Math.round(plain);
        return desc.unit ? (n + " " + desc.unit) : String(n);
      }
      case "float":
      default: {
        const u = desc.unit || "";
        return u ? (plain.toFixed(2) + " " + u) : plain.toFixed(3);
      }
    }
  }

  // ── Primitive ──
  function create(el, ctx) {
    const desc = ctx.desc;
    if (!desc) return { set: function () {} };

    const fill = el.querySelector(".fader-track-fill");
    const thumb = el.querySelector(".fader-thumb");
    const valueEl = el.querySelector(".fader-value");
    let currentPlain = desc.default;
    let currentNorm = paramToNorm(desc, currentPlain);

    function paint() {
      const pct = (currentNorm * 100).toFixed(2) + "%";
      if (fill) fill.style.height = pct;
      if (thumb) thumb.style.bottom = pct;
      if (valueEl) valueEl.textContent = formatDisplay(desc, currentPlain);
    }
    paint();

    // ── Gesture ──
    let dragging = false;
    let pointerId = null;
    let startY = 0;
    let startNorm = 0;
    let pendingNorm = null;
    let rafScheduled = false;

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

    function onPointerDown(ev) {
      if (ev.button !== undefined && ev.button !== 0) return;
      ev.preventDefault();
      dragging = true;
      pointerId = ev.pointerId;
      startY = ev.clientY;
      startNorm = currentNorm;
      el.classList.add("dragging");
      if (el.setPointerCapture && pointerId !== undefined) {
        try { el.setPointerCapture(pointerId); } catch (_) {}
      }
      ctx.beginGesture();
    }

    function onPointerMove(ev) {
      if (!dragging) return;
      ev.preventDefault();
      const dy = startY - ev.clientY;
      const range = 200; // px for full travel
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
      // Flush trailing edit unthrottled — keeps tap-and-hold crisp.
      if (pendingNorm !== null) {
        postNorm(pendingNorm);
        pendingNorm = null;
      }
      dragging = false;
      el.classList.remove("dragging");
      if (el.releasePointerCapture && pointerId !== undefined) {
        try { el.releasePointerCapture(pointerId); } catch (_) {}
      }
      ctx.endGesture();
      pointerId = null;
    }

    function onDoubleClick(ev) {
      ev.preventDefault();
      ctx.requestTextInput(formatDisplay(desc, currentPlain));
    }

    el.addEventListener("pointerdown", onPointerDown);
    el.addEventListener("pointermove", onPointerMove);
    el.addEventListener("pointerup", onPointerUp);
    el.addEventListener("pointercancel", onPointerUp);
    el.addEventListener("dblclick", onDoubleClick);

    return {
      set: function (plain) {
        if (dragging) return; // user gesture wins
        currentPlain = plain;
        currentNorm = paramToNorm(desc, plain);
        paint();
      },
    };
  }

  window.__vxn.panels.fader = {
    create: create,
    paramToNorm: paramToNorm,
    normToParam: normToParam,
    formatDisplay: formatDisplay,
  };
})();
