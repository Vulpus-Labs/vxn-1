import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { installVxn } from './_helpers.js';

// 0093: presetBar wiring (Prev / Next / Browse / Save). `browserPanel` is a
// free identifier in panels.js (concat-time global at runtime); under Node
// ESM we satisfy it via `globalThis.browserPanel` before the dynamic import.

let sendCalls, browserPanel;

function mountPresetBarDOM() {
  document.body.innerHTML = `
    <div id="pbar-name"></div>
    <button id="pbar-prev" type="button"></button>
    <button id="pbar-next" type="button"></button>
    <button id="pbar-browse" type="button"></button>
    <button id="pbar-save" type="button"></button>
  `;
}
async function loadPanels() {
  vi.resetModules();
  const mod = await import('../panels.js');
  // bridge.js (imported by panels.js) overwrites `window.vxn` at eval time —
  // install our recorder after the import so the buttons resolve to it at
  // click time.
  ({ sendCalls } = installVxn(['stepPreset']));
  return mod;
}

beforeEach(() => {
  mountPresetBarDOM();
  browserPanel = {
    _open: false,
    _cb: null,
    isOpen: vi.fn(function () { return this._open; }),
    setOpen: vi.fn(function (v) { this._open = !!v; }),
    openSaveAs: vi.fn(),
    onOpenChange: vi.fn(function (cb) { this._cb = cb; }),
  };
  globalThis.browserPanel = browserPanel;
});

afterEach(() => {
  delete globalThis.browserPanel;
});

describe('presetBar buttons', () => {
  it('Prev click sends stepPreset(-1)', async () => {
    await loadPanels();
    document.getElementById('pbar-prev').click();
    expect(sendCalls).toEqual([['stepPreset', -1]]);
  });

  it('Next click sends stepPreset(+1)', async () => {
    await loadPanels();
    document.getElementById('pbar-next').click();
    expect(sendCalls).toEqual([['stepPreset', 1]]);
  });

  it('Browse click toggles the browser panel via setOpen', async () => {
    await loadPanels();
    const btn = document.getElementById('pbar-browse');
    btn.click();                                       // closed → open
    expect(browserPanel.setOpen).toHaveBeenLastCalledWith(true);
    browserPanel._open = true;                         // simulate the open state
    btn.click();                                       // open → close
    expect(browserPanel.setOpen).toHaveBeenLastCalledWith(false);
  });

  it('Save click calls openSaveAs with the current preset name', async () => {
    const { presetBar } = await loadPanels();
    presetBar.setName('My Patch');
    document.getElementById('pbar-save').click();
    expect(browserPanel.openSaveAs).toHaveBeenCalledWith('My Patch');
  });

  it('onOpenChange is registered and flips the browse button\'s .active class', async () => {
    await loadPanels();
    expect(browserPanel.onOpenChange).toHaveBeenCalled();
    const btn = document.getElementById('pbar-browse');
    expect(btn.classList.contains('active')).toBe(false);
    browserPanel._cb(true);
    expect(btn.classList.contains('active')).toBe(true);
    browserPanel._cb(false);
    expect(btn.classList.contains('active')).toBe(false);
  });
});
