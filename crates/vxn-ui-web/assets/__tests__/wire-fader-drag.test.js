import { describe, it, expect, beforeEach, vi } from 'vitest';
import { wireFaderDrag } from '../panels.js';

// jsdom doesn't give a useful bounding rect or pointer capture. Each test
// mounts a fresh fader, stubs `getBoundingClientRect` to a fixed window so
// the norm math is non-degenerate, and stubs `setPointerCapture` /
// `releasePointerCapture` so the helper's calls don't throw and we can
// assert they ran.

const RECT_TOP    = 100;
const RECT_HEIGHT = 200;

function makeFader() {
  const fader = document.createElement('div');
  document.body.appendChild(fader);
  vi.spyOn(fader, 'getBoundingClientRect').mockReturnValue({
    top: RECT_TOP,
    height: RECT_HEIGHT,
    left: 0,
    right: 0,
    bottom: RECT_TOP + RECT_HEIGHT,
    width: 0,
    x: 0,
    y: RECT_TOP,
    toJSON() {},
  });
  fader.setPointerCapture = vi.fn();
  fader.releasePointerCapture = vi.fn();
  return fader;
}

function pointerEvt(type, { clientY = 0, pointerId = 7 } = {}) {
  // jsdom doesn't ship `PointerEvent`; build a MouseEvent and graft the
  // pointer fields. The helper only reads `clientY` and `pointerId`.
  const ev = new MouseEvent(type, { bubbles: true, cancelable: true });
  Object.defineProperty(ev, 'pointerId', { value: pointerId });
  Object.defineProperty(ev, 'clientY', { value: clientY });
  return ev;
}

describe('wireFaderDrag', () => {
  let fader, onEnter, onLeave, onDown, onMove, onUp, drag;

  beforeEach(() => {
    document.body.innerHTML = '';
    fader = makeFader();
    onEnter = vi.fn();
    onLeave = vi.fn();
    onDown  = vi.fn();
    onMove  = vi.fn();
    onUp    = vi.fn();
    drag = wireFaderDrag(fader, { onEnter, onDown, onMove, onUp, onLeave });
  });

  it('fires onEnter on pointerenter and flips isHovered', () => {
    fader.dispatchEvent(pointerEvt('pointerenter'));
    expect(onEnter).toHaveBeenCalledTimes(1);
    expect(drag.isHovered()).toBe(true);
    expect(drag.isDragging()).toBe(false);
  });

  it('suppresses onEnter while dragging', () => {
    fader.dispatchEvent(pointerEvt('pointerdown', { clientY: 100 }));
    onEnter.mockClear();
    fader.dispatchEvent(pointerEvt('pointerenter'));
    expect(onEnter).not.toHaveBeenCalled();
  });

  it('fires onLeave on pointerleave when not dragging', () => {
    fader.dispatchEvent(pointerEvt('pointerenter'));
    fader.dispatchEvent(pointerEvt('pointerleave'));
    expect(onLeave).toHaveBeenCalledTimes(1);
    expect(drag.isHovered()).toBe(false);
  });

  it('suppresses onLeave on pointerleave while dragging', () => {
    fader.dispatchEvent(pointerEvt('pointerenter'));
    fader.dispatchEvent(pointerEvt('pointerdown', { clientY: 100 }));
    onLeave.mockClear();
    fader.dispatchEvent(pointerEvt('pointerleave'));
    expect(onLeave).not.toHaveBeenCalled();
  });

  it('onDown captures the pointer and reports norm = 1 − (clientY − top) / height', () => {
    // clientY at the top of the rect → norm 1 (top of fader); at bottom → 0.
    fader.dispatchEvent(pointerEvt('pointerdown', { clientY: RECT_TOP }));
    expect(onDown).toHaveBeenCalledTimes(1);
    expect(onDown.mock.calls[0][1]).toBe(1);
    expect(fader.setPointerCapture).toHaveBeenCalledWith(7);
    expect(drag.isDragging()).toBe(true);

    // Re-init for the bottom-edge case.
    drag.isDragging(); // sanity: still dragging from the previous test path
  });

  it('onDown clamps the norm into [0, 1]', () => {
    // clientY way above the rect → upper clamp.
    fader.dispatchEvent(pointerEvt('pointerdown', { clientY: RECT_TOP - 9999 }));
    expect(onDown.mock.calls[0][1]).toBe(1);
    // pointerup so the next pointerdown re-arms.
    fader.dispatchEvent(pointerEvt('pointerup'));
    onDown.mockClear();
    // clientY way below the rect → lower clamp.
    fader.dispatchEvent(pointerEvt('pointerdown', {
      clientY: RECT_TOP + RECT_HEIGHT + 9999,
    }));
    expect(onDown.mock.calls[0][1]).toBe(0);
  });

  it('onMove only fires while dragging', () => {
    // Not dragging yet: pointermove is a no-op.
    fader.dispatchEvent(pointerEvt('pointermove', { clientY: RECT_TOP + 50 }));
    expect(onMove).not.toHaveBeenCalled();
    // After pointerdown, moves stream through.
    fader.dispatchEvent(pointerEvt('pointerdown', { clientY: RECT_TOP }));
    fader.dispatchEvent(pointerEvt('pointermove', { clientY: RECT_TOP + 100 }));
    expect(onMove).toHaveBeenCalledTimes(1);
    // norm = 1 − 100/200 = 0.5.
    expect(onMove.mock.calls[0][1]).toBe(0.5);
  });

  it('pointerup ends the drag, releases capture, and fires onUp', () => {
    fader.dispatchEvent(pointerEvt('pointerdown', { clientY: RECT_TOP }));
    fader.dispatchEvent(pointerEvt('pointerup'));
    expect(onUp).toHaveBeenCalledTimes(1);
    expect(fader.releasePointerCapture).toHaveBeenCalledWith(7);
    expect(drag.isDragging()).toBe(false);
  });

  it('pointercancel ends the drag the same way as pointerup', () => {
    fader.dispatchEvent(pointerEvt('pointerdown', { clientY: RECT_TOP }));
    fader.dispatchEvent(pointerEvt('pointercancel'));
    expect(onUp).toHaveBeenCalledTimes(1);
    expect(drag.isDragging()).toBe(false);
  });

  it('drag-end fires onLeave when the pointer is no longer hovering', () => {
    // Hover, press, then leave the element mid-drag — drag-end must
    // notice the deferred leave.
    fader.dispatchEvent(pointerEvt('pointerenter'));
    fader.dispatchEvent(pointerEvt('pointerdown', { clientY: RECT_TOP }));
    fader.dispatchEvent(pointerEvt('pointerleave'));
    onLeave.mockClear();
    fader.dispatchEvent(pointerEvt('pointerup'));
    expect(onLeave).toHaveBeenCalledTimes(1);
  });

  it('drag-end does not double-fire onLeave when still hovering', () => {
    fader.dispatchEvent(pointerEvt('pointerenter'));
    fader.dispatchEvent(pointerEvt('pointerdown', { clientY: RECT_TOP }));
    onLeave.mockClear();
    fader.dispatchEvent(pointerEvt('pointerup'));
    expect(onLeave).not.toHaveBeenCalled();
  });
});
