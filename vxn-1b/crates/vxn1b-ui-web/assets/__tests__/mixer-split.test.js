import { describe, it, expect, beforeEach, vi } from 'vitest';
import * as dispatch from '../dispatch.js';

const {
  bindCell, model, wireSplit, SPLIT_MIN, SPLIT_MAX, SPLIT_DEFAULT,
  _resetParamIndex, _resetKeyStateView,
} = dispatch;

const PATCH_COUNT = 100;

// Minimal param fixture: `layer_level` is a per-patch param, so it has one id
// per layer — the whole point of the mixer's fixed-layer binding.
function seedParams() {
  globalThis.window = globalThis;
  window.vxn = {
    patchCount: PATCH_COUNT,
    params: {
      0:                 { name: 'layer_level', variants: [], min: 0, max: 1, default: 1 },
      [PATCH_COUNT]:     { name: 'layer_level', variants: [], min: 0, max: 1, default: 1 },
    },
    send: {
      setKeyMode: vi.fn(),
      setSplitPoint: vi.fn(),
      discrete: vi.fn(),
      beginGesture: vi.fn(),
      endGesture: vi.fn(),
      setParamNorm: vi.fn(),
    },
  };
  _resetParamIndex();
}

beforeEach(() => {
  document.body.innerHTML = '';
  seedParams();
  madeIds = [];
  stubPrimitives();
  // The view's KeyState mirrors are module-level (one long-lived page in
  // production), so reset them or one test's toggles leak into the next.
  _resetKeyStateView();
});

// Stub the primitive factories on `globalThis`, the way the orchestration
// suite does: dispatch.js resolves them from the concatenated scope in
// production, so an import would shadow them (and did, once).
function stubPrimitives() {
  const factory = () => (el, id) => {
    madeIds.push(id);
    return { update: vi.fn() };
  };
  globalThis.makeFader = factory();
  globalThis.makeSwitch = factory();
}
let madeIds = [];

// A cell as the faceplate mounts it — `makeFader` reads `data-label`.
function mountCell() {
  const el = document.createElement('div');
  el.dataset.label = 'Level';
  document.body.appendChild(el);
  return el;
}

describe('data-fixed-layer', () => {
  it('binds to the named layer, ignoring the current edit layer', () => {
    // This is what lets the mixer show BOTH layer strips at once: a fixed-layer
    // cell must not follow the edit-layer tab the way a `data-layered` one does.
    const el = mountCell();
    const entry = { el, kind: 'fader', name: 'layer_level', fixedLayer: 'lower' };

    // Editing layer 1, but the cell is pinned to layer 2.
    const bound = bindCell(entry, 'upper');
    expect(bound.ids).toEqual([PATCH_COUNT]);

    // And it stays pinned when the edit layer flips.
    document.body.innerHTML = '';
    const again = bindCell({ ...entry, el: mountCell() }, 'lower');
    expect(again.ids).toEqual([PATCH_COUNT]);
  });

  it('an upper-pinned cell resolves to the layer 1 id from either tab', () => {
    const entry = { el: mountCell(), kind: 'fader', name: 'layer_level', fixedLayer: 'upper' };
    expect(bindCell(entry, 'lower').ids).toEqual([0]);
  });

  it('without the marker a cell follows the edit layer as before', () => {
    const entry = { el: mountCell(), kind: 'fader', name: 'layer_level', fixedLayer: null };
    expect(bindCell(entry, 'lower').ids).toEqual([PATCH_COUNT]);
  });
});

// The split UI writes KeyState, not params, and KeyMode is DERIVED — so the
// interesting behaviour is which mode index gets posted.
function mountSplit() {
  document.body.innerHTML = `
    <div class="split-enable-slot" id="split-enable"></div>
    <input type="range" id="split-point-slider" min="12" max="96" step="1" value="60" />
    <div id="split-point-readout"></div>
  `;
  wireSplit();
  return {
    toggle: document.querySelector('#split-enable .ctl-tg-row'),
    slider: document.getElementById('split-point-slider'),
    readout: document.getElementById('split-point-readout'),
  };
}

function press(el) {
  el.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, cancelable: true }));
}

describe('wireSplit — enable toggle', () => {
  it('does not post while Layer 2 is off', () => {
    // A split with one layer is meaningless, and posting Split (2) would turn
    // Layer 2 on as a side effect, because KeyMode is derived from both toggles.
    const { toggle } = mountSplit();
    model.setLayer2On && model.setLayer2On(false);
    press(toggle);
    expect(window.vxn.send.setKeyMode).not.toHaveBeenCalled();
    // The flag is still remembered locally, for when Layer 2 comes back.
    expect(toggle.classList.contains('active')).toBe(true);
  });

  it('posts Split (2) and Dual (1) once Layer 2 is on', () => {
    document.body.innerHTML = `
      <span id="layer2-enable"></span>
      <div class="split-enable-slot" id="split-enable"></div>
      <input type="range" id="split-point-slider" min="12" max="96" value="60" />
      <div id="split-point-readout"></div>
    `;
    dispatch.wireLayer2Toggle();
    wireSplit();
    const toggle = document.querySelector('#split-enable .ctl-tg-row');

    document.getElementById('layer2-enable').dispatchEvent(
      new MouseEvent('click', { bubbles: true }),
    );
    expect(window.vxn.send.setKeyMode).toHaveBeenLastCalledWith(1); // Dual

    press(toggle);
    expect(window.vxn.send.setKeyMode).toHaveBeenLastCalledWith(2); // Split
    press(toggle);
    expect(window.vxn.send.setKeyMode).toHaveBeenLastCalledWith(1); // back to Dual
  });

  it('turning Layer 2 on with split already armed goes straight to Split', () => {
    document.body.innerHTML = `
      <span id="layer2-enable"></span>
      <div class="split-enable-slot" id="split-enable"></div>
      <input type="range" id="split-point-slider" min="12" max="96" value="60" />
      <div id="split-point-readout"></div>
    `;
    dispatch.wireLayer2Toggle();
    wireSplit();
    // Arm the split while Layer 2 is off — nothing posted yet.
    press(document.querySelector('#split-enable .ctl-tg-row'));
    expect(window.vxn.send.setKeyMode).not.toHaveBeenCalled();
    // Now enable Layer 2: the derived mode must carry BOTH toggles.
    document.getElementById('layer2-enable').dispatchEvent(
      new MouseEvent('click', { bubbles: true }),
    );
    expect(window.vxn.send.setKeyMode).toHaveBeenLastCalledWith(2);
  });
});

describe('wireSplit — split point', () => {
  it('posts the slider note and repaints the readout optimistically', () => {
    const { slider, readout } = mountSplit();
    slider.value = '48';
    slider.dispatchEvent(new Event('input', { bubbles: true }));
    expect(window.vxn.send.setSplitPoint).toHaveBeenCalledWith(48);
    expect(readout.textContent).toBe('C3');
  });

  it('clamps to the usable range rather than the full MIDI span', () => {
    const { slider } = mountSplit();
    slider.value = '200';
    slider.dispatchEvent(new Event('input', { bubbles: true }));
    expect(window.vxn.send.setSplitPoint).toHaveBeenCalledWith(SPLIT_MAX);
    slider.value = '0';
    slider.dispatchEvent(new Event('input', { bubbles: true }));
    expect(window.vxn.send.setSplitPoint).toHaveBeenCalledWith(SPLIT_MIN);
  });

  it('reflects an engine echo without posting back', () => {
    // A state / preset load must not bounce an opcode at the engine.
    const { slider, readout } = mountSplit();
    model.setSplitPoint(72);
    expect(slider.value).toBe('72');
    expect(readout.textContent).toBe('C5');
    expect(window.vxn.send.setSplitPoint).not.toHaveBeenCalled();
  });

  it('double-click restores the default point', () => {
    const { slider, readout } = mountSplit();
    slider.dispatchEvent(new MouseEvent('dblclick', { bubbles: true, cancelable: true }));
    expect(window.vxn.send.setSplitPoint).toHaveBeenCalledWith(SPLIT_DEFAULT);
    expect(readout.textContent).toBe('C4');
  });
});

describe('split point row dimming', () => {
  it('is dimmed while the split is off and lit once it is on', () => {
    document.body.innerHTML = `
      <div class="mixer-split">
        <div class="split-point-row" id="split-point-row">
          <input type="range" id="split-point-slider" min="12" max="96" value="60" />
          <div id="split-point-readout"></div>
        </div>
        <div class="split-enable-slot" id="split-enable"></div>
      </div>
    `;
    wireSplit();
    const pointRow = document.getElementById('split-point-row');
    const toggle = document.querySelector('#split-enable .ctl-tg-row');
    // Off by default: the point is inert, so grey it out.
    expect(pointRow.classList.contains('dimmed')).toBe(true);

    press(toggle);
    expect(pointRow.classList.contains('dimmed')).toBe(false);
    press(toggle);
    expect(pointRow.classList.contains('dimmed')).toBe(true);
  });

  it('follows an engine echo too', () => {
    document.body.innerHTML = `
      <div class="mixer-split">
        <div class="split-point-row" id="split-point-row">
          <input type="range" id="split-point-slider" min="12" max="96" value="60" />
          <div id="split-point-readout"></div>
        </div>
        <div class="split-enable-slot" id="split-enable"></div>
      </div>
    `;
    wireSplit();
    model.setSplitEnabled(true);
    expect(document.getElementById('split-point-row').classList.contains('dimmed')).toBe(false);
  });
});

describe('repaintAllControls', () => {
  it('re-applies each control\'s cached value', () => {
    // What a tab switch calls: the pane just revealed was `display: none`, so
    // every fader in it has an unplaced thumb until this runs.
    const { model: m, repaintAllControls } = dispatch;
    m.controls.clear();
    m.lastParam.clear();
    const updated = [];
    m.controls.set(7, [{ update: (p, n, d) => updated.push([p, n, d]) }]);
    m.lastParam.set(7, { plain: 440, norm: 0.5, display: '440 Hz' });
    repaintAllControls();
    expect(updated).toEqual([[440, 0.5, '440 Hz']]);
  });

  it('skips ids with no cached value rather than pushing undefined', () => {
    const { model: m, repaintAllControls } = dispatch;
    m.controls.clear();
    m.lastParam.clear();
    let called = false;
    m.controls.set(9, [{ update: () => { called = true; } }]);
    repaintAllControls();
    expect(called).toBe(false);
  });
});
