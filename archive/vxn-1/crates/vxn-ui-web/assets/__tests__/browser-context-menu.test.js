import { describe, it, expect, beforeEach } from 'vitest';
import { browserDOM, installVxn, loadBrowserPanel } from './_helpers.js';

// 0093: Right-click context menu on user folder / preset rows.
// Factory rows are read-only and have no contextmenu handler.

const OPCODES = [
  'renameFolder', 'deleteFolder',
  'renamePreset', 'deletePreset', 'movePreset',
  'newFolder', 'loadFactory', 'loadUser',
  'stepPreset', 'savePreset',
];

let sendCalls;

function ctxEvt() {
  return new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 10, clientY: 10 });
}
function folderRow(text) {
  return Array.from(document.querySelectorAll('#browser-folders .browser-row'))
    .find((r) => r.textContent === text) || null;
}
function presetRow(name) {
  return Array.from(document.querySelectorAll('#browser-presets .browser-row'))
    .find((r) => r.textContent === name || r.querySelector('.browser-row-name')?.textContent === name) || null;
}
function menuItem(text) {
  return Array.from(document.querySelectorAll('.browser-menu > .browser-menu-item'))
    .find((el) => el.textContent.trim() === text || el.firstChild?.textContent?.trim() === text) || null;
}

beforeEach(() => {
  document.body.innerHTML = browserDOM();
  ({ sendCalls } = installVxn(OPCODES, { promptValue: 'Renamed' }));
});

describe('browserPanel context-menu — user folder', () => {
  it('right-click opens the menu and Rename sends renameFolder', async () => {
    const panel = await loadBrowserPanel();
    panel.setOpen(true);
    panel.setCorpus({ factory: [], user: [{ name: 'Bass', presets: [] }] });
    folderRow('Bass').dispatchEvent(ctxEvt());
    expect(document.querySelector('.browser-menu')).not.toBeNull();
    menuItem('Rename').click();
    expect(sendCalls).toEqual([['renameFolder', 'Bass', 'Renamed']]);
  });

  it('Rename is a no-op when promptText returns null, empty, or unchanged', async () => {
    const panel = await loadBrowserPanel();
    panel.setOpen(true);
    panel.setCorpus({ factory: [], user: [{ name: 'Bass', presets: [] }] });

    window.vxn.promptText = (_t, _i, cb) => cb(null);
    folderRow('Bass').dispatchEvent(ctxEvt());
    menuItem('Rename').click();
    expect(sendCalls).toEqual([]);

    window.vxn.promptText = (_t, _i, cb) => cb('   ');
    folderRow('Bass').dispatchEvent(ctxEvt());
    menuItem('Rename').click();
    expect(sendCalls).toEqual([]);

    window.vxn.promptText = (_t, _i, cb) => cb('Bass');
    folderRow('Bass').dispatchEvent(ctxEvt());
    menuItem('Rename').click();
    expect(sendCalls).toEqual([]);
  });

  it('Delete opens the confirm modal; OK sends deleteFolder, Cancel sends nothing', async () => {
    const panel = await loadBrowserPanel();
    panel.setOpen(true);
    panel.setCorpus({ factory: [], user: [{ name: 'Bass', presets: [] }] });

    folderRow('Bass').dispatchEvent(ctxEvt());
    menuItem('Delete').click();
    const cancel = document.querySelector('.browser-modal-actions .browser-modal-btn:first-child');
    expect(cancel.textContent).toBe('Cancel');
    cancel.click();
    expect(document.querySelector('.browser-modal')).toBeNull();
    expect(sendCalls).toEqual([]);

    folderRow('Bass').dispatchEvent(ctxEvt());
    menuItem('Delete').click();
    const ok = document.querySelector('.browser-modal-actions .browser-modal-btn:last-child');
    ok.click();
    expect(sendCalls).toEqual([['deleteFolder', 'Bass']]);
    expect(document.querySelector('.browser-modal')).toBeNull();
  });
});

describe('browserPanel context-menu — user preset', () => {
  it('Rename on a preset row sends renamePreset', async () => {
    const panel = await loadBrowserPanel();
    panel.setOpen(true);
    panel.setCorpus({
      factory: [],
      user: [{ name: 'Bass', presets: [{ name: 'Sub', path: '/u/Bass/Sub.preset' }] }],
    });
    folderRow('Bass').click();
    presetRow('Sub').dispatchEvent(ctxEvt());
    menuItem('Rename').click();
    expect(sendCalls).toEqual([['renamePreset', '/u/Bass/Sub.preset', 'Renamed']]);
  });

  it('Delete on a preset row → modal OK sends deletePreset', async () => {
    const panel = await loadBrowserPanel();
    panel.setOpen(true);
    panel.setCorpus({
      factory: [],
      user: [{ name: 'Bass', presets: [{ name: 'Sub', path: '/u/Bass/Sub.preset' }] }],
    });
    folderRow('Bass').click();
    presetRow('Sub').dispatchEvent(ctxEvt());
    menuItem('Delete').click();
    document.querySelector('.browser-modal-actions .browser-modal-btn:last-child').click();
    expect(sendCalls).toEqual([['deletePreset', '/u/Bass/Sub.preset']]);
  });

  it('Move to ▸ submenu lists user folders excluding the current; click sends movePreset', async () => {
    const panel = await loadBrowserPanel();
    panel.setOpen(true);
    panel.setCorpus({
      factory: [],
      user: [
        { name: 'Bass', presets: [{ name: 'Sub', path: '/u/Bass/Sub.preset' }] },
        { name: 'Lead', presets: [] },
        { name: 'Pad',  presets: [] },
      ],
    });
    folderRow('Bass').click();
    presetRow('Sub').dispatchEvent(ctxEvt());
    const subItems = Array.from(document.querySelectorAll('.browser-submenu-item'));
    const labels = subItems.map((el) => el.textContent);
    expect(labels).toEqual(['Lead', 'Pad']);    // alpha, excludes 'Bass' (current)
    subItems.find((el) => el.textContent === 'Lead').click();
    expect(sendCalls).toEqual([['movePreset', '/u/Bass/Sub.preset', 'Lead']]);
  });
});

describe('browserPanel context-menu — factory rows are read-only', () => {
  it('right-click on a factory folder row opens no menu', async () => {
    const panel = await loadBrowserPanel();
    panel.setOpen(true);
    panel.setCorpus({
      factory: [{ category: 'Bass', presets: [{ name: 'Wobble', index: 0 }] }],
      user: [],
    });
    folderRow('Bass').dispatchEvent(ctxEvt());
    expect(document.querySelector('.browser-menu')).toBeNull();
  });
});
