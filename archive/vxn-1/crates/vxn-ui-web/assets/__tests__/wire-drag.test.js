import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { wireDrag } from '../panels.js';
import { mountEl, pointerEvt } from './_helpers.js';

// `wireFaderDrag` (a thin wrapper around `wireDrag`) gets its own focused
// coverage in wire-fader-drag.test.js. This suite exercises the
// generalisation: the delta-based `pointerToValue` path that the wave knob
// relies on, and the hover-during-drag suppression contract shared by both.

describe('wireDrag — delta-based map (wave-knob style)', () => {
  let el, onDown, onMove, onUp, downContext, pointerToValue, drag;

  beforeEach(() => {
    document.body.innerHTML = '';
    el = mountEl();
    onDown = vi.fn();
    onMove = vi.fn();
    onUp   = vi.fn();
    // Wave knob's actual shape: ctx captures start state; pointerToValue
    // reads ctx every move. PIXELS_PER_DETENT = 30 elsewhere — use 10 here
    // for shorter test arithmetic.
    downContext = vi.fn((ev) => ({ y0: ev.clientY, v0: 2 }));
    pointerToValue = vi.fn((ev, ctx) => ctx.v0 + (ctx.y0 - ev.clientY) / 10);
    drag = wireDrag(el, { downContext, pointerToValue },
      { onDown, onMove, onUp });
  });

  it('downContext fires once on pointerdown; the returned ctx is passed to pointerToValue', () => {
    el.dispatchEvent(pointerEvt('pointerdown', { clientY: 50 }));
    expect(downContext).toHaveBeenCalledTimes(1);
    expect(pointerToValue).toHaveBeenCalledTimes(1);
    expect(pointerToValue.mock.calls[0][1]).toEqual({ y0: 50, v0: 2 });
    // Initial pointerToValue at down-time: dy = 0 → unchanged.
    expect(onDown.mock.calls[0][1]).toBe(2);
  });

  it('pointermove uses the captured ctx, not a fresh one', () => {
    el.dispatchEvent(pointerEvt('pointerdown', { clientY: 50 }));
    downContext.mockClear();
    el.dispatchEvent(pointerEvt('pointermove', { clientY: 20 })); // dy=30 → +3
    el.dispatchEvent(pointerEvt('pointermove', { clientY: 80 })); // dy=-30 → -3
    expect(downContext).not.toHaveBeenCalled();
    expect(onMove).toHaveBeenCalledTimes(2);
    expect(onMove.mock.calls[0][1]).toBe(5);
    expect(onMove.mock.calls[1][1]).toBe(-1);
  });

  it('passes pointerToValue\'s return value through verbatim — wireDrag does not interpret it', () => {
    // Make pointerToValue return a sentinel object; the helper should not
    // mutate or unwrap it.
    const sentinel = { tag: 'opaque' };
    pointerToValue.mockReturnValue(sentinel);
    el.dispatchEvent(pointerEvt('pointerdown', { clientY: 0 }));
    expect(onDown.mock.calls[0][1]).toBe(sentinel);
    el.dispatchEvent(pointerEvt('pointermove', { clientY: 5 }));
    expect(onMove.mock.calls[0][1]).toBe(sentinel);
  });

  it('downContext is optional — drags with no start-state work too', () => {
    document.body.innerHTML = '';
    const el2 = mountEl();
    const ptv = vi.fn(() => 7);
    const onDown2 = vi.fn();
    wireDrag(el2, { pointerToValue: ptv }, { onDown: onDown2 });
    el2.dispatchEvent(pointerEvt('pointerdown', { clientY: 0 }));
    expect(onDown2).toHaveBeenCalledWith(expect.anything(), 7);
    // ctx arg is null when no downContext is provided.
    expect(ptv.mock.calls[0][1]).toBeNull();
  });

  it('pointer capture + dragging class lifecycle is identical to the fader contract', () => {
    el.dispatchEvent(pointerEvt('pointerdown', { clientY: 0, pointerId: 11 }));
    expect(el.setPointerCapture).toHaveBeenCalledWith(11);
    expect(el.classList.contains('dragging')).toBe(true);
    el.dispatchEvent(pointerEvt('pointerup', { pointerId: 11 }));
    expect(el.releasePointerCapture).toHaveBeenCalledWith(11);
    expect(el.classList.contains('dragging')).toBe(false);
    expect(onUp).toHaveBeenCalledTimes(1);
  });

  it('pointercancel ends the drag like pointerup', () => {
    el.dispatchEvent(pointerEvt('pointerdown', { clientY: 0 }));
    el.dispatchEvent(pointerEvt('pointercancel'));
    expect(onUp).toHaveBeenCalledTimes(1);
    expect(drag.isDragging()).toBe(false);
  });
});

describe('wireDrag — hover-during-drag suppression', () => {
  let el, onEnter, onLeave;

  beforeEach(() => {
    document.body.innerHTML = '';
    el = mountEl();
    onEnter = vi.fn();
    onLeave = vi.fn();
    wireDrag(el, { pointerToValue: () => 0 }, { onEnter, onLeave });
  });

  it('onEnter fires when not dragging', () => {
    el.dispatchEvent(pointerEvt('pointerenter'));
    expect(onEnter).toHaveBeenCalledTimes(1);
  });

  it('onEnter is suppressed while dragging', () => {
    el.dispatchEvent(pointerEvt('pointerdown'));
    onEnter.mockClear();
    el.dispatchEvent(pointerEvt('pointerenter'));
    expect(onEnter).not.toHaveBeenCalled();
  });

  it('onLeave is deferred until drag-end when pointer leaves mid-drag', () => {
    el.dispatchEvent(pointerEvt('pointerenter'));
    el.dispatchEvent(pointerEvt('pointerdown'));
    el.dispatchEvent(pointerEvt('pointerleave'));
    expect(onLeave).not.toHaveBeenCalled();
    el.dispatchEvent(pointerEvt('pointerup'));
    expect(onLeave).toHaveBeenCalledTimes(1);
  });

  it('onLeave does not double-fire when still hovering at drag-end', () => {
    el.dispatchEvent(pointerEvt('pointerenter'));
    el.dispatchEvent(pointerEvt('pointerdown'));
    el.dispatchEvent(pointerEvt('pointerup'));
    expect(onLeave).not.toHaveBeenCalled();
  });
});

// 0140: the RELATIVE delta model VXN2's faders / knob use. With no
// `pointerToValue`, onMove receives `{ dx, dy, ctx }` — the signed pixel
// delta from the grab point, scaled by `shift` while Shift is held.
describe('wireDrag — relative delta + shift-fine taper', () => {
  let el, onMove;

  beforeEach(() => {
    document.body.innerHTML = '';
    el = mountEl();
    onMove = vi.fn();
    wireDrag(el, { axis: 'y' }, { onMove });
  });

  it('reports the signed delta from the grab point (full sensitivity)', () => {
    el.dispatchEvent(pointerEvt('pointerdown', { clientX: 100, clientY: 200 }));
    el.dispatchEvent(pointerEvt('pointermove', { clientX: 130, clientY: 160 }));
    expect(onMove.mock.calls[0][1]).toEqual({ dx: 30, dy: -40, ctx: null });
  });

  it('scales the whole delta-since-grab by the default 0.1 while Shift is held', () => {
    el.dispatchEvent(pointerEvt('pointerdown', { clientX: 0, clientY: 100 }));
    const ev = pointerEvt('pointermove', { clientX: 0, clientY: 0 }); // dy = -100
    Object.defineProperty(ev, 'shiftKey', { value: true });
    el.dispatchEvent(ev);
    expect(onMove.mock.calls[0][1].dy).toBeCloseTo(-10, 9); // -100 × 0.1
  });

  it('honours a per-call shift override (knob keeps 0.25)', () => {
    document.body.innerHTML = '';
    const el2 = mountEl();
    const mv = vi.fn();
    wireDrag(el2, { axis: 'y', shift: 0.25 }, { onMove: mv });
    el2.dispatchEvent(pointerEvt('pointerdown', { clientX: 0, clientY: 100 }));
    const ev = pointerEvt('pointermove', { clientX: 0, clientY: 0 }); // dy = -100
    Object.defineProperty(ev, 'shiftKey', { value: true });
    el2.dispatchEvent(ev);
    expect(mv.mock.calls[0][1].dy).toBeCloseTo(-25, 9); // -100 × 0.25
  });

  it('downContext is stashed and threaded through the delta payload', () => {
    document.body.innerHTML = '';
    const el2 = mountEl();
    const mv = vi.fn();
    const downContext = vi.fn(() => ({ startNorm: 0.5 }));
    wireDrag(el2, { axis: 'y', downContext }, { onMove: mv });
    el2.dispatchEvent(pointerEvt('pointerdown', { clientX: 0, clientY: 50 }));
    el2.dispatchEvent(pointerEvt('pointermove', { clientX: 0, clientY: 30 }));
    expect(downContext).toHaveBeenCalledTimes(1);
    expect(mv.mock.calls[0][1].ctx).toEqual({ startNorm: 0.5 });
  });
});

describe('wireDrag — rAF throttle', () => {
  let raf, flushRaf;

  beforeEach(() => {
    document.body.innerHTML = '';
    // Controllable rAF: queue callbacks, fire them on demand.
    const queue = [];
    flushRaf = () => { const q = queue.splice(0); q.forEach((cb) => cb()); };
    raf = vi.spyOn(window, 'requestAnimationFrame').mockImplementation((cb) => {
      queue.push(cb);
      return queue.length;
    });
  });

  afterEach(() => { raf.mockRestore(); });

  it('coalesces a burst of moves into one onMove carrying the latest delta', () => {
    const el = mountEl();
    const onMove = vi.fn();
    wireDrag(el, { axis: 'y', raf: true }, { onMove });
    el.dispatchEvent(pointerEvt('pointerdown', { clientX: 0, clientY: 0 }));
    el.dispatchEvent(pointerEvt('pointermove', { clientX: 0, clientY: -10 }));
    el.dispatchEvent(pointerEvt('pointermove', { clientX: 0, clientY: -20 }));
    el.dispatchEvent(pointerEvt('pointermove', { clientX: 0, clientY: -30 }));
    expect(onMove).not.toHaveBeenCalled(); // throttled until the frame fires
    flushRaf();
    expect(onMove).toHaveBeenCalledTimes(1);
    expect(onMove.mock.calls[0][1].dy).toBe(-30); // latest sample wins
  });

  it('flushes the trailing edit unthrottled on pointerup', () => {
    const el = mountEl();
    const onMove = vi.fn();
    const onUp = vi.fn();
    wireDrag(el, { axis: 'y', raf: true }, { onMove, onUp });
    el.dispatchEvent(pointerEvt('pointerdown', { clientX: 0, clientY: 0 }));
    el.dispatchEvent(pointerEvt('pointermove', { clientX: 0, clientY: -42 }));
    // No frame fired yet; pointerup must still deliver the pending sample,
    // and before onUp.
    el.dispatchEvent(pointerEvt('pointerup'));
    expect(onMove).toHaveBeenCalledTimes(1);
    expect(onMove.mock.calls[0][1].dy).toBe(-42);
    expect(onUp).toHaveBeenCalledTimes(1);
    expect(onMove.mock.invocationCallOrder[0])
      .toBeLessThan(onUp.mock.invocationCallOrder[0]);
  });
});

describe('wireDrag — hit-exclude + inner target + double-click', () => {
  it('a pointerdown matching excludeHit is not a drag (click-to-select passes through)', () => {
    document.body.innerHTML = '';
    const el = mountEl();
    el.innerHTML = '<span class="glyph">g</span><span class="face">f</span>';
    const onDown = vi.fn();
    const drag = wireDrag(el, { excludeHit: '.glyph' }, { onDown });
    const glyph = el.querySelector('.glyph');
    const evGlyph = pointerEvt('pointerdown');
    Object.defineProperty(evGlyph, 'target', { value: glyph });
    el.dispatchEvent(evGlyph);
    expect(onDown).not.toHaveBeenCalled();
    expect(drag.isDragging()).toBe(false);
    // A press off the glyph still drags.
    const face = el.querySelector('.face');
    const evFace = pointerEvt('pointerdown');
    Object.defineProperty(evFace, 'target', { value: face });
    el.dispatchEvent(evFace);
    expect(onDown).toHaveBeenCalledTimes(1);
    expect(drag.isDragging()).toBe(true);
  });

  it('attaches to an inner target element when given', () => {
    document.body.innerHTML = '';
    const host = mountEl();
    const inner = document.createElement('div');
    inner.setPointerCapture = vi.fn();
    inner.releasePointerCapture = vi.fn();
    host.appendChild(inner);
    const onDown = vi.fn();
    wireDrag(host, { target: inner, pointerToValue: () => 1 }, { onDown });
    // Event on the host (outer) is NOT wired.
    host.dispatchEvent(pointerEvt('pointerdown'));
    expect(onDown).not.toHaveBeenCalled();
    // Event on the inner target fires.
    inner.dispatchEvent(pointerEvt('pointerdown', { pointerId: 3 }));
    expect(onDown).toHaveBeenCalledTimes(1);
    expect(inner.setPointerCapture).toHaveBeenCalledWith(3);
  });

  it('fires onDoubleClick when supplied', () => {
    document.body.innerHTML = '';
    const el = mountEl();
    const onDoubleClick = vi.fn();
    wireDrag(el, {}, { onDoubleClick });
    el.dispatchEvent(new MouseEvent('dblclick', { bubbles: true, cancelable: true }));
    expect(onDoubleClick).toHaveBeenCalledTimes(1);
  });

  it('ignores a non-primary button press', () => {
    document.body.innerHTML = '';
    const el = mountEl();
    const onDown = vi.fn();
    wireDrag(el, { pointerToValue: () => 0 }, { onDown });
    const ev = pointerEvt('pointerdown');
    Object.defineProperty(ev, 'button', { value: 2 }); // right button
    el.dispatchEvent(ev);
    expect(onDown).not.toHaveBeenCalled();
  });
});
