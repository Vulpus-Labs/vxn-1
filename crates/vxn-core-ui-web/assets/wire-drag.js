// Pointer-drag lifecycle primitive — shared by both faceplates (0140).
//
// Both synths re-implemented the pointerdown → setPointerCapture → move →
// up/cancel choreography ≥4× with observable drift (VXN1 `wireDrag`, VXN2
// `fader.create` / `fader.createBipolar` / `knob.bindDrag`, plus the op-row
// KS-handle drags). This is the one owner.
//
// `wireDrag(el, geometry, callbacks)` owns the *mechanics*: hover vs drag
// state, pointer capture, the `dragging` class, optional rAF-throttling of
// moves, an optional hit-exclude predicate (so a press on a click-to-select
// child isn't a drag), and an optional inner target element. Two value
// models share the same `onMove(ev, payload)` seam:
//
//   • ABSOLUTE — pass `pointerToValue(ev, ctx)`; `payload` is its return
//     value, verbatim. VXN1's position-mapped faders / wave knob use this;
//     the contract is unchanged from the original `wireDrag`.
//   • RELATIVE — pass `axis: 'x' | 'y'` and no `pointerToValue`; `payload`
//     is `{ dx, dy, ctx }`, the signed pixel delta since the grab point,
//     already scaled by `shift` (default 0.1) while Shift is held. VXN2's
//     accumulate-from-grab faders / knob use this.
//
// Per-control side effects (gesture brackets, the `dataset.dragging`
// echo-gate) stay in the caller's `onDown` / `onUp` — they aren't universal.
//
// ES module so the vitest suites can pull `wireDrag` in directly; the ESM
// marker is stripped at splice time (see `strip_esm_exports`).
export function wireDrag(
  el,
  {
    pointerToValue,        // (ev, ctx) => value — ABSOLUTE model; omit for RELATIVE
    downContext,           // (ev) => ctx, stashed once on pointerdown and passed to every map call
    target = el,           // element the listeners + pointer capture attach to (e.g. an inner <svg>)
    excludeHit = null,     // selector string | (ev) => bool — pointerdown on a match is ignored (click-to-select)
    axis = null,           // 'x' | 'y' — enables the RELATIVE delta payload (informational; both axes are reported)
    shift = 0.1,           // fine-drag multiplier applied to the whole delta-since-grab while Shift is held
    raf = false,           // throttle moves to one requestAnimationFrame; the trailing edit flushes on pointerup
    preventDefault = true,
  } = {},
  { onEnter, onDown, onMove, onUp, onLeave, onDoubleClick } = {},
) {
  void axis; // documents the relative model at the call site; the payload always carries both dx/dy
  let dragging = false;
  let hovered = false;
  let ctx = null;
  let startX = 0;
  let startY = 0;
  let pendingEv = null; // latest move event awaiting an rAF flush
  let rafScheduled = false;

  function hitExcluded(ev) {
    if (!excludeHit) return false;
    if (typeof excludeHit === 'function') return excludeHit(ev);
    return ev.target instanceof Element && !!ev.target.closest(excludeHit);
  }

  // RELATIVE payload: signed delta from the grab point, shift-scaled.
  function dragInfo(ev) {
    const sens = ev.shiftKey ? shift : 1;
    return { dx: (ev.clientX - startX) * sens, dy: (ev.clientY - startY) * sens, ctx };
  }
  function mapValue(ev) {
    return pointerToValue ? pointerToValue(ev, ctx) : dragInfo(ev);
  }
  function fireMove(ev) {
    if (onMove) onMove(ev, mapValue(ev));
  }
  function flush() {
    rafScheduled = false;
    if (pendingEv) {
      const ev = pendingEv;
      pendingEv = null;
      fireMove(ev);
    }
  }

  target.addEventListener('pointerenter', (ev) => {
    if (dragging) return;
    hovered = true;
    if (onEnter) onEnter(ev);
  });
  target.addEventListener('pointerleave', () => {
    hovered = false;
    if (!dragging && onLeave) onLeave();
  });
  target.addEventListener('pointerdown', (ev) => {
    if (ev.button !== undefined && ev.button !== 0) return; // primary button only
    if (hitExcluded(ev)) return;
    if (preventDefault) ev.preventDefault();
    dragging = true;
    startX = ev.clientX;
    startY = ev.clientY;
    ctx = downContext ? downContext(ev) : null;
    target.classList.add('dragging');
    if (target.setPointerCapture && ev.pointerId !== undefined) {
      try { target.setPointerCapture(ev.pointerId); } catch (_) {}
    }
    if (onDown) onDown(ev, mapValue(ev));
  });
  target.addEventListener('pointermove', (ev) => {
    if (!dragging || !onMove) return;
    if (preventDefault) ev.preventDefault();
    if (raf) {
      pendingEv = ev;
      if (!rafScheduled) {
        rafScheduled = true;
        window.requestAnimationFrame(flush);
      }
    } else {
      fireMove(ev);
    }
  });
  const end = (ev) => {
    if (!dragging) return;
    if (preventDefault) ev.preventDefault();
    // Flush the trailing throttled edit unthrottled so tap-and-release is crisp.
    if (raf && pendingEv) {
      const last = pendingEv;
      pendingEv = null;
      rafScheduled = false;
      fireMove(last);
    }
    dragging = false;
    target.classList.remove('dragging');
    if (target.releasePointerCapture && ev.pointerId !== undefined) {
      try { target.releasePointerCapture(ev.pointerId); } catch (_) {}
    }
    if (onUp) onUp(ev);
    if (!hovered && onLeave) onLeave();
  };
  target.addEventListener('pointerup', end);
  target.addEventListener('pointercancel', end);
  if (onDoubleClick) {
    target.addEventListener('dblclick', (ev) => {
      if (preventDefault) ev.preventDefault();
      onDoubleClick(ev);
    });
  }
  return {
    isDragging: () => dragging,
    isHovered: () => hovered,
  };
}
