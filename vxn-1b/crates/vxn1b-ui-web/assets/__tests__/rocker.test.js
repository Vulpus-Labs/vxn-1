// The rocker (Voice mode) and the multi-column button group (Voice width).
//
// Both are Voice-panel presentation changes over unchanged params, so what
// matters is that the *wire* stayed a plain discrete write on the same enum
// index — a rocker that posted a bool, or a column layout that reordered the
// stored values, would silently repatch every preset.
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { pointerEvt } from './_helpers.js';
import { makeRocker, makeButtonGroup } from '../panels.js';

const VOICE_MODE = { kind: 'enum', label: 'Voice', variants: ['Poly', 'Solo'] };
const WIDTH = { kind: 'enum', label: 'Width', variants: ['1', '2', '4', '8', '16', '32'] };

let sent;

beforeEach(() => {
  sent = [];
  globalThis.window.vxn = {
    send: { discrete: (id, plain) => sent.push([id, plain]) },
  };
  document.body.innerHTML = '';
});

afterEach(() => {
  document.body.innerHTML = '';
});

function mount() {
  const el = document.createElement('div');
  document.body.appendChild(el);
  return el;
}

describe('makeRocker', () => {
  it('draws both variant names either side of the track', () => {
    const el = mount();
    makeRocker(el, 7, VOICE_MODE);
    expect(el.querySelector('.ctl-rocker-left').textContent).toBe('POLY');
    expect(el.querySelector('.ctl-rocker-right').textContent).toBe('SOLO');
    expect(el.querySelector('.ctl-rocker-track')).not.toBeNull();
    expect(el.querySelector('.ctl-rocker-knob')).not.toBeNull();
  });

  it('reflects the echoed variant index, lighting the active label', () => {
    const el = mount();
    const ctl = makeRocker(el, 7, VOICE_MODE);

    ctl.update(0);
    expect(el.classList.contains('right')).toBe(false);
    expect(el.querySelector('.ctl-rocker-left').classList.contains('active')).toBe(true);
    expect(el.querySelector('.ctl-rocker-right').classList.contains('active')).toBe(false);

    ctl.update(1);
    expect(el.classList.contains('right')).toBe(true);
    expect(el.querySelector('.ctl-rocker-right').classList.contains('active')).toBe(true);
  });

  it('posts the other variant index on click, from either state', () => {
    const el = mount();
    const ctl = makeRocker(el, 7, VOICE_MODE);

    ctl.update(0);
    el.dispatchEvent(pointerEvt('pointerdown'));
    expect(sent).toEqual([[7, 1]]);

    // The echo is what flips the visual, so drive it as the dispatcher would.
    ctl.update(1);
    el.dispatchEvent(pointerEvt('pointerdown'));
    expect(sent).toEqual([[7, 1], [7, 0]]);
  });

  it('clamps an out-of-range echo instead of desyncing', () => {
    const el = mount();
    const ctl = makeRocker(el, 7, VOICE_MODE);
    ctl.update(9);
    expect(el.classList.contains('right')).toBe(true);
    ctl.update(-4);
    expect(el.classList.contains('right')).toBe(false);
  });
});

describe('makeButtonGroup — columns', () => {
  it('keeps one column by default', () => {
    const el = mount();
    makeButtonGroup(el, 3, WIDTH);
    expect(el.style.getPropertyValue('--ctl-rows')).toBe('');
  });

  it('splits six variants into two columns of three', () => {
    const el = mount();
    el.dataset.columns = '2';
    makeButtonGroup(el, 3, WIDTH);
    // The CSS grid is column-major over this row count, so column one is
    // 1/2/4 and column two is 8/16/32.
    expect(el.style.getPropertyValue('--ctl-rows')).toBe('3');
    const labels = [...el.querySelectorAll('.ctl-tg-lbl')].map((n) => n.textContent);
    expect(labels).toEqual(['1', '2', '4', '8', '16', '32']);
  });

  it('rounds up so an odd variant count leaves the short column last', () => {
    const el = mount();
    el.dataset.columns = '2';
    makeButtonGroup(el, 3, { kind: 'enum', label: 'W', variants: ['a', 'b', 'c', 'd', 'e'] });
    expect(el.style.getPropertyValue('--ctl-rows')).toBe('3');
  });

  it('still posts each variant\'s own index, whatever the layout', () => {
    const el = mount();
    el.dataset.columns = '2';
    makeButtonGroup(el, 3, WIDTH);
    const rows = el.querySelectorAll('.ctl-tg-row');
    // "16" is the fifth row in DOM order and index 4 in the descriptor — the
    // column split must not have renumbered anything.
    rows[4].dispatchEvent(pointerEvt('pointerdown'));
    expect(sent).toEqual([[3, 4]]);
  });
});
