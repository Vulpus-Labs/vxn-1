// 0219: the mod-matrix overlay — builds 16 slot rows, posts set_matrix topology
// edits, and rebinds its selectors to the active edit layer. Depth dials are
// data-control cells bound by dispatch (covered elsewhere); this suite drives
// the topology selectors + the per-layer snapshot.
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

function emptySlots() {
  return Array.from({ length: 16 }, () => ({ source: 0, dest: 0, curve: 0, scale: 0 }));
}

let sends;

beforeEach(() => {
  const l0 = emptySlots();
  // Layer 1 slot 0 is a live route (Env1 → Cutoff); layer 2 stays empty.
  l0[0] = { source: 1, dest: 4, curve: 0, scale: 0 };
  window.vxn = {
    matrix: { sources: SOURCES, dests: DESTS, curves: CURVES, slots: [l0, emptySlots()] },
    send: { setMatrix: vi.fn((...a) => sends.push(a)) },
  };
  sends = [];
  matrixOverlay._layer = 'upper';
  document.body.innerHTML = `
    <button id="matrix-toggle"></button>
    <div id="matrix-overlay" hidden>
      <span id="matrix-layer-label"></span>
      <div id="matrix-rows"></div>
    </div>
  `;
});

const rows = () => document.querySelectorAll('#matrix-rows .mtx-row');
const rowSel = (i, field) =>
  rows()[i].querySelector(`.mtx-sel[data-field="${field}"]`);

describe('matrixOverlay.build', () => {
  it('renders 16 rows with populated selectors', () => {
    matrixOverlay.build();
    expect(rows()).toHaveLength(16);
    // Source select carries the vocab options.
    expect(rowSel(0, 'source').querySelectorAll('option')).toHaveLength(SOURCES.length);
    expect(rowSel(0, 'dest').querySelectorAll('option')).toHaveLength(DESTS.length);
    // A depth dial cell is present for dispatch to bind, layered + named.
    const depth = rows()[3].querySelector('.mtx-depth');
    expect(depth.dataset.control).toBe('dial');
    expect(depth.dataset.param).toBe('matrix_slot3_depth');
    expect(depth.hasAttribute('data-layered')).toBe(true);
  });

  it('seeds selectors from the active layer and dims inactive rows', () => {
    matrixOverlay.build();
    // Layer 1 slot 0 is Env1 → Cutoff → active (not dimmed).
    expect(rowSel(0, 'source').value).toBe('1');
    expect(rowSel(0, 'dest').value).toBe('4');
    expect(rows()[0].classList.contains('mtx-active')).toBe(true);
    // Slot 1 is empty → dimmed.
    expect(rows()[1].classList.contains('mtx-active')).toBe(false);
  });

  it('a selector edit posts set_matrix and updates the snapshot', () => {
    matrixOverlay.build();
    const src = rowSel(2, 'source');
    src.value = '4'; // LFO 2
    src.dispatchEvent(new Event('change'));
    expect(sends).toContainEqual(['upper', 2, 'source', 4]);
    // Local snapshot updated so a re-render reflects it.
    expect(window.vxn.matrix.slots[0][2].source).toBe(4);
    // Still inactive (no dest yet).
    expect(rows()[2].classList.contains('mtx-active')).toBe(false);
    // Add a dest → row becomes active.
    const dst = rowSel(2, 'dest');
    dst.value = '4';
    dst.dispatchEvent(new Event('change'));
    expect(sends).toContainEqual(['upper', 2, 'dest', 4]);
    expect(rows()[2].classList.contains('mtx-active')).toBe(true);
  });
});

describe('matrixOverlay.refreshForLayer', () => {
  it('reseeds from Layer 2 and updates the label', () => {
    matrixOverlay.build();
    matrixOverlay.refreshForLayer('lower');
    expect(document.getElementById('matrix-layer-label').textContent).toBe('Layer 2');
    // Layer 2 slot 0 is empty (Layer 1's live route does not bleed across).
    expect(rowSel(0, 'source').value).toBe('0');
    expect(rows()[0].classList.contains('mtx-active')).toBe(false);
    // An edit now targets the lower layer — a value distinct from Layer 1's.
    const src = rowSel(0, 'source');
    src.value = '4'; // LFO 2
    src.dispatchEvent(new Event('change'));
    expect(sends).toContainEqual(['lower', 0, 'source', 4]);
    expect(window.vxn.matrix.slots[1][0].source).toBe(4);
    expect(window.vxn.matrix.slots[0][0].source).toBe(1); // Layer 1 keeps its own route
  });
});

describe('matrix toggle', () => {
  it('reveals and hides the overlay', () => {
    matrixOverlay.build();
    const overlay = document.getElementById('matrix-overlay');
    const toggle = document.getElementById('matrix-toggle');
    expect(overlay.hidden).toBe(true);
    toggle.click();
    expect(overlay.hidden).toBe(false);
    expect(toggle.classList.contains('on')).toBe(true);
    toggle.click();
    expect(overlay.hidden).toBe(true);
  });
});
