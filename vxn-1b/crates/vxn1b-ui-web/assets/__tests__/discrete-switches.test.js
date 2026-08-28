// 0320: `makeSwitch` and `makeHeaderSwitch` appeared only as STUB factories in
// dispatch-orchestration.test.js, so the suite proved they were *called*, not
// that they work. (`makeWave` was the third in that list and got its own suite
// in 0318; `makeDropdown` died in 0310.)
//
// Both are echo-driven: a click posts a `discrete` opcode and paints nothing —
// the engine's ParamChanged echo does the painting via `update`. That split is
// the thing worth pinning, because a widget that paints locally on click looks
// identical until the engine refuses the value.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { makeSwitch, makeHeaderSwitch } from '../panels.js';
import { mountEl } from './_helpers.js';

const down = (el) =>
  el.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, cancelable: true }));

const BOOL = { name: 'legato', label: 'Legato', kind: 'bool', min: 0, max: 1, default: 0 };
const ENUM = {
  name: 'filter_mode',
  label: 'Filter Mode',
  kind: 'enum',
  min: 0,
  max: 3,
  default: 0,
  variants: ['LP', 'HP', 'BP', 'Notch'],
};
const ID = 22;

describe('makeSwitch — bool', () => {
  let el, send, ctl;

  beforeEach(() => {
    document.body.innerHTML = '';
    el = mountEl();
    send = { discrete: vi.fn() };
    window.vxn = { send };
    ctl = makeSwitch(el, ID, BOOL);
  });

  const rows = () => el.querySelectorAll('.ctl-tg-row');
  const active = () => Array.from(rows()).map((r) => r.classList.contains('active'));

  it('renders one row carrying the label, upper-cased', () => {
    expect(rows()).toHaveLength(1);
    expect(el.querySelector('.ctl-tg-lbl').textContent).toBe('LEGATO');
    expect(el.querySelector('.ctl-tg-box')).not.toBeNull();
  });

  it('prefers the markup label over the descriptor label', () => {
    const other = mountEl();
    other.dataset.label = 'Glide';
    makeSwitch(other, ID, BOOL);
    expect(other.querySelector('.ctl-tg-lbl').textContent).toBe('GLIDE');
  });

  it('toggles against the painted state, not an internal copy', () => {
    down(rows()[0]);
    expect(send.discrete).toHaveBeenLastCalledWith(ID, 1);
    // Still dark: the click posts, the echo paints. Clicking again therefore
    // still reads "off" and posts 1 a second time.
    expect(active()).toEqual([false]);
    down(rows()[0]);
    expect(send.discrete).toHaveBeenLastCalledWith(ID, 1);

    // Once the echo lands, the next click posts the opposite.
    ctl.update(1);
    expect(active()).toEqual([true]);
    down(rows()[0]);
    expect(send.discrete).toHaveBeenLastCalledWith(ID, 0);
  });

  it('treats anything at or above 0.5 as on', () => {
    ctl.update(0.5);
    expect(active()).toEqual([true]);
    ctl.update(0.49);
    expect(active()).toEqual([false]);
    ctl.update(1);
    expect(active()).toEqual([true]);
    ctl.update(0);
    expect(active()).toEqual([false]);
  });
});

describe('makeSwitch — enum', () => {
  let el, send, ctl;

  beforeEach(() => {
    document.body.innerHTML = '';
    el = mountEl();
    send = { discrete: vi.fn() };
    window.vxn = { send };
    ctl = makeSwitch(el, ID, ENUM);
  });

  const rows = () => el.querySelectorAll('.ctl-tg-row');
  const activeIndex = () =>
    Array.from(rows()).findIndex((r) => r.classList.contains('active'));

  it('renders one row per variant, in order', () => {
    expect(rows()).toHaveLength(4);
    expect(Array.from(el.querySelectorAll('.ctl-tg-lbl')).map((l) => l.textContent))
      .toEqual(['LP', 'HP', 'BP', 'NOTCH']);
  });

  it('posts the absolute variant index, not a toggle', () => {
    down(rows()[2]);
    expect(send.discrete).toHaveBeenLastCalledWith(ID, 2);
    // Same row twice posts the same index — an enum row is a radio, not a
    // toggle, so re-clicking the live one must not turn it off.
    down(rows()[2]);
    expect(send.discrete).toHaveBeenLastCalledWith(ID, 2);
    down(rows()[0]);
    expect(send.discrete).toHaveBeenLastCalledWith(ID, 0);
  });

  it('lights exactly one row on an echo', () => {
    for (let i = 0; i < 4; i++) {
      ctl.update(i);
      expect(activeIndex()).toBe(i);
      expect(Array.from(rows()).filter((r) => r.classList.contains('active'))).toHaveLength(1);
    }
  });

  it('clamps an out-of-range echo into the variant list', () => {
    ctl.update(99);
    expect(activeIndex()).toBe(3);
    ctl.update(-7);
    expect(activeIndex()).toBe(0);
  });
});

describe('makeHeaderSwitch', () => {
  let el, send, ctl;

  beforeEach(() => {
    document.body.innerHTML = '';
    el = mountEl();
    send = { discrete: vi.fn() };
    window.vxn = { send };
    ctl = makeHeaderSwitch(el, ID, BOOL);
  });

  const box = () => el.querySelector('.panel-header-switch');

  it('renders the lamp and starts dark', () => {
    expect(box()).not.toBeNull();
    expect(box().classList.contains('active')).toBe(false);
  });

  it('is clickable anywhere on the mount, not just the lamp', () => {
    // The listener is on `el`, so the whole header cell is the hit area.
    down(el);
    expect(send.discrete).toHaveBeenLastCalledWith(ID, 1);
  });

  it('toggles against the painted state and paints only on echo', () => {
    down(box());
    expect(send.discrete).toHaveBeenLastCalledWith(ID, 1);
    expect(box().classList.contains('active')).toBe(false);
    ctl.update(1);
    expect(box().classList.contains('active')).toBe(true);
    down(box());
    expect(send.discrete).toHaveBeenLastCalledWith(ID, 0);
  });

  it('treats anything at or above 0.5 as on', () => {
    ctl.update(0.5);
    expect(box().classList.contains('active')).toBe(true);
    ctl.update(0.49);
    expect(box().classList.contains('active')).toBe(false);
  });
});
