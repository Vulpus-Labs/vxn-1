import { describe, it, expect, beforeEach } from 'vitest';
import { browserDOM, installVxn, loadBrowserPanel } from './_helpers.js';

// 0093: search input filters across the whole corpus; clear button resets.
// User-side hits keep contextmenu + draggable; factory hits don't.

const OPCODES = [
  'loadFactory', 'loadUser',
  'movePreset', 'renamePreset', 'deletePreset',
  'renameFolder', 'deleteFolder', 'newFolder',
  'stepPreset', 'savePreset',
];

let sendCalls;

function ctxEvt() {
  return new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 10, clientY: 10 });
}
function typeQuery(q) {
  const input = document.getElementById('browser-search-input');
  input.value = q;
  input.dispatchEvent(new Event('input'));
  return input;
}
function seedRichCorpus(panel) {
  panel.setOpen(true);
  panel.setCorpus({
    factory: [
      { category: 'Bass', presets: [{ name: 'BassDrop', index: 0 }] },
      { category: 'Lead', presets: [{ name: 'Brass', index: 1 }] },
    ],
    user: [
      { name: 'Bass', presets: [{ name: 'BassLine', path: '/u/Bass/BassLine.preset' }] },
      { name: 'Pad',  presets: [{ name: 'Warm', path: '/u/Pad/Warm.preset' }] },
    ],
  });
}

beforeEach(() => {
  document.body.innerHTML = browserDOM();
  ({ sendCalls } = installVxn(OPCODES));
});

describe('browserPanel search', () => {
  it('typing into the input filters the preset pane to matches with origin labels', async () => {
    const panel = await loadBrowserPanel();
    seedRichCorpus(panel);
    typeQuery('bass');                                  // case-insensitive
    const rows = document.querySelectorAll('#browser-presets .browser-row');
    const names = Array.from(rows).map((r) => r.querySelector('.browser-row-name').textContent);
    expect(names).toEqual(['BassDrop', 'BassLine']);   // factory first, then user
    for (const r of rows) {
      expect(r.querySelector('.browser-row-origin')).not.toBeNull();
    }
  });

  it('a query with no matches renders the "No matches" placeholder', async () => {
    const panel = await loadBrowserPanel();
    seedRichCorpus(panel);
    typeQuery('zzzzz');
    const empty = document.querySelector('#browser-presets .browser-empty');
    expect(empty).not.toBeNull();
    expect(empty.textContent).toBe('No matches');
  });

  it('clear button resets the input value and re-renders the folder pane', async () => {
    const panel = await loadBrowserPanel();
    seedRichCorpus(panel);
    const input = typeQuery('bass');
    expect(input.value).toBe('bass');
    document.getElementById('browser-search-clear').click();
    expect(input.value).toBe('');
    // After clear we're back on the user-root selection (default) which is
    // not present in the corpus → "No presets" placeholder.
    expect(document.querySelector('#browser-presets .browser-empty')).not.toBeNull();
  });

  it('user-side hits carry contextmenu + draggable; factory hits carry neither', async () => {
    const panel = await loadBrowserPanel();
    seedRichCorpus(panel);
    typeQuery('bass');
    const allRows = Array.from(document.querySelectorAll('#browser-presets .browser-row'));
    const userHit    = allRows.find((r) => r.dataset.path === '/u/Bass/BassLine.preset');
    const factoryHit = allRows.find((r) => !r.dataset.path);
    expect(userHit.draggable).toBe(true);
    expect(factoryHit.draggable).toBe(false);

    // Factory contextmenu → no menu opens (no handler).
    factoryHit.dispatchEvent(ctxEvt());
    expect(document.querySelector('.browser-menu')).toBeNull();

    // User contextmenu → menu opens.
    userHit.dispatchEvent(ctxEvt());
    expect(document.querySelector('.browser-menu')).not.toBeNull();
  });
});
