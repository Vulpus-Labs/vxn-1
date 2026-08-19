// Copy Layer 1 → Layer 2 (0265), moved to the preset bar behind a
// confirmation modal.
//
// The op itself is engine-side (`SharedParams::copy_layer`, covered in
// shared.rs); what needs testing here is that it cannot fire *unconfirmed*. The
// copy overwrites whatever Layer 2 held and lands ~66 param changes in the
// host's undo stack as one burst, so every dismissal route — Cancel, the
// backdrop, Esc — must leave Layer 2 alone.
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { wireCopyLayer, confirmDialog } from '../dispatch.js';

// The confirm markup as it appears in faceplate.html, plus the bar button.
function mountBar() {
  document.body.innerHTML = `
    <button id="copy-layer" type="button">Copy L1 &rarr; L2</button>
    <div class="overlay-backdrop confirm-backdrop" id="confirm-backdrop" hidden>
      <div class="overlay-panel confirm-panel">
        <div class="overlay-title" id="confirm-title"></div>
        <div class="confirm-message" id="confirm-message"></div>
        <div class="confirm-actions">
          <button id="confirm-cancel" type="button">Cancel</button>
          <button id="confirm-ok" type="button">OK</button>
        </div>
      </div>
    </div>
  `;
  return {
    btn: document.getElementById('copy-layer'),
    backdrop: document.getElementById('confirm-backdrop'),
    ok: document.getElementById('confirm-ok'),
    cancel: document.getElementById('confirm-cancel'),
  };
}

const click = (el) => el.dispatchEvent(new MouseEvent('click', { bubbles: true }));
const esc = () =>
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));

describe('copy layer button (0265)', () => {
  let sent;

  beforeEach(() => {
    sent = [];
    globalThis.window.vxn = { send: { copyLayer: (from, to) => sent.push([from, to]) } };
  });

  afterEach(() => {
    document.body.innerHTML = '';
  });

  it('opens the confirmation instead of copying', () => {
    const { btn, backdrop } = mountBar();
    wireCopyLayer();

    click(btn);
    expect(sent).toEqual([]);
    expect(backdrop.hidden).toBe(false);
    expect(document.getElementById('confirm-title').textContent).toMatch(/Layer 1/);
  });

  it('copies upper → lower once the confirm button is pressed', () => {
    const { btn, backdrop, ok } = mountBar();
    wireCopyLayer();

    click(btn);
    click(ok);
    expect(sent).toEqual([['upper', 'lower']]);
    expect(backdrop.hidden).toBe(true);
  });

  it('fires exactly once per confirmation — listeners do not accumulate', () => {
    const { btn, ok } = mountBar();
    wireCopyLayer();

    // Open/cancel/open again: the first round's handler must be gone, or the
    // second confirm would post two copies.
    click(btn);
    click(document.getElementById('confirm-cancel'));
    click(btn);
    click(ok);
    expect(sent).toEqual([['upper', 'lower']]);

    // A stray click on the now-closed dialogue's OK does nothing.
    click(ok);
    expect(sent).toEqual([['upper', 'lower']]);
  });

  it('cancels on the Cancel button, the backdrop and Esc', () => {
    const { btn, backdrop, cancel } = mountBar();
    wireCopyLayer();

    click(btn);
    click(cancel);
    expect(backdrop.hidden).toBe(true);

    click(btn);
    click(backdrop);
    expect(backdrop.hidden).toBe(true);

    click(btn);
    esc();
    expect(backdrop.hidden).toBe(true);

    expect(sent).toEqual([]);
  });

  it('stays open when the click lands inside the panel', () => {
    const { btn, backdrop } = mountBar();
    wireCopyLayer();

    click(btn);
    // Clicks inside bubble to the backdrop; dismissing on those would make the
    // message impossible to read without closing it.
    click(document.getElementById('confirm-message'));
    expect(backdrop.hidden).toBe(false);
    expect(sent).toEqual([]);
  });
});

describe('confirmDialog', () => {
  afterEach(() => {
    document.body.innerHTML = '';
  });

  it('does nothing at all without the dialogue markup', () => {
    document.body.innerHTML = '';
    let fired = false;
    // An unconfirmable destructive action must not silently become an
    // unconfirmed one.
    expect(() => confirmDialog.ask({ title: 't' }, () => { fired = true; })).not.toThrow();
    expect(fired).toBe(false);
  });

  it('labels the confirm button from the caller', () => {
    mountBar();
    confirmDialog.ask({ title: 'T', message: 'M', okLabel: 'Copy' }, () => {});
    expect(document.getElementById('confirm-ok').textContent).toBe('Copy');
    expect(document.getElementById('confirm-message').textContent).toBe('M');
  });

  it('leaves no keydown listener behind after closing', () => {
    mountBar();
    let fired = 0;
    confirmDialog.ask({ title: 'T' }, () => { fired += 1; });
    esc();
    // The dialogue is gone; a later Esc must not reach a stale handler.
    esc();
    expect(fired).toBe(0);
    expect(document.getElementById('confirm-backdrop').hidden).toBe(true);
  });
});
