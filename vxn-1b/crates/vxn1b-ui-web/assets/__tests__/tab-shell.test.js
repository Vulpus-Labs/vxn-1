// 0219: the three-tab shell wiring — Layer 1 / Layer 2 / FX-Global tabs and the
// Layer 2 on/off toggle. Layer tabs flip `model.currentLayer` and rebind the
// layer pane's cells (the 0045 machinery, now live at patchCount = 64); the
// FX/Global tab swaps panes without touching the edit layer. Mirrors the
// splice-scope stubbing the orchestration suite uses (dispatch.js imports
// nothing — cross-module symbols resolve as free globals).
import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  model,
  wireTabs,
  wireLayer2Toggle,
  paramIdByNameAtLayer,
  _resetParamIndex,
} from '../dispatch.js';
import { installFixture, PATCH_COUNT } from '../fixtures/params.js';

function resetModel() {
  model.controls.clear();
  model.lastParam.clear();
  model.syncOfRate.clear();
  model.rateOfSync.clear();
  model.tunedOfCutoff.clear();
  model.cutoffOfTuned.clear();
  model.dimRules.length = 0;
  model.dimRuleSpecs.length = 0;
  model.cells.length = 0;
  model.currentLayer = 'upper';
  delete model.setLayer2On;
}

function stubGlobals() {
  globalThis.keysPanel = { wireLayerLevels: vi.fn() };
  globalThis.matrixOverlay = { build: vi.fn(), refreshForLayer: vi.fn() };
}

let sends;

beforeEach(() => {
  installFixture();
  // Real two-layer surface: lower id = upper id + patchCount (unlike the
  // orchestration suite, which pins patchCount = 1 to keep flips inert).
  window.vxn.patchCount = PATCH_COUNT;
  window.vxn.send = {
    setEditLayer: vi.fn((layer) => sends.push(['edit', layer])),
    setKeyMode: vi.fn((mode) => sends.push(['keymode', mode])),
  };
  sends = [];
  _resetParamIndex();
  resetModel();
  stubGlobals();
  document.body.innerHTML = `
    <div id="tab-strip">
      <button class="tab-btn active" data-tab="layer" data-layer="upper">LAYER 1</button>
      <button class="tab-btn" data-tab="layer" data-layer="lower">
        <span class="tab-switch" id="layer2-enable"></span>LAYER 2
      </button>
      <button class="tab-btn" data-tab="global">FX / GLOBAL</button>
    </div>
    <div class="tab-pane active" data-tab-pane="layer" data-edit-layer="upper"></div>
    <div class="tab-pane" data-tab-pane="global"></div>
  `;
});

const btn = (i) => document.querySelectorAll('.tab-btn')[i];
const pane = (name) => document.querySelector(`[data-tab-pane="${name}"]`);

describe('wireTabs', () => {
  it('Layer 2 tab flips the edit layer to lower and keeps the layer pane', () => {
    wireTabs();
    btn(1).click(); // LAYER 2

    expect(model.currentLayer).toBe('lower');
    expect(pane('layer').classList.contains('active')).toBe(true);
    expect(pane('global').classList.contains('active')).toBe(false);
    // The controller is told, so preset/echo context tracks the same layer.
    expect(sends).toContainEqual(['edit', 'lower']);
    // Active-tab styling follows the click.
    expect(btn(1).classList.contains('active')).toBe(true);
    expect(btn(0).classList.contains('active')).toBe(false);
  });

  it('the Layer 2 enable panel is gated to Layer 2 via the pane edit-layer attr', () => {
    wireTabs();
    const layerPane = pane('layer');
    // Starts on Upper — CSS hides the Layer 2 enable panel.
    expect(layerPane.dataset.editLayer).toBe('upper');
    btn(1).click(); // LAYER 2
    expect(layerPane.dataset.editLayer).toBe('lower');
    btn(0).click(); // back to LAYER 1
    expect(layerPane.dataset.editLayer).toBe('upper');
  });

  it('a per-layer param now resolves to the Lower id (upper + patchCount)', () => {
    wireTabs();
    btn(1).click();
    // assign_mode is a per-patch param (id < patchCount) in the fixture.
    const upper = paramIdByNameAtLayer('assign_mode', 'upper');
    const lower = paramIdByNameAtLayer('assign_mode', model.currentLayer);
    expect(lower).toBe(upper + PATCH_COUNT);
  });

  it('FX/Global tab swaps to the global pane without changing the edit layer', () => {
    wireTabs();
    btn(1).click(); // to lower first
    sends.length = 0;
    btn(2).click(); // FX / GLOBAL

    expect(pane('global').classList.contains('active')).toBe(true);
    expect(pane('layer').classList.contains('active')).toBe(false);
    expect(model.currentLayer).toBe('lower'); // unchanged
    expect(sends).not.toContainEqual(['edit', 'upper']);
  });

  it('re-selecting the same layer does not re-post an edit-layer op', () => {
    wireTabs();
    btn(0).click(); // LAYER 1 — already upper
    expect(sends).not.toContainEqual(['edit', 'upper']);
  });
});

describe('wireLayer2Toggle', () => {
  it('starts off, and a click enables (Dual) then disables (Single)', () => {
    wireLayer2Toggle();
    const el = document.getElementById('layer2-enable');
    expect(el.classList.contains('on')).toBe(false);

    el.click();
    expect(el.classList.contains('on')).toBe(true);
    expect(sends).toContainEqual(['keymode', 1]); // Dual

    el.click();
    expect(el.classList.contains('on')).toBe(false);
    expect(sends).toContainEqual(['keymode', 0]); // Single
  });

  it('an echo (model.setLayer2On) reflects state without re-posting', () => {
    wireLayer2Toggle();
    const el = document.getElementById('layer2-enable');
    model.setLayer2On(true);
    expect(el.classList.contains('on')).toBe(true);
    // No op was posted by the echo path.
    expect(sends).toHaveLength(0);
  });

  it('the switch sits in the Layer 2 tab: enabling it also selects that tab', () => {
    // Both wired: the switch click toggles enable AND bubbles to the tab button.
    wireTabs();
    wireLayer2Toggle();
    model.setLayer2On(false); // reset the module-level toggle state for isolation
    document.getElementById('layer2-enable').click();
    expect(model.currentLayer).toBe('lower');
    expect(pane('layer').classList.contains('active')).toBe(true);
    expect(sends).toContainEqual(['keymode', 1]);
  });
});
