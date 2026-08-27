import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  browserDOM,
  installVxn,
  loadBrowserPanel,
} from './_helpers.js';

// Per the E015 / 0080 ticket: each test mounts a fresh fixture DOM,
// `vi.resetModules()` to force re-eval of `browser.js`, then dynamic
// `import` it. `browserPanel` is a module-level IIFE that snapshots DOM
// element refs at evaluation time, so the markup must exist before the
// import — and a stale prior-test panel cannot be reused.

const BROWSER_OPCODES = [
  'stepPreset', 'loadFactory', 'loadUser',
  'renamePreset', 'deletePreset', 'movePreset',
  'renameFolder', 'deleteFolder', 'newFolder',
  'savePreset',
];

let sendCalls;

function folderRowByText(text) {
  const folders = document.getElementById('browser-folders');
  return Array.from(folders.querySelectorAll('.browser-row'))
    .find((r) => r.textContent === text) || null;
}

beforeEach(() => {
  document.body.innerHTML = browserDOM();
  ({ sendCalls } = installVxn(BROWSER_OPCODES, { promptValue: 'Pad 1' }));
});

describe('browserPanel.setCorpus', () => {
  it('collapses selection to user root when the prior folder vanishes', async () => {
    const panel = await loadBrowserPanel();
    panel.setOpen(true);
    panel.setCorpus({
      factory: [],
      user: [{ name: 'Bass', presets: [{ name: 'A', path: '/u/Bass/A.preset' }] }],
    });
    const bassRow = folderRowByText('Bass');
    expect(bassRow).not.toBeNull();
    bassRow.click();
    expect(folderRowByText('Bass').classList.contains('selected')).toBe(true);

    // Push a corpus that no longer has Bass — selection collapses to
    // the user-root row (`Uncategorised`), which always exists.
    panel.setCorpus({ factory: [], user: [{ name: 'Lead', presets: [] }] });
    expect(folderRowByText('Bass')).toBeNull();
    // No row gets the `selected` class because the corpus we pushed
    // doesn't include the virtual root either; setCorpus stamps it on
    // user root, but that row only renders when a `{name: null}` group
    // is in the corpus. Re-push with the root group present:
    panel.setCorpus({
      factory: [],
      user: [{ name: null, presets: [] }, { name: 'Lead', presets: [] }],
    });
    expect(folderRowByText('Uncategorised').classList.contains('selected')).toBe(true);
  });

  it('preserves an in-flight search query across a setCorpus call', async () => {
    const panel = await loadBrowserPanel();
    panel.setOpen(true);
    panel.setCorpus({ factory: [], user: [{ name: 'Bass', presets: [] }] });
    const input = document.getElementById('browser-search-input');
    input.value = 'sub';
    input.dispatchEvent(new Event('input'));
    panel.setCorpus({
      factory: [],
      user: [{ name: 'Bass', presets: [] }, { name: 'Lead', presets: [] }],
    });
    expect(input.value).toBe('sub');
  });
});

describe('browserPanel.followPath', () => {
  it("selects the path's folder, clears search, and scrolls the row into view", async () => {
    const scrollSpy = vi.fn();
    Element.prototype.scrollIntoView = scrollSpy;

    const panel = await loadBrowserPanel();
    panel.setOpen(true);
    panel.setCorpus({
      factory: [],
      user: [
        { name: 'Bass', presets: [{ name: 'Sub', path: '/u/Bass/Sub.preset' }] },
        { name: 'Lead', presets: [{ name: 'Brass', path: '/u/Lead/Brass.preset' }] },
      ],
    });
    const input = document.getElementById('browser-search-input');
    input.value = 'brass';
    input.dispatchEvent(new Event('input'));
    expect(input.value).toBe('brass');

    panel.followPath('/u/Lead/Brass.preset');

    expect(folderRowByText('Lead').classList.contains('selected')).toBe(true);
    expect(input.value).toBe('');
    expect(scrollSpy).toHaveBeenCalled();
    // The scroll target is the preset row carrying `data-path` matching.
    const presetRow = document.querySelector(
      '#browser-presets .browser-row[data-path="/u/Lead/Brass.preset"]',
    );
    expect(presetRow).not.toBeNull();
  });

  it('is a no-op for an unknown path', async () => {
    const panel = await loadBrowserPanel();
    panel.setOpen(true);
    panel.setCorpus({
      factory: [],
      user: [{ name: 'Bass', presets: [{ name: 'Sub', path: '/u/Bass/Sub.preset' }] }],
    });
    expect(() => panel.followPath('/nowhere.preset')).not.toThrow();
  });
});

describe('browserPanel.setCurrentSource', () => {
  it('clears the .current class on every preset row when called with null', async () => {
    const panel = await loadBrowserPanel();
    panel.setOpen(true);
    panel.setCorpus({
      factory: [{ category: 'Bass', presets: [{ name: 'A', index: 0 }] }],
      user: [],
    });
    folderRowByText('Bass').click();
    panel.setCurrentSource({ kind: 'factory', index: 0 });
    expect(document.querySelectorAll('#browser-presets .browser-row.current').length).toBe(1);
    panel.setCurrentSource(null);
    expect(document.querySelectorAll('#browser-presets .browser-row.current').length).toBe(0);
  });
});

describe('browserPanel.openSaveAs', () => {
  it('disables Save while the name is empty; flips once promptText commits one', async () => {
    ({ sendCalls } = installVxn(BROWSER_OPCODES));
    const panel = await loadBrowserPanel();
    panel.openSaveAs('');
    const okBtn = document.querySelector(
      '.browser-modal-actions .browser-modal-btn:last-child',
    );
    expect(okBtn.disabled).toBe(true);
    // Cancel-style commit (promptText calls back with null) keeps it disabled.
    document.querySelector('.save-as-row .browser-modal-btn').click();
    expect(okBtn.disabled).toBe(true);
    // Swap to a real value and click Edit again — the gate flips.
    window.vxn.promptText = (title, initial, cb) => cb('Pad 1');
    document.querySelector('.save-as-row .browser-modal-btn').click();
    expect(okBtn.disabled).toBe(false);
    okBtn.click();
    expect(sendCalls).toEqual([['savePreset', 'Pad 1', null]]);
  });

  it('uses the currently-selected user folder as the save target', async () => {
    const panel = await loadBrowserPanel();
    panel.setOpen(true);
    panel.setCorpus({
      factory: [],
      user: [{ name: 'Lead', presets: [] }],
    });
    folderRowByText('Lead').click();
    panel.openSaveAs('My Patch');
    const okBtn = document.querySelector(
      '.browser-modal-actions .browser-modal-btn:last-child',
    );
    expect(okBtn.disabled).toBe(false);
    okBtn.click();
    expect(sendCalls).toEqual([['savePreset', 'My Patch', 'Lead']]);
  });
});
