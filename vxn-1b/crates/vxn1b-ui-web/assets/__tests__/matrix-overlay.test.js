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
const POLARITIES = [
  { value: 0, name: 'none', label: 'None' },
  { value: 1, name: 'bipolar', label: 'Bipolar' },
  { value: 2, name: 'abs', label: 'Abs' },
];
const SHAPES = [
  { value: 0, name: 'lin', label: 'Lin' },
  { value: 1, name: 'exp', label: 'Exp' },
  { value: 2, name: 'log', label: 'Log' },
];
// The flat `(polarity, shape)` vocabulary the curve picker labels cells with,
// and the geometry it draws — both from `vxn_core_matrix` in the real page.
// Stub points here: the panel never reads them, it only passes them through.
const CURVES = [
  { value: 0, name: 'lin', label: 'Lin' },
  { value: 1, name: 'exp', label: 'Exp' },
  { value: 2, name: 'log', label: 'Log' },
  { value: 3, name: 'bipolar', label: 'Bipolar' },
  { value: 4, name: 'bipolar-exp', label: 'Bipolar Exp' },
  { value: 5, name: 'bipolar-log', label: 'Bipolar Log' },
  { value: 6, name: 'abs', label: 'Abs' },
  { value: 7, name: 'abs-exp', label: 'Abs Exp' },
  { value: 8, name: 'abs-log', label: 'Abs Log' },
];
const GLYPHS = CURVES.map((c) => ({
  code: c.value,
  points: '0,100 100,0',
  band_x: 0,
  band_w: 100,
}));
// None / Abs / Bipolar down, Lin / Exp / Log across — display order only.
const PICKER_CODES = [0, 1, 2, 6, 7, 8, 3, 4, 5];

const blank = () => ({
  source: 0,
  dest: 0,
  polarity: 0,
  shape: 0,
  scale: 0,
  scalePolarity: 0,
  scaleShape: 0,
  enabled: false,
});
const emptySlots = () => Array.from({ length: 16 }, blank);

let sends;

beforeEach(() => {
  const l0 = emptySlots();
  // Env1 → Cutoff, wired and switched on.
  l0[0] = { ...blank(), source: 1, dest: 4, enabled: true };
  window.vxn = {
    matrix: {
      sources: SOURCES,
      dests: DESTS,
      polarities: POLARITIES,
      shapes: SHAPES,
      curves: CURVES,
      curveGlyphs: GLYPHS,
      pickerCodes: PICKER_CODES,
      slots: [l0, emptySlots()],
    },
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
const onBox = (i) => rows()[i].querySelector('.vxn-mm-on');
const setCheck = (el, v) => {
  el.checked = v;
  el.dispatchEvent(new Event('change'));
};
const setCombo = (el, v) => {
  el.value = String(v);
  el.dispatchEvent(new Event('change'));
};
// The curve buttons are not combos: click one to open the shared picker, then
// click the cell for `code`.
const curveBtn = (i, which) => rows()[i].querySelector(`.cg-btn[data-curve="${which}"]`);
const pickCurve = (i, which, code) => {
  curveBtn(i, which).dispatchEvent(new MouseEvent('click', { bubbles: true }));
  document
    .querySelector(`.cg-picker .cg-opt[data-code="${code}"]`)
    .dispatchEvent(new MouseEvent('click', { bubbles: true }));
};

describe('matrixOverlay.build', () => {
  it('renders a header + 16 rows with custom combos (no native selects)', () => {
    matrixOverlay.build();
    expect(rows()).toHaveLength(16);
    expect(document.querySelectorAll('#matrix-rows select')).toHaveLength(0);
    expect(document.querySelector('.vxn-mm-header')).toBeTruthy();
    // Source combo carries every vocab option.
    expect(combo(0, 'source').parentElement).toBeTruthy();
    // Depth is the automatable per-layer bipolar fader cell.
    const depth = rows()[3].querySelector('.vxn-mm-depth');
    expect(depth.dataset.control).toBe('bipolar');
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

  it('the bin clears a slot (every topology field zeroed, switch off)', () => {
    matrixOverlay.build();
    rows()[0].querySelector('.vxn-mm-bin').click();
    // Wire names, not snapshot keys — the scale bend differs between the two.
    for (const f of ['source', 'dest', 'polarity', 'shape', 'scale', 'scale-shape', 'enabled']) {
      expect(sends).toContainEqual(['matrix', 'upper', 0, f, 0]);
    }
    expect(rows()[0].dataset.active).toBe('0');
    expect(onBox(0).checked).toBe(false);
  });

  it('the on/off switch posts enabled without touching the wiring', () => {
    matrixOverlay.build();
    expect(onBox(0).checked).toBe(true); // slot 0 is the live Env1 → Cutoff
    setCheck(onBox(0), false);
    expect(sends).toContainEqual(['matrix', 'upper', 0, 'enabled', 0]);
    // Greyed out, but the endpoints are untouched — switching off is not a
    // delete, which is the whole reason the flag is separate from the wiring.
    expect(rows()[0].dataset.active).toBe('0');
    expect(window.vxn.matrix.slots[0][0].source).toBe(1);
    expect(window.vxn.matrix.slots[0][0].dest).toBe(4);
    expect(combo(0, 'source').value).toBe('1');

    setCheck(onBox(0), true);
    expect(sends).toContainEqual(['matrix', 'upper', 0, 'enabled', 1]);
    expect(rows()[0].dataset.active).toBe('1');
  });

  it('picking a source on a blank row switches it on', () => {
    matrixOverlay.build();
    expect(onBox(1).checked).toBe(false);
    setCombo(combo(1, 'source'), 4);
    expect(sends).toContainEqual(['matrix', 'upper', 1, 'source', 4]);
    expect(sends).toContainEqual(['matrix', 'upper', 1, 'enabled', 1]);
    expect(onBox(1).checked).toBe(true);
  });

  it('retuning a deliberately disabled row leaves it disabled', () => {
    matrixOverlay.build();
    setCheck(onBox(0), false);
    sends.length = 0;
    setCombo(combo(0, 'source'), 4);
    // The auto-enable fires only on the None→real edge; slot 0 already had a
    // source, so this is a retune of a route the player switched off.
    expect(sends).not.toContainEqual(['matrix', 'upper', 0, 'enabled', 1]);
    expect(onBox(0).checked).toBe(false);
  });

  it('one curve pick posts both axes independently', () => {
    matrixOverlay.build();
    // `Abs Exp` — flat code 7, i.e. polarity 2 × shape 1. VXN1b stores the two
    // axes as separate bytes, so the picker's single choice must arrive as two
    // edits, not as the flat code the glyph is indexed by.
    pickCurve(0, 'curve', 7);
    expect(sends).toContainEqual(['matrix', 'upper', 0, 'polarity', 2]);
    expect(sends).toContainEqual(['matrix', 'upper', 0, 'shape', 1]);
    expect(window.vxn.matrix.slots[0][0].polarity).toBe(2);
    expect(window.vxn.matrix.slots[0][0].shape).toBe(1);
  });

  it('the scale curve drives the scale axes, and only those', () => {
    matrixOverlay.build();
    sends.length = 0;
    // `Bipolar Log` — flat code 5, polarity 1 × shape 2.
    pickCurve(0, 'scaleCurve', 5);
    // Snapshot keys are camelCase, wire names kebab (`vocab::MATRIX_FIELD_NAMES`).
    expect(sends).toContainEqual(['matrix', 'upper', 0, 'scale-polarity', 1]);
    expect(sends).toContainEqual(['matrix', 'upper', 0, 'scale-shape', 2]);
    expect(window.vxn.matrix.slots[0][0].scalePolarity).toBe(1);
    expect(window.vxn.matrix.slots[0][0].scaleShape).toBe(2);
    // The route's own curve is a different control and is untouched.
    expect(sends).not.toContainEqual(['matrix', 'upper', 0, 'polarity', 1]);
    expect(window.vxn.matrix.slots[0][0].polarity).toBe(0);
  });

  it('a row shows one glyph button per curve and no axis pick-lists', () => {
    matrixOverlay.build();
    expect(rows()[0].querySelectorAll('.cg-btn')).toHaveLength(2);
    for (const field of ['polarity', 'shape', 'scaleShape', 'scalePolarity']) {
      expect(combo(0, field)).toBeNull();
    }
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
