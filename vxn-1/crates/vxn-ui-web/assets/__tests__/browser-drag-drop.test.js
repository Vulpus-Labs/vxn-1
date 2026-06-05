import { describe, it, expect, beforeEach, vi } from 'vitest';
import { browserDOM, installVxn, loadBrowserPanel } from './_helpers.js';

// 0093: HTML5 DnD preset moves. jsdom omits `DataTransfer`; stub a minimal
// recording shim per dispatched event (setData / dropEffect / effectAllowed
// are all browser.js touches).

const OPCODES = [
  'movePreset', 'loadFactory', 'loadUser',
  'renamePreset', 'deletePreset',
  'renameFolder', 'deleteFolder', 'newFolder',
  'stepPreset', 'savePreset',
];

let sendCalls;

function dragEvt(type) {
  const ev = new Event(type, { bubbles: true, cancelable: true });
  const dt = {
    _data: new Map(),
    setData(k, v) { this._data.set(k, v); },
    getData(k) { return this._data.get(k); },
    effectAllowed: '',
    dropEffect: '',
  };
  Object.defineProperty(ev, 'dataTransfer', { value: dt });
  return ev;
}

function folderRow(text) {
  return Array.from(document.querySelectorAll('#browser-folders .browser-row'))
    .find((r) => r.textContent === text) || null;
}
function presetRow(name) {
  return Array.from(document.querySelectorAll('#browser-presets .browser-row'))
    .find((r) => r.textContent === name) || null;
}
function seedCorpus(panel) {
  panel.setOpen(true);
  panel.setCorpus({
    factory: [],
    user: [
      { name: 'Bass', presets: [{ name: 'Sub', path: '/u/Bass/Sub.preset' }] },
      { name: 'Lead', presets: [] },
    ],
  });
  folderRow('Bass').click();
}

beforeEach(() => {
  document.body.innerHTML = browserDOM();
  ({ sendCalls } = installVxn(OPCODES));
});

describe('browserPanel drag source — dragstart', () => {
  it('stamps vxn/preset on DataTransfer, sets effectAllowed=move, adds .dragging', async () => {
    const panel = await loadBrowserPanel();
    seedCorpus(panel);
    const row = presetRow('Sub');
    expect(row.draggable).toBe(true);
    const ev = dragEvt('dragstart');
    row.dispatchEvent(ev);
    expect(ev.dataTransfer.getData('vxn/preset')).toBe('/u/Bass/Sub.preset');
    expect(ev.dataTransfer.effectAllowed).toBe('move');
    expect(row.classList.contains('dragging')).toBe(true);
  });
});

describe('browserPanel drop target — dragover/dragleave/drop', () => {
  it('dragover on a different user folder preventDefaults, sets dropEffect=move, adds .drag-over', async () => {
    const panel = await loadBrowserPanel();
    seedCorpus(panel);
    presetRow('Sub').dispatchEvent(dragEvt('dragstart'));

    const target = folderRow('Lead');
    const over = dragEvt('dragover');
    const pd = vi.spyOn(over, 'preventDefault');
    target.dispatchEvent(over);
    expect(pd).toHaveBeenCalled();
    expect(over.dataTransfer.dropEffect).toBe('move');
    expect(target.classList.contains('drag-over')).toBe(true);
  });

  it('dragover on the source folder adds .drag-blocked and does NOT preventDefault', async () => {
    const panel = await loadBrowserPanel();
    seedCorpus(panel);
    presetRow('Sub').dispatchEvent(dragEvt('dragstart'));

    const source = folderRow('Bass');
    const over = dragEvt('dragover');
    const pd = vi.spyOn(over, 'preventDefault');
    source.dispatchEvent(over);
    expect(pd).not.toHaveBeenCalled();
    expect(source.classList.contains('drag-blocked')).toBe(true);
    expect(source.classList.contains('drag-over')).toBe(false);
  });

  it('dragleave clears both .drag-over and .drag-blocked', async () => {
    const panel = await loadBrowserPanel();
    seedCorpus(panel);
    presetRow('Sub').dispatchEvent(dragEvt('dragstart'));

    const target = folderRow('Lead');
    target.dispatchEvent(dragEvt('dragover'));
    expect(target.classList.contains('drag-over')).toBe(true);
    target.dispatchEvent(dragEvt('dragleave'));
    expect(target.classList.contains('drag-over')).toBe(false);
    expect(target.classList.contains('drag-blocked')).toBe(false);
  });

  it('drop on a valid target sends movePreset and clears highlight; drop on source sends nothing', async () => {
    const panel = await loadBrowserPanel();
    seedCorpus(panel);
    presetRow('Sub').dispatchEvent(dragEvt('dragstart'));

    const target = folderRow('Lead');
    target.dispatchEvent(dragEvt('dragover'));
    target.dispatchEvent(dragEvt('drop'));
    expect(sendCalls).toEqual([['movePreset', '/u/Bass/Sub.preset', 'Lead']]);
    expect(target.classList.contains('drag-over')).toBe(false);

    sendCalls.length = 0;
    presetRow('Sub').dispatchEvent(dragEvt('dragstart'));
    folderRow('Bass').dispatchEvent(dragEvt('drop'));
    expect(sendCalls).toEqual([]);
  });

  it('dragend clears dragSourcePath (next dragover is a no-op) and any stuck highlights', async () => {
    const panel = await loadBrowserPanel();
    seedCorpus(panel);
    const row = presetRow('Sub');
    row.dispatchEvent(dragEvt('dragstart'));

    const target = folderRow('Lead');
    target.dispatchEvent(dragEvt('dragover'));
    expect(target.classList.contains('drag-over')).toBe(true);

    row.dispatchEvent(dragEvt('dragend'));
    expect(row.classList.contains('dragging')).toBe(false);
    expect(target.classList.contains('drag-over')).toBe(false);

    // Next dragover with no active source must be a no-op (no preventDefault,
    // no highlight) since dragSourcePath was cleared.
    const over = dragEvt('dragover');
    const pd = vi.spyOn(over, 'preventDefault');
    target.dispatchEvent(over);
    expect(pd).not.toHaveBeenCalled();
    expect(target.classList.contains('drag-over')).toBe(false);
  });
});
