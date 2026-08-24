// Legato dim rule. Legato decides whether a Solo reveal slides or
// re-articulates, so under Poly — where every note takes its own stack and
// nothing is ever revealed — nothing reads it and the toggle greys out.
//
// This used to live inside the Detune+Legato composite, which existed only to
// hold it; Legato is a plain `switch` cell now and the rule is a `BUILTIN_DIM_
// SPECS` entry like every other gate. These cases pin the behaviour across that
// move, including the layer offset the composite resolved by hand.
import { describe, it, expect, beforeEach } from 'vitest';
import * as dispatch from '../dispatch.js';

const { rebuildDimRules, applyDimRulesFor, model, _resetParamIndex } = dispatch;

const PATCH_COUNT = 100;
const VOICE_MODE = 20;
const LEGATO = 21;

const MODES = ['Poly', 'Solo'];

function seedParams() {
  globalThis.window = globalThis;
  const mode = () => ({ name: 'voice_mode', variants: MODES, min: 0, max: 1, default: 0 });
  const legato = () => ({ name: 'legato', variants: [], min: 0, max: 1, default: 0 });
  window.vxn = {
    patchCount: PATCH_COUNT,
    params: {
      [VOICE_MODE]: mode(),
      [LEGATO]: legato(),
      [VOICE_MODE + PATCH_COUNT]: mode(),
      [LEGATO + PATCH_COUNT]: legato(),
    },
  };
  _resetParamIndex();
}

function legatoCell() {
  return document.querySelector('[data-param="legato"]');
}

beforeEach(() => {
  document.body.innerHTML = `
    <div class="panel" data-name="Voice" data-layered>
      <div class="ctl-col voice-stack">
        <div class="ctl-strip voice-mode" data-control="rocker" data-param="voice_mode" data-orient="h"></div>
        <div class="ctl-strip voice-legato" data-control="switch" data-param="legato"></div>
      </div>
    </div>`;
  seedParams();
  model.dimRuleSpecs.length = 0; // builtins only — no markup-driven specs here
  model.lastParam.clear();
});

describe('legato dim rule', () => {
  it('dims under Poly and clears under Solo', () => {
    rebuildDimRules('upper');
    applyDimRulesFor(VOICE_MODE, 0); // Poly
    expect(legatoCell().classList.contains('dimmed')).toBe(true);
    applyDimRulesFor(VOICE_MODE, 1); // Solo
    expect(legatoCell().classList.contains('dimmed')).toBe(false);
  });

  it('finds Solo by name, not by a hard-coded index', () => {
    // Reorder the variants: the rule must follow the label, so a table edit
    // cannot silently invert it.
    window.vxn.params[VOICE_MODE].variants = ['Solo', 'Poly'];
    _resetParamIndex();
    rebuildDimRules('upper');
    applyDimRulesFor(VOICE_MODE, 0); // now Solo
    expect(legatoCell().classList.contains('dimmed')).toBe(false);
    applyDimRulesFor(VOICE_MODE, 1); // now Poly
    expect(legatoCell().classList.contains('dimmed')).toBe(true);
  });

  it('watches the edit layer\'s mode id', () => {
    rebuildDimRules('lower');
    const rule = model.dimRules.find((r) => r.target.dataset.param === 'legato');
    expect(rule).toBeTruthy();
    expect(rule.watchId).toBe(VOICE_MODE + PATCH_COUNT);
    // Layer 1's mode must no longer move it.
    applyDimRulesFor(VOICE_MODE, 0);
    expect(legatoCell().classList.contains('dimmed')).toBe(false);
    applyDimRulesFor(VOICE_MODE + PATCH_COUNT, 0);
    expect(legatoCell().classList.contains('dimmed')).toBe(true);
  });
});
