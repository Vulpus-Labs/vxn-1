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

  // ── Shared value-pop singleton ──
  let popEl = null;
  function ensurePop() {
    if (popEl) return popEl;
    popEl = document.createElement("div");
    popEl.className = "value-pop";
    document.body.appendChild(popEl);
    return popEl;
  }
  function showPop(text, x, y) {
    const el = ensurePop();
    el.textContent = text;
    el.style.left = (x + 12) + "px";
    el.style.top  = (y - 8)  + "px";
    el.style.display = "block";
  }
  function updatePop(text) { if (popEl) popEl.textContent = text; }
  function hidePop() { if (popEl) popEl.style.display = "none"; }

  // ── Primitive ──
  function create(el, ctx) {
    const desc = ctx.desc;
    if (!desc) return { set: function () {} };

    const fill = el.querySelector(".fader-track-fill");
    const thumb = el.querySelector(".fader-thumb");

    // ── Cutoff "Tuned" override (E007 / VXN-1 parity) ──
    // When `ctx.tuned` is present and active, the fader maps its position to a
    // semitone-snapped note instead of the descriptor taper, and stores Hz via
    // `set_param` (not the normalised position) so the DSP/automation see the
    // same Hz value as untuned. Plain faders leave `ctx.tuned` null → no-op.
    function tunedOn() { return !!(ctx.tuned && ctx.tuned.active()); }
    function plainToNormM(plain) {
      return tunedOn() ? ctx.tuned.toNorm(plain) : paramToNorm(desc, plain);
    }

    let currentPlain = desc.default;
    let currentNorm = plainToNormM(currentPlain);
    let hovered = false;

    // Display text for the value popup. When this is a synced rate/time
    // fader (ctx.syncLabel returns a label) the readout is the subdivision
    // its position selects — computed locally so it walks the divisions live
    // during a drag, when no engine echo arrives. Otherwise unit-formatted.
    function displayText() {
      if (tunedOn()) return ctx.tuned.display(currentPlain);
      if (ctx.syncLabel) {
        const lbl = ctx.syncLabel(currentNorm);
        if (lbl != null) return lbl;
      }
      return formatDisplay(desc, currentPlain);
    }

    function paint() {
      const pct = (currentNorm * 100).toFixed(2) + "%";
      if (fill) fill.style.height = pct;
      if (thumb) thumb.style.bottom = pct;
      if (hovered || dragging) {
        updatePop(displayText());
      }
    }

    // ── Gesture ──
    let dragging = false;
    let pointerId = null;
    let startY = 0;
    let startNorm = 0;
    let pendingNorm = null;
    let rafScheduled = false;

    paint();

    function postNorm(n) {
      const clamped = n < 0 ? 0 : n > 1 ? 1 : n;
      if (tunedOn()) {
        // Tuned: snap to a semitone, send the resulting Hz as a plain value
        // (not the raw norm — the engine's exp-Hz taper would un-snap it), and
        // re-derive the norm so the thumb lands on the semitone.
        const plain = ctx.tuned.fromNorm(clamped);
        ctx.setParam(plain);
        currentPlain = plain;
        currentNorm = ctx.tuned.toNorm(plain);
      } else {
        ctx.setNorm(clamped);
        currentNorm = clamped;
        currentPlain = normToParam(desc, clamped);
      }
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
      // Bind-helper gate (ADR 0003 / 0060): `bindGestureGated` reads
      // this dataset flag to drop incoming param_changed echoes for
      // the duration of the drag — the page is the source of truth.
      el.dataset.dragging = "1";
      if (el.setPointerCapture && pointerId !== undefined) {
        try { el.setPointerCapture(pointerId); } catch (_) {}
      }
      showPop(displayText(), ev.clientX, ev.clientY);
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
      delete el.dataset.dragging;
      if (el.releasePointerCapture && pointerId !== undefined) {
        try { el.releasePointerCapture(pointerId); } catch (_) {}
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
        if (dragging) return; // user gesture wins
        currentPlain = plain;
        currentNorm = plainToNormM(plain);
        paint();
      },
    };
  }

  // ── Bipolar variant (E008 0096) ──
  // A center-origin fader for a raw bipolar value in `[-1, 1]` (0 = no
  // modulation) — the mod-matrix depth control. Shares the value-pop
  // singleton and the same RAF-throttle / shift-fine / drag-gate idiom as
  // `create`, but drags HORIZONTALLY (left = -1, right = +1), is value-based
  // (no param descriptor / taper), and fills from the center toward the thumb.
  // Kept a thin sibling of `create` rather than a flag on it so the
  // descriptor-bound param path stays untouched. `ctx`:
  //   value():  current depth `[-1, 1]`
  //   commit(d): dispatch a new depth (does the optimistic update)
  //   format(d): readout string (e.g. "+0.42")
  //   requestText(): open numeric entry (double-click)
  function createBipolar(el, ctx) {
    const fill = el.querySelector(".fader-track-fill");
    const thumb = el.querySelector(".fader-thumb");

    function clampDepth(d) { return d < -1 ? -1 : d > 1 ? 1 : d; }
    function depthToNorm(d) { return (d + 1) * 0.5; }
    function normToDepth(n) { return n * 2 - 1; }

    let current = clampDepth(ctx.value());
    let hovered = false;
    let dragging = false;
    let pointerId = null;
    let startX = 0;
    let startNorm = 0;
    let pendingNorm = null;
    let rafScheduled = false;

    function paint() {
      const norm = depthToNorm(current);
      const pct = norm * 100;
      // Signed fill grown horizontally from the 50% center toward the thumb.
      if (fill) {
        if (current >= 0) {
          fill.style.left = "50%";
          fill.style.width = (pct - 50) + "%";
        } else {
          fill.style.left = pct + "%";
          fill.style.width = (50 - pct) + "%";
        }
      }
      if (thumb) thumb.style.left = pct + "%";
      if (hovered || dragging) updatePop(ctx.format(current));
    }
    paint();

    function postNorm(n) {
      const clamped = n < 0 ? 0 : n > 1 ? 1 : n;
      current = normToDepth(clamped);
      ctx.commit(current);
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
      if (!dragging) showPop(ctx.format(current), ev.clientX, ev.clientY);
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
      startX = ev.clientX;
      startNorm = depthToNorm(current);
      el.classList.add("dragging");
      // Drag-gate flag the matrix repaint honours so a snapshot echo can't
      // stomp an in-progress drag.
      el.dataset.dragging = "1";
      if (el.setPointerCapture && pointerId !== undefined) {
        try { el.setPointerCapture(pointerId); } catch (_) {}
      }
      showPop(ctx.format(current), ev.clientX, ev.clientY);
    }
    function onPointerMove(ev) {
      if (!dragging) return;
      ev.preventDefault();
      const dx = ev.clientX - startX;
      const range = 200; // px for full -1..+1 travel
      const sens = ev.shiftKey ? 0.1 : 1.0;
      const next = startNorm + (dx / range) * sens;
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
      if (el.releasePointerCapture && pointerId !== undefined) {
        try { el.releasePointerCapture(pointerId); } catch (_) {}
      }
      if (!hovered) hidePop();
      pointerId = null;
    }
    function onDoubleClick(ev) {
      ev.preventDefault();
      ctx.requestText();
    }

    el.addEventListener("pointerenter", onPointerEnter);
    el.addEventListener("pointerleave", onPointerLeave);
    el.addEventListener("pointerdown", onPointerDown);
    el.addEventListener("pointermove", onPointerMove);
    el.addEventListener("pointerup", onPointerUp);
    el.addEventListener("pointercancel", onPointerUp);
    el.addEventListener("dblclick", onDoubleClick);

    return {
      set: function (depth) {
        if (dragging) return; // gesture / drag-gate wins over echoes
        current = clampDepth(depth);
        paint();
      },
      isDragging: function () { return dragging; },
    };
  }

  window.__vxn.panels.fader = {
    create: create,
    createBipolar: createBipolar,
    paramToNorm: paramToNorm,
    normToParam: normToParam,
    formatDisplay: formatDisplay,
  };
})();
