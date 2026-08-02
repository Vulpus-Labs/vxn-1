// 0219: the mod-matrix overlay (ported from vxn-2) — builds 16 slot rows with
// custom div-combos (not native <select>), posts set_matrix topology edits,
// rebinds to the active edit layer, and opens/closes as a dismissible modal.
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
const CURVES = [
  { value: 0, name: 'lin', label: 'Lin' },
  { value: 3, name: 'bipolar', label: 'Bipolar' },
];

const emptySlots = () =>
  Array.from({ length: 16 }, () => ({ source: 0, dest: 0, curve: 0, scale: 0 }));

let sends;

beforeEach(() => {
  const l0 = emptySlots();
  l0[0] = { source: 1, dest: 4, curve: 0, scale: 0 }; // Env1 → Cutoff, live
  window.vxn = {
    matrix: { sources: SOURCES, dests: DESTS, curves: CURVES, slots: [l0, emptySlots()] },
    send: {
      setMatrix: vi.fn((...a) => sends.push(['matrix', ...a])),
      setParam: vi.fn((...a) => sends.push(['param', ...a])),
    },
  };
  sends = [];
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
});

const rows = () => document.querySelectorAll('#matrix-rows .vxn-mm-row');
const combo = (i, field) => rows()[i].querySelector(`.vxn-mm-combo[data-field="${field}"]`);
const setCombo = (el, v) => {
  el.value = String(v);
  el.dispatchEvent(new Event('change'));
};

describe('matrixOverlay.build', () => {
  it('renders a header + 16 rows with custom combos (no native selects)', () => {
    matrixOverlay.build();
    expect(rows()).toHaveLength(16);
    expect(document.querySelectorAll('#matrix-rows select')).toHaveLength(0);
    expect(document.querySelector('.vxn-mm-header')).toBeTruthy();
    // Source combo carries every vocab option.
    expect(combo(0, 'source').parentElement).toBeTruthy();
    // Depth is the automatable per-layer dial cell.
    const depth = rows()[3].querySelector('.vxn-mm-depth');
    expect(depth.dataset.control).toBe('dial');
    expect(depth.dataset.param).toBe('matrix_slot3_depth');
    expect(depth.hasAttribute('data-layered')).toBe(true);
  });

  it('seeds combos from the active layer and marks active rows', () => {
    matrixOverlay.build();
    expect(combo(0, 'source').value).toBe('1');
    expect(combo(0, 'dest').value).toBe('4');
    expect(rows()[0].dataset.active).toBe('1');
    expect(rows()[1].dataset.active).toBe('0'); // empty → inactive
  });

  it('a combo edit posts set_matrix and updates the snapshot', () => {
    matrixOverlay.build();
    setCombo(combo(2, 'source'), 4); // LFO 2
    expect(sends).toContainEqual(['matrix', 'upper', 2, 'source', 4]);
    expect(window.vxn.matrix.slots[0][2].source).toBe(4);
    expect(rows()[2].dataset.active).toBe('0'); // no dest yet
    setCombo(combo(2, 'dest'), 4);
    expect(rows()[2].dataset.active).toBe('1');
  });

  it('the bin clears a slot (four topology zeros)', () => {
    matrixOverlay.build();
    rows()[0].querySelector('.vxn-mm-bin').click();
    for (const f of ['source', 'dest', 'curve', 'scale']) {
      expect(sends).toContainEqual(['matrix', 'upper', 0, f, 0]);
    }
    expect(rows()[0].dataset.active).toBe('0');
  });
});

describe('matrixOverlay.refreshForLayer', () => {
  it('reseeds from Layer 2 and updates the label', () => {
    matrixOverlay.build();
    matrixOverlay.refreshForLayer('lower');
    expect(document.getElementById('matrix-layer-label').textContent).toBe('Layer 2');
    expect(combo(0, 'source').value).toBe('0'); // layer 2 slot 0 empty
    setCombo(combo(0, 'source'), 4);
    expect(sends).toContainEqual(['matrix', 'lower', 0, 'source', 4]);
    expect(window.vxn.matrix.slots[1][0].source).toBe(4);
    expect(window.vxn.matrix.slots[0][0].source).toBe(1); // layer 1 keeps its route
  });
});

describe('mod-matrix modal', () => {
  it('opens and closes via the toggle, close button, and backdrop', () => {
    matrixOverlay.build();
    const backdrop = document.getElementById('matrix-backdrop');
    const toggle = document.getElementById('matrix-toggle');
    expect(backdrop.hidden).toBe(true);
    toggle.click();
    expect(backdrop.hidden).toBe(false);
    expect(toggle.classList.contains('on')).toBe(true);
    document.getElementById('matrix-close').click();
    expect(backdrop.hidden).toBe(true);
    // Reopen, then dismiss by clicking the backdrop itself.
    toggle.click();
    backdrop.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
    expect(backdrop.hidden).toBe(true);
  });
});
