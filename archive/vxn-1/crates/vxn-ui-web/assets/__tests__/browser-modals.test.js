import { describe, it, expect, beforeEach } from 'vitest';
import { browserDOM, installVxn, loadBrowserPanel } from './_helpers.js';

// 0093: modal scaffold via the Delete-confirm entry point (the Save-As path
// is already covered by browser-invariants).

const OPCODES = [
  'deleteFolder', 'deletePreset', 'movePreset',
  'renameFolder', 'renamePreset', 'newFolder',
  'loadFactory', 'loadUser', 'stepPreset', 'savePreset',
];

let sendCalls;

function ctxEvt() {
  return new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 10, clientY: 10 });
}
function folderRow(text) {
  return Array.from(document.querySelectorAll('#browser-folders .browser-row'))
    .find((r) => r.textContent === text) || null;
}
function menuItem(text) {
  return Array.from(document.querySelectorAll('.browser-menu > .browser-menu-item'))
    .find((el) => el.textContent.trim() === text || el.firstChild?.textContent?.trim() === text) || null;
}
function openDeleteFolderModal() {
  folderRow('Bass').dispatchEvent(ctxEvt());
  menuItem('Delete').click();
}

beforeEach(() => {
  document.body.innerHTML = browserDOM();
  ({ sendCalls } = installVxn(OPCODES));
});

describe('browserPanel modals — Delete confirm', () => {
  it('backdrop click closes the modal', async () => {
    const panel = await loadBrowserPanel();
    panel.setOpen(true);
    panel.setCorpus({ factory: [], user: [{ name: 'Bass', presets: [] }] });
    openDeleteFolderModal();
    expect(document.querySelector('.browser-modal')).not.toBeNull();
    document.querySelector('.browser-modal-backdrop').click();
    expect(document.querySelector('.browser-modal')).toBeNull();
    expect(sendCalls).toEqual([]);
  });

  it('Cancel closes the modal without sending an opcode', async () => {
    const panel = await loadBrowserPanel();
    panel.setOpen(true);
    panel.setCorpus({ factory: [], user: [{ name: 'Bass', presets: [] }] });
    openDeleteFolderModal();
    document.querySelector('.browser-modal-actions .browser-modal-btn:first-child').click();
    expect(document.querySelector('.browser-modal')).toBeNull();
    expect(sendCalls).toEqual([]);
  });

  it('OK sends the bound opcode and closes the modal', async () => {
    const panel = await loadBrowserPanel();
    panel.setOpen(true);
    panel.setCorpus({ factory: [], user: [{ name: 'Bass', presets: [] }] });
    openDeleteFolderModal();
    document.querySelector('.browser-modal-actions .browser-modal-btn:last-child').click();
    expect(sendCalls).toEqual([['deleteFolder', 'Bass']]);
    expect(document.querySelector('.browser-modal')).toBeNull();
  });

  it('re-opens cleanly after a Cancel (no stale modal state)', async () => {
    const panel = await loadBrowserPanel();
    panel.setOpen(true);
    panel.setCorpus({ factory: [], user: [{ name: 'Bass', presets: [] }] });
    openDeleteFolderModal();
    document.querySelector('.browser-modal-actions .browser-modal-btn:first-child').click();
    openDeleteFolderModal();
    expect(document.querySelectorAll('.browser-modal').length).toBe(1);
    document.querySelector('.browser-modal-actions .browser-modal-btn:last-child').click();
    expect(sendCalls).toEqual([['deleteFolder', 'Bass']]);
  });
});
