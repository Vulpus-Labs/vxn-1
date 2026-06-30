import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
// The shared drag primitive (0140), imported from the sibling crate
// (allow-listed via server.fs). VXN2's fader / dial / knob / op-row drags all
// consume the RELATIVE delta model, so this suite pins the taper math each
// relies on: shift-fine scaling, the fader/bipolar value map, and the knob's
// step quantiser — plus the rAF coalescing the faders use.
import { wireDrag } from "../../../../../crates/vxn-core-ui-web/assets/wire-drag.js";

function pointerEvt(type, { clientX = 0, clientY = 0, pointerId = 7, shiftKey = false } = {}) {
  const ev = new MouseEvent(type, { bubbles: true, cancelable: true });
  Object.defineProperty(ev, "pointerId", { value: pointerId });
  Object.defineProperty(ev, "clientX", { value: clientX });
  Object.defineProperty(ev, "clientY", { value: clientY });
  Object.defineProperty(ev, "shiftKey", { value: shiftKey });
  return ev;
}
function mountEl() {
  const el = document.createElement("div");
  document.body.appendChild(el);
  el.setPointerCapture = vi.fn();
  el.releasePointerCapture = vi.fn();
  return el;
}

describe("wireDrag — relative delta payload (fader/dial vertical)", () => {
  let el;
  beforeEach(() => { document.body.innerHTML = ""; el = mountEl(); });

  it("maps a vertical drag to startNorm − dy/range, the fader/dial contract", () => {
    const RANGE = 200;
    let norm = 0.5;
    const drag = wireDrag(el, {
      axis: "y",
      downContext: () => ({ startNorm: norm }),
    }, {
      onMove: (_ev, info) => { norm = info.ctx.startNorm - info.dy / RANGE; },
    });
    el.dispatchEvent(pointerEvt("pointerdown", { clientY: 300 }));
    el.dispatchEvent(pointerEvt("pointermove", { clientY: 200 })); // up 100 px → +0.5
    expect(norm).toBeCloseTo(1.0, 9);
    expect(drag.isDragging()).toBe(true);
  });

  it("bakes the 0.1× shift-fine factor into the delta", () => {
    const RANGE = 200;
    let norm = 0.5;
    wireDrag(el, { axis: "y", downContext: () => ({ startNorm: norm }) }, {
      onMove: (_ev, info) => { norm = info.ctx.startNorm - info.dy / RANGE; },
    });
    el.dispatchEvent(pointerEvt("pointerdown", { clientY: 300 }));
    el.dispatchEvent(pointerEvt("pointermove", { clientY: 200, shiftKey: true })); // 100 px × 0.1
    expect(norm).toBeCloseTo(0.55, 9); // +0.05 instead of +0.5
  });
});

describe("wireDrag — horizontal map (bipolar depth)", () => {
  it("maps a horizontal drag to startNorm + dx/range", () => {
    document.body.innerHTML = "";
    const el = mountEl();
    const RANGE = 200;
    let norm = 0.5;
    wireDrag(el, { axis: "x", downContext: () => ({ startNorm: norm }) }, {
      onMove: (_ev, info) => { norm = info.ctx.startNorm + info.dx / RANGE; },
    });
    el.dispatchEvent(pointerEvt("pointerdown", { clientX: 0 }));
    el.dispatchEvent(pointerEvt("pointermove", { clientX: 100 })); // right 100 px → +0.5
    expect(norm).toBeCloseTo(1.0, 9);
  });
});

describe("wireDrag — knob step quantiser (svg target, glyph exclude, 0.25 shift)", () => {
  let host, svg;
  beforeEach(() => {
    document.body.innerHTML = "";
    host = mountEl();
    svg = document.createElement("div"); // stands in for the inner <svg>
    svg.setPointerCapture = vi.fn();
    svg.releasePointerCapture = vi.fn();
    host.appendChild(svg);
  });

  it("quantises -dy to detents off the captured start index", () => {
    const DETENT = 28;
    let idx = 2;
    wireDrag(svg, {
      target: svg, axis: "y", shift: 0.25, excludeHit: "g[data-variant]",
      downContext: () => ({ startIdx: idx }),
    }, {
      onMove: (_ev, info) => { idx = info.ctx.startIdx + Math.round(-info.dy / DETENT); },
    });
    svg.dispatchEvent(pointerEvt("pointerdown", { clientY: 100 }));
    svg.dispatchEvent(pointerEvt("pointermove", { clientY: 100 - 2 * DETENT })); // up 2 detents
    expect(idx).toBe(4);
  });

  it("a press on an excluded glyph is not a drag (click-to-select survives)", () => {
    const onDown = vi.fn();
    const drag = wireDrag(svg, { target: svg, excludeHit: "g[data-variant]" }, { onDown });
    // Real DOM so `target.closest('g[data-variant]')` resolves like it does live.
    svg.innerHTML = '<g data-variant="0"><rect></rect></g><circle class="face"></circle>';
    const onGlyph = pointerEvt("pointerdown");
    Object.defineProperty(onGlyph, "target", { value: svg.querySelector("rect") });
    svg.dispatchEvent(onGlyph);
    expect(onDown).not.toHaveBeenCalled();
    expect(drag.isDragging()).toBe(false);
    // A press off the glyph (the knob face) still starts a drag.
    const onFace = pointerEvt("pointerdown");
    Object.defineProperty(onFace, "target", { value: svg.querySelector(".face") });
    svg.dispatchEvent(onFace);
    expect(onDown).toHaveBeenCalledTimes(1);
    expect(drag.isDragging()).toBe(true);
  });
});

describe("wireDrag — rAF coalescing (fader/dial throttle)", () => {
  let raf, flushRaf;
  beforeEach(() => {
    document.body.innerHTML = "";
    const queue = [];
    flushRaf = () => queue.splice(0).forEach((cb) => cb());
    raf = vi.spyOn(window, "requestAnimationFrame").mockImplementation((cb) => queue.push(cb));
  });
  afterEach(() => raf.mockRestore());

  it("coalesces a burst into one onMove with the latest delta; flushes on up before onUp", () => {
    const el = mountEl();
    const onMove = vi.fn();
    const onUp = vi.fn();
    wireDrag(el, { axis: "y", raf: true }, { onMove, onUp });
    el.dispatchEvent(pointerEvt("pointerdown", { clientY: 0 }));
    el.dispatchEvent(pointerEvt("pointermove", { clientY: -10 }));
    el.dispatchEvent(pointerEvt("pointermove", { clientY: -30 }));
    expect(onMove).not.toHaveBeenCalled();
    flushRaf();
    expect(onMove).toHaveBeenCalledTimes(1);
    expect(onMove.mock.calls[0][1].dy).toBe(-30);
    // A trailing move then an immediate up: the pending sample flushes on up.
    el.dispatchEvent(pointerEvt("pointermove", { clientY: -42 }));
    el.dispatchEvent(pointerEvt("pointerup"));
    expect(onMove).toHaveBeenCalledTimes(2);
    expect(onMove.mock.calls[1][1].dy).toBe(-42);
    expect(onMove.mock.invocationCallOrder[1]).toBeLessThan(onUp.mock.invocationCallOrder[0]);
  });
});
