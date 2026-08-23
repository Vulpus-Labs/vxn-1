// Osc 1 Free dim rule (0283). Under Cross Mod = Sync, osc1 is the slave and
// osc2's wraps reset its phase whatever the flag says, so the switch is inert
// and greys out. Osc 2's Free is never dimmed — it is the one that bites under
// sync. Per-layer, like the cross-mod rule it sits beside.
import { describe, it, expect, beforeEach } from 'vitest';
import * as dispatch from '../dispatch.js';

const { rebuildDimRules, model, _resetParamIndex } = dispatch;

const PATCH_COUNT = 100;
const CROSS_MOD_TYPE = 7;
const OSC1_FREE_RUN = 11;
const OSC2_FREE_RUN = 12;

const VARIANTS = ['Off', 'Sync', 'FM', 'Ring'];

function seedParams() {
  globalThis.window = globalThis;
  const bool = (name) => ({ name, variants: [], min: 0, max: 1, default: 0 });
  const type = () => ({ name: 'cross_mod_type', variants: VARIANTS, min: 0, max: 3, default: 0 });
  window.vxn = {
    patchCount: PATCH_COUNT,
    params: {
      [CROSS_MOD_TYPE]: type(),
      [OSC1_FREE_RUN]: bool('osc1_free_run'),
      [OSC2_FREE_RUN]: bool('osc2_free_run'),
      [CROSS_MOD_TYPE + PATCH_COUNT]: type(),
      [OSC1_FREE_RUN + PATCH_COUNT]: bool('osc1_free_run'),
      [OSC2_FREE_RUN + PATCH_COUNT]: bool('osc2_free_run'),
    },
  };
  _resetParamIndex();
}

beforeEach(() => {
  document.body.innerHTML = `
    <div class="panel" data-name="Osc 1" data-layered>
      <div class="ctl-strip" data-control="switch" data-param="osc1_free_run"></div>
    </div>
    <div class="panel" data-name="Osc 2" data-layered>
      <div class="ctl-strip" data-control="switch" data-param="osc2_free_run"></div>
    </div>`;
  seedParams();
  model.dimRuleSpecs.length = 0; // builtins only — no markup-driven specs here
});

describe('osc free-run dim rule', () => {
  it('dims Osc 1 Free only under Sync', () => {
    rebuildDimRules('upper');
    const rule = model.dimRules.find(
      (r) => r.watchId === CROSS_MOD_TYPE && r.target.dataset.param === 'osc1_free_run',
    );
    expect(rule).toBeTruthy();
    expect(rule.predicate(0)).toBe(false); // Off
    expect(rule.predicate(1)).toBe(true); // Sync — osc1 is the slave
    expect(rule.predicate(2)).toBe(false); // FM
    expect(rule.predicate(3)).toBe(false); // Ring
  });

  it('never dims Osc 2 Free', () => {
    rebuildDimRules('upper');
    const osc2 = model.dimRules.find((r) => r.target.dataset.param === 'osc2_free_run');
    expect(osc2).toBeUndefined();
  });

  it('follows the edit layer', () => {
    rebuildDimRules('lower');
    const rule = model.dimRules.find(
      (r) => r.watchId === CROSS_MOD_TYPE + PATCH_COUNT
        && r.target.dataset.param === 'osc1_free_run',
    );
    expect(rule).toBeTruthy();
  });
});
