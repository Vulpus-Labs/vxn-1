// Reset the edit layer to the factory patch (0307).
//
// The op itself is engine-side (`SharedParams::reset_layer`, covered in
// shared.rs); the confirmation machinery is covered once in copy-layer.test.js.
// What is specific to this button is that it has no fixed direction — it acts
// on whichever layer the tab is on — so the two things worth pinning here are
// that the label says which layer, and that the opcode names the same one.
//
// Getting that wrong is the expensive kind of bug: a button reading "Reset L1"
// that blanks Layer 2 destroys work the player did not agree to lose, and the
// confirmation would have said the reassuring thing on the way past.
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { wireResetLayer, syncResetLayerLabel, model } from '../dispatch.js';

function mountBar() {
  document.body.innerHTML = `
    <button id="reset-layer" type="button">Reset L1</button>
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
    btn: document.getElementById('reset-layer'),
    backdrop: document.getElementById('confirm-backdrop'),
    ok: document.getElementById('confirm-ok'),
  };
}

const click = (el) => el.dispatchEvent(new MouseEvent('click', { bubbles: true }));

describe('reset layer button (0307)', () => {
  let sent;

  beforeEach(() => {
    sent = [];
    globalThis.window.vxn = { send: { resetLayer: (layer) => sent.push(layer) } };
    model.currentLayer = 'upper';
  });

  afterEach(() => {
    document.body.innerHTML = '';
    model.currentLayer = 'upper';
  });

  it('opens the confirmation instead of resetting', () => {
    const { btn, backdrop } = mountBar();
    wireResetLayer();

    click(btn);
    expect(sent).toEqual([]);
    expect(backdrop.hidden).toBe(false);
  });

  it('resets the upper layer when the layer tab is on Layer 1', () => {
    const { btn, ok } = mountBar();
    wireResetLayer();

    click(btn);
    click(ok);
    expect(sent).toEqual(['upper']);
  });

  it('resets the lower layer when the layer tab is on Layer 2', () => {
    const { btn, ok } = mountBar();
    model.currentLayer = 'lower';
    wireResetLayer();

    click(btn);
    click(ok);
    expect(sent).toEqual(['lower']);
  });

  it('reads the edit layer at click time, not at wire time', () => {
    const { btn, ok } = mountBar();
    wireResetLayer();
    // Wired on Layer 1, flipped to Layer 2 before the click: the button must
    // follow the tab, not the state it was born in.
    model.currentLayer = 'lower';

    click(btn);
    click(ok);
    expect(sent).toEqual(['lower']);
  });

  it('labels itself for the current layer, and restamps on a flip', () => {
    const { btn } = mountBar();
    wireResetLayer();
    expect(btn.textContent).toBe('Reset L1');

    model.currentLayer = 'lower';
    syncResetLayerLabel();
    expect(btn.textContent).toBe('Reset L2');

    model.currentLayer = 'upper';
    syncResetLayerLabel();
    expect(btn.textContent).toBe('Reset L1');
  });

  it('names the layer it is about to blank in the confirmation', () => {
    const { btn } = mountBar();
    model.currentLayer = 'lower';
    wireResetLayer();

    click(btn);
    expect(document.getElementById('confirm-title').textContent).toBe('Reset Layer 2');
    // The mixer strip is the one place Reset and Copy disagree, so the message
    // must say so before the player loses a level they set.
    expect(document.getElementById('confirm-message').textContent).toMatch(/mixer strip/);
  });

  it('does nothing when the button is absent', () => {
    document.body.innerHTML = '';
    expect(() => wireResetLayer()).not.toThrow();
    expect(() => syncResetLayerLabel()).not.toThrow();
  });
});
