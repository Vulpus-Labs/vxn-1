// 0265: the "Copy Layer 1 → Layer 2" cell.
//
// The op itself is engine-side (`SharedParams::copy_layer`, covered in
// shared.rs); what needs testing here is the *arming*. The copy overwrites
// whatever Layer 2 held and lands ~66 param changes in the host's undo stack as
// one burst, so a single stray tap must never fire it.
//
// dispatch.js imports nothing — at splice time it shares one scope with
// panels.js — so `tgRow` is a free identifier stubbed on globalThis, exactly as
// the splice would define it.
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { pointerEvt } from './_helpers.js';
import { wireCopyLayer, COPY_ARM_MS } from '../dispatch.js';

function mountCell() {
  document.body.innerHTML = '<div id="copy-layer"></div>';
  return document.getElementById('copy-layer');
}

describe('copy layer cell (0265)', () => {
  let sent;

  beforeEach(() => {
    vi.useFakeTimers();
    globalThis.tgRow = (label) => {
      const el = document.createElement('div');
      el.className = 'ctl-tg-row';
      el.textContent = label;
      return el;
    };
    sent = [];
    globalThis.window.vxn = { send: { copyLayer: (from, to) => sent.push([from, to]) } };
  });

  afterEach(() => {
    vi.useRealTimers();
    document.body.innerHTML = '';
  });

  it('does not copy on a single press — it arms', () => {
    const el = mountCell();
    wireCopyLayer();
    const row = el.firstChild;

    row.dispatchEvent(pointerEvt('pointerdown'));
    expect(sent).toEqual([]);
    expect(row.classList.contains('active')).toBe(true);
    expect(row.textContent).toBe('Sure?');
  });

  it('copies upper → lower on the confirming press, once', () => {
    const el = mountCell();
    wireCopyLayer();
    const row = el.firstChild;

    row.dispatchEvent(pointerEvt('pointerdown'));
    row.dispatchEvent(pointerEvt('pointerdown'));
    expect(sent).toEqual([['upper', 'lower']]);
    // Disarmed again, so the next press re-arms rather than copying twice.
    expect(row.textContent).toBe('Copy → L2');
    row.dispatchEvent(pointerEvt('pointerdown'));
    expect(sent).toEqual([['upper', 'lower']]);
  });

  it('disarms on a timeout, so an armed cell cannot outlive attention', () => {
    const el = mountCell();
    wireCopyLayer();
    const row = el.firstChild;

    row.dispatchEvent(pointerEvt('pointerdown'));
    vi.advanceTimersByTime(COPY_ARM_MS + 1);
    expect(row.textContent).toBe('Copy → L2');
    expect(row.classList.contains('active')).toBe(false);

    // The press that follows arms afresh instead of confirming the stale one.
    row.dispatchEvent(pointerEvt('pointerdown'));
    expect(sent).toEqual([]);
  });

  it('disarms when the player clicks anything else', () => {
    const el = mountCell();
    const elsewhere = document.createElement('div');
    document.body.appendChild(elsewhere);
    wireCopyLayer();
    const row = el.firstChild;

    row.dispatchEvent(pointerEvt('pointerdown'));
    elsewhere.dispatchEvent(pointerEvt('pointerdown'));
    expect(row.textContent).toBe('Copy → L2');

    row.dispatchEvent(pointerEvt('pointerdown'));
    expect(sent).toEqual([]);
  });
});
