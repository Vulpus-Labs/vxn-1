// 0318: the mod-matrix's custom div-combo, driven through its POPUP rather
// than by assigning `.value`. matrix-overlay.test.js covers the overlay that
// hosts these and sets values directly; nothing covered the dropdown itself,
// which is where all of the combo's behaviour lives — open, pick, commit,
// dismiss-on-outside-click, dismiss-on-Escape.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { matrixOverlay } from '../panels/matrix.js';

const SOURCES = [
  { value: 0, name: 'none', label: '—' },
  { value: 1, name: 'env1', label: 'Env 1' },
  { value: 4, name: 'lfo2', label: 'LFO 2' },
];
const DESTS = [
  { value: 0, name: 'none', label: '—' },
  { value: 4, name: 'cutoff', label: 'Cutoff' },
];
const CURVES = [{ value: 0, name: 'lin', label: 'Lin' }];

const emptySlots = () =>
  Array.from({ length: 16 }, () => ({ source: 0, dest: 0, curve: 0, scale: 0 }));

// `mousedown` because that is the event the combo listens on — it deliberately
// commits before focus moves, to keep focus inside the webview.
const down = (el) => el.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true }));

describe('the mod-matrix combo popup', () => {
  let btn;

  beforeEach(() => {
    window.vxn = {
      matrix: {
        sources: SOURCES,
        dests: DESTS,
        curves: CURVES,
        slots: [emptySlots(), emptySlots()],
      },
      send: { setMatrix: vi.fn(), setParam: vi.fn() },
    };
    matrixOverlay._layer = 'upper';
    document.body.innerHTML = `
      <button id="matrix-toggle"></button>
      <div class="overlay-backdrop" id="matrix-backdrop" hidden>
        <div class="overlay-panel">
          <button id="matrix-close"></button>
          <span id="matrix-layer-label"></span>
          <div id="matrix-rows"></div>
        </div>
      </div>
    `;
    matrixOverlay.build();
    btn = document.querySelector('#matrix-rows .vxn-mm-combo[data-field="source"]');
  });

  const popup = () => document.querySelector('.vxn-mm-combo-pop');
  const options = () => Array.from(popup().querySelectorAll('.vxn-mm-combo-opt'));

  it('shows the current label, not the raw value', () => {
    expect(btn.querySelector('.vxn-mm-combo-label').textContent).toBe('—');
    btn.value = 4;
    expect(btn.querySelector('.vxn-mm-combo-label').textContent).toBe('LFO 2');
  });

  it('opens a body-attached popup carrying every vocab entry, current one marked', () => {
    expect(popup()).toBeNull();
    down(btn);
    expect(popup().parentElement).toBe(document.body);
    expect(options().map((o) => o.textContent)).toEqual(['—', 'Env 1', 'LFO 2']);
    expect(options().filter((o) => o.classList.contains('sel'))).toHaveLength(1);
    expect(options()[0].classList.contains('sel')).toBe(true);
    expect(btn.classList.contains('open')).toBe(true);
  });

  it('a second click on the button closes it again', () => {
    down(btn);
    down(btn);
    expect(popup()).toBeNull();
    expect(btn.classList.contains('open')).toBe(false);
  });

  it('picking an entry commits the value, fires one change, and closes', () => {
    const onChange = vi.fn();
    btn.addEventListener('change', onChange);
    down(btn);
    down(options()[2]);
    expect(btn.value).toBe('4');
    expect(btn.querySelector('.vxn-mm-combo-label').textContent).toBe('LFO 2');
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(popup()).toBeNull();
  });

  it('picking the entry already selected commits without firing change', () => {
    const onChange = vi.fn();
    btn.addEventListener('change', onChange);
    down(btn);
    down(options()[0]);
    expect(onChange).not.toHaveBeenCalled();
    expect(popup()).toBeNull();
  });

  it('a mousedown outside both button and popup dismisses it', () => {
    down(btn);
    down(document.body);
    expect(popup()).toBeNull();
  });

  it('Escape dismisses the popup and is not allowed to reach the overlay', () => {
    down(btn);
    const ev = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true });
    const stop = vi.spyOn(ev, 'stopPropagation');
    document.body.dispatchEvent(ev);
    expect(popup()).toBeNull();
    expect(stop).toHaveBeenCalled();
  });

  it('drops its document listeners on close, so a later outside click is inert', () => {
    const remove = vi.spyOn(document, 'removeEventListener');
    down(btn);
    down(document.body);
    const kinds = remove.mock.calls.map((c) => c[0]);
    expect(kinds).toContain('mousedown');
    expect(kinds).toContain('keydown');
    // A second outside click with no popup open must not resurrect one.
    down(document.body);
    expect(popup()).toBeNull();
  });
});
