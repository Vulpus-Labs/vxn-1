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
    <button id="pbar-save-overwrite" type="button"></button>
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

// Overwrite-Save (`pbar-save-overwrite`) needs the dirty wrap intact: panels.js
// wraps the bridge's `send.setParam` at import to flip dirty, and the recorder
// `installVxn` swaps in would lose that wrap. So this loader keeps the bridge's
// `window.vxn.send` (its `_post` is a no-op under jsdom — no `window.ipc`) and
// just spies `savePreset` to capture the write.
async function loadPanelsKeepBridge() {
  vi.resetModules();
  const mod = await import('../panels.js');
  // `_post` posts via `window.ipc.postMessage`; stub it so the wrapped senders
  // run silently under jsdom (otherwise bridge's catch logs a console error).
  globalThis.window.ipc = { postMessage: vi.fn() };
  vi.spyOn(window.vxn.send, 'savePreset');
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
    folderForUserPath: vi.fn(() => 'Bass'),
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

describe('presetBar overwrite-Save (0094 / 0016)', () => {
  it('stays disabled until the patch is dirty AND the source is a user preset', async () => {
    const { presetBar } = await loadPanelsKeepBridge();
    const btn = document.getElementById('pbar-save-overwrite');

    // Fresh load: not dirty → disabled regardless of source.
    presetBar.setSource({ kind: 'user', path: 'Bass/My Patch.preset' });
    expect(btn.disabled).toBe(true);

    // A user edit flips dirty (the wrapped bridge sender) → enabled.
    window.vxn.send.setParam(0, 0.5);
    expect(btn.disabled).toBe(false);

    // Factory source never enables overwrite (no write target).
    presetBar.setSource({ kind: 'factory', index: 3 });
    window.vxn.send.setParam(0, 0.6);
    expect(btn.disabled).toBe(true);
  });

  it('writes savePreset(name, folder) for a dirty user preset, then re-disables', async () => {
    const { presetBar } = await loadPanelsKeepBridge();
    const btn = document.getElementById('pbar-save-overwrite');
    presetBar.setName('My Patch');
    presetBar.setSource({ kind: 'user', path: 'Bass/My Patch.preset' });
    window.vxn.send.setParam(0, 0.5); // dirty

    btn.click();
    expect(browserPanel.folderForUserPath).toHaveBeenCalledWith('Bass/My Patch.preset');
    expect(window.vxn.send.savePreset).toHaveBeenCalledWith('My Patch', 'Bass');
    // Optimistic clear: button disables again until the next edit.
    expect(btn.disabled).toBe(true);
  });

  it('refuses to save when the folder is missing from the corpus (no silent fork)', async () => {
    const { presetBar } = await loadPanelsKeepBridge();
    browserPanel.folderForUserPath.mockReturnValue(undefined);
    const btn = document.getElementById('pbar-save-overwrite');
    presetBar.setName('My Patch');
    presetBar.setSource({ kind: 'user', path: 'Bass/My Patch.preset' });
    window.vxn.send.setParam(0, 0.5); // dirty → enabled

    btn.click();
    expect(window.vxn.send.savePreset).not.toHaveBeenCalled();
  });
});
