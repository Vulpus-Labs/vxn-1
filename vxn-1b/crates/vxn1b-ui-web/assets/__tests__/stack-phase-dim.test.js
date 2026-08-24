// Stack Phase dim rule (0284). The stack's start-phase depth is only consumed
// by an oscillator that is *reset* at note-on, so it goes inert exactly when
// both oscillators free-run — one free and one locked still leaves the knob
// biting on the locked one. That makes it an AND over two watched params, the
// first dim rule to watch more than one, so these cases also cover the
// list-valued `watch` fan-out itself.
import { describe, it, expect, beforeEach } from 'vitest';
import * as dispatch from '../dispatch.js';

const { rebuildDimRules, refreshAllDimRules, applyDimRulesFor, model, _resetParamIndex } = dispatch;

const PATCH_COUNT = 100;
const OSC1_FREE_RUN = 11;
const OSC2_FREE_RUN = 12;
const STACK_PHASE = 40;

function seedParams() {
  globalThis.window = globalThis;
  const bool = (name) => ({ name, variants: [], min: 0, max: 1, default: 0 });
  const phase = () => ({ name: 'stack_phase', variants: [], min: 0, max: 1, default: 1 });
  window.vxn = {
    patchCount: PATCH_COUNT,
    params: {
      [OSC1_FREE_RUN]: bool('osc1_free_run'),
      [OSC2_FREE_RUN]: bool('osc2_free_run'),
      [STACK_PHASE]: phase(),
      [OSC1_FREE_RUN + PATCH_COUNT]: bool('osc1_free_run'),
      [OSC2_FREE_RUN + PATCH_COUNT]: bool('osc2_free_run'),
      [STACK_PHASE + PATCH_COUNT]: phase(),
    },
  };
  _resetParamIndex();
}

/** Drive a param echo the way `dispatch` does: cache, then apply. */
function echo(id, plain) {
  model.lastParam.set(id, { plain, norm: plain, display: String(plain) });
  applyDimRulesFor(id, plain);
}

function phaseCell() {
  return document.querySelector('[data-param="stack_phase"]');
}

beforeEach(() => {
  document.body.innerHTML = `
    <div class="panel" data-name="Voice" data-layered>
      <div class="ctl" data-control="fader" data-param="stack_phase"></div>
    </div>`;
  seedParams();
  model.dimRuleSpecs.length = 0; // builtins only — no markup-driven specs here
  model.lastParam.clear();
});

describe('stack phase dim rule', () => {
  it('registers one rule per watched free-run flag', () => {
    rebuildDimRules('upper');
    const rules = model.dimRules.filter((r) => r.target.dataset.param === 'stack_phase');
    expect(rules.map((r) => r.watchId).sort((a, b) => a - b)).toEqual([
      OSC1_FREE_RUN,
      OSC2_FREE_RUN,
    ]);
  });

  it('dims only when both oscillators free-run', () => {
    rebuildDimRules('upper');
    echo(OSC1_FREE_RUN, 1);
    expect(phaseCell().classList.contains('dimmed')).toBe(false);
    echo(OSC2_FREE_RUN, 1);
    expect(phaseCell().classList.contains('dimmed')).toBe(true);
    // Releasing either one brings it back — whichever one changed.
    echo(OSC1_FREE_RUN, 0);
    expect(phaseCell().classList.contains('dimmed')).toBe(false);
  });

  it('reads the cache rather than the echoed value', () => {
    // Osc 2 already on, then Osc 1's echo arrives: the predicate has to consult
    // Osc 2's cached value, not just the value it was handed.
    rebuildDimRules('upper');
    model.lastParam.set(OSC2_FREE_RUN, { plain: 1, norm: 1, display: '1' });
    echo(OSC1_FREE_RUN, 1);
    expect(phaseCell().classList.contains('dimmed')).toBe(true);
  });

  it('treats a flag with no echo yet as off', () => {
    rebuildDimRules('upper');
    echo(OSC1_FREE_RUN, 1);
    expect(phaseCell().classList.contains('dimmed')).toBe(false);
  });

  it('follows the edit layer', () => {
    rebuildDimRules('lower');
    const rules = model.dimRules.filter((r) => r.target.dataset.param === 'stack_phase');
    expect(rules.map((r) => r.watchId).sort((a, b) => a - b)).toEqual([
      OSC1_FREE_RUN + PATCH_COUNT,
      OSC2_FREE_RUN + PATCH_COUNT,
    ]);
    model.lastParam.set(OSC1_FREE_RUN + PATCH_COUNT, { plain: 1, norm: 1, display: '1' });
    model.lastParam.set(OSC2_FREE_RUN + PATCH_COUNT, { plain: 1, norm: 1, display: '1' });
    refreshAllDimRules();
    expect(phaseCell().classList.contains('dimmed')).toBe(true);
  });
});
