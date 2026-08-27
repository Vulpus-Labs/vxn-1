import { describe, it, expect, beforeEach } from 'vitest';
import { attachValuePop } from '../panels.js';
import { valuePop } from '../panels.js';

// `attachValuePop` is the popup lifecycle adapter every primitive uses.
// The popup itself (`valuePop`) is a module-level singleton from
// bridge.js — we observe state via the popup's div in document.body.

function popupEl() {
  return document.querySelector('.value-pop');
}

function popupVisible() {
  const el = popupEl();
  return !!el && el.style.display === 'block';
}

function makeHost({ hovered = false, dragging = false } = {}) {
  let h = hovered;
  let d = dragging;
  return {
    isHovered:  () => h,
    isDragging: () => d,
    setHovered:  (v) => { h = v; },
    setDragging: (v) => { d = v; },
  };
}

describe('attachValuePop', () => {
  beforeEach(() => {
    // Reset popup state. Hide first, then re-test from a known floor.
    valuePop.hide();
  });

  it('markEntered shows the popup with getLabel()\'s current value', () => {
    const host = makeHost();
    const pop = attachValuePop(host, () => '42 Hz');
    pop.markEntered({ clientX: 100, clientY: 50 });
    expect(popupVisible()).toBe(true);
    expect(popupEl().textContent).toBe('42 Hz');
  });

  it('markEntered is suppressed while the host is dragging', () => {
    const host = makeHost({ dragging: true });
    const pop = attachValuePop(host, () => '99 %');
    pop.markEntered({ clientX: 0, clientY: 0 });
    expect(popupVisible()).toBe(false);
  });

  it('markLeft hides the popup when not dragging', () => {
    const host = makeHost();
    const pop = attachValuePop(host, () => 'x');
    pop.markEntered({ clientX: 0, clientY: 0 });
    expect(popupVisible()).toBe(true);
    pop.markLeft();
    expect(popupVisible()).toBe(false);
  });

  it('markLeft is suppressed while dragging (popup stays put)', () => {
    const host = makeHost({ dragging: true });
    const pop = attachValuePop(host, () => 'x');
    // Show first via markGrabbed (always shows), then markLeft must keep it.
    pop.markGrabbed({ clientX: 0, clientY: 0 });
    expect(popupVisible()).toBe(true);
    pop.markLeft();
    expect(popupVisible()).toBe(true);
  });

  it('markGrabbed always shows and re-anchors at the grab point', () => {
    const host = makeHost({ dragging: true });
    const pop = attachValuePop(host, () => 'grab');
    pop.markGrabbed({ clientX: 200, clientY: 80 });
    expect(popupVisible()).toBe(true);
    // The popup positions itself at (clientX + 12, clientY - 8).
    expect(popupEl().style.left).toBe('212px');
    expect(popupEl().style.top).toBe('72px');
  });

  it('markReleased hides only when not hovered', () => {
    const host = makeHost({ hovered: true, dragging: true });
    const pop = attachValuePop(host, () => 'v');
    pop.markGrabbed({ clientX: 0, clientY: 0 });
    pop.markReleased();
    expect(popupVisible()).toBe(true);
    // Now drop the hover and release again.
    host.setHovered(false);
    pop.markReleased();
    expect(popupVisible()).toBe(false);
  });

  it('refresh updates the text only when hovered or dragging', () => {
    let label = 'first';
    const host = makeHost({ hovered: true });
    const pop = attachValuePop(host, () => label);
    pop.markEntered({ clientX: 0, clientY: 0 });
    expect(popupEl().textContent).toBe('first');
    label = 'second';
    pop.refresh();
    expect(popupEl().textContent).toBe('second');

    // When neither hovered nor dragging, refresh is a no-op.
    host.setHovered(false);
    label = 'third';
    pop.refresh();
    expect(popupEl().textContent).toBe('second');
  });
});
