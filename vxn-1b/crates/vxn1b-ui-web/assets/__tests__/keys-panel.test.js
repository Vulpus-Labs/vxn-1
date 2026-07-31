import { describe, it, expect, beforeEach, vi } from 'vitest';
import { installVxn } from './_helpers.js';
import { noteName, KEYS_SPLIT_MIN, KEYS_SPLIT_MAX, KEYS_DEFAULT_SPLIT } from '../panels.js';

// 0093: Keys panel mode / edit-layer / split-point widgets. `setMode` /
// `setLayer` on the returned API are the test door for internal mode/layer
// state; visibility flips and Reset's branching both follow from there.

let sendCalls;

function mountKeysDOM() {
  document.body.innerHTML = `
    <div class="panel" data-name="Keys">
      <div class="panel-body"></div>
    </div>
  `;
}
async function loadKeysPanel() {
  vi.resetModules();
  const mod = await import('../panels.js');
  // bridge.js (imported by panels.js) overwrites `window.vxn` at eval time —
  // install our recorder after the import so handlers resolve to it at fire
  // time.
  ({ sendCalls } = installVxn(['setKeyMode', 'setEditLayer', 'setSplitPoint', 'resetLayer']));
  return mod.keysPanel;
}
function pointerdown() {
  return new MouseEvent('pointerdown', { bubbles: true, cancelable: true });
}

beforeEach(() => {
  mountKeysDOM();
});

describe('Keys panel — mode rows', () => {
  it('pointerdown on an inactive mode row sends setKeyMode; active row no-ops', async () => {
    await loadKeysPanel();
    const rows = document.querySelectorAll('#keys-mode-list .ctl-tg-row');
    // Initial mode is 0 (Whole) → row 0 is active.
    rows[0].dispatchEvent(pointerdown());
    expect(sendCalls).toEqual([]);
    rows[2].dispatchEvent(pointerdown());                // SPLIT
    expect(sendCalls).toEqual([['setKeyMode', 2]]);
  });
});

describe('Keys panel — edit-layer rows', () => {
  it('pointerdown on the inactive layer sends setEditLayer; active layer no-ops', async () => {
    const keysPanel = await loadKeysPanel();
    keysPanel.setMode(1);                                // Dual — edit-list visible
    const rows = document.querySelectorAll('#keys-edit-list .ctl-tg-row');
    rows[0].dispatchEvent(pointerdown());                // UPPER, the active layer
    expect(sendCalls).toEqual([]);
    rows[1].dispatchEvent(pointerdown());                // LOWER
    expect(sendCalls).toEqual([['setEditLayer', 'lower']]);
  });
});

describe('Keys panel — dim state', () => {
  it('edit-layer list is dimmed in Whole, live in Dual/Split', async () => {
    const keysPanel = await loadKeysPanel();
    const editList = document.getElementById('keys-edit-list');
    expect(editList.classList.contains('dimmed')).toBe(true);  // Whole (initial)
    keysPanel.setMode(1);
    expect(editList.classList.contains('dimmed')).toBe(false);
    keysPanel.setMode(2);
    expect(editList.classList.contains('dimmed')).toBe(false);
    keysPanel.setMode(0);
    expect(editList.classList.contains('dimmed')).toBe(true);
  });

  it('split row is live only in Split mode, dimmed elsewhere', async () => {
    const keysPanel = await loadKeysPanel();
    const row = document.getElementById('keys-split-row');
    expect(row.classList.contains('dimmed')).toBe(true);   // Whole
    keysPanel.setMode(1);
    expect(row.classList.contains('dimmed')).toBe(true);   // Dual
    keysPanel.setMode(2);
    expect(row.classList.contains('dimmed')).toBe(false);  // Split
  });
});

describe('Keys panel — split slider', () => {
  it('input clamps to [KEYS_SPLIT_MIN, KEYS_SPLIT_MAX], sends setSplitPoint, updates readout', async () => {
    await loadKeysPanel();
    const slider   = document.getElementById('keys-split-slider');
    const readout  = document.getElementById('keys-split-readout');

    slider.value = String(KEYS_SPLIT_MIN - 5);          // below floor
    slider.dispatchEvent(new Event('input'));
    expect(sendCalls).toEqual([['setSplitPoint', KEYS_SPLIT_MIN]]);
    expect(readout.textContent).toBe(noteName(KEYS_SPLIT_MIN));

    sendCalls.length = 0;
    slider.value = String(KEYS_SPLIT_MAX + 5);          // above ceiling
    slider.dispatchEvent(new Event('input'));
    expect(sendCalls).toEqual([['setSplitPoint', KEYS_SPLIT_MAX]]);
    expect(readout.textContent).toBe(noteName(KEYS_SPLIT_MAX));
  });

  it('dblclick sends setSplitPoint(KEYS_DEFAULT_SPLIT)', async () => {
    await loadKeysPanel();
    const slider = document.getElementById('keys-split-slider');
    slider.dispatchEvent(new MouseEvent('dblclick', { bubbles: true, cancelable: true }));
    expect(sendCalls).toEqual([['setSplitPoint', KEYS_DEFAULT_SPLIT]]);
  });
});

describe('Keys panel — Reset button', () => {
  it('in Whole sends resetLayer for both upper and lower', async () => {
    await loadKeysPanel();
    document.getElementById('keys-reset').dispatchEvent(pointerdown());
    expect(sendCalls).toEqual([['resetLayer', 'upper'], ['resetLayer', 'lower']]);
  });

  it('in Dual/Split sends resetLayer for the current edit layer only', async () => {
    const keysPanel = await loadKeysPanel();
    keysPanel.setMode(1);                                // Dual
    document.getElementById('keys-reset').dispatchEvent(pointerdown());
    expect(sendCalls).toEqual([['resetLayer', 'upper']]);

    sendCalls.length = 0;
    keysPanel.setLayer('lower');
    keysPanel.setMode(2);                                // Split
    document.getElementById('keys-reset').dispatchEvent(pointerdown());
    expect(sendCalls).toEqual([['resetLayer', 'lower']]);
  });
});
