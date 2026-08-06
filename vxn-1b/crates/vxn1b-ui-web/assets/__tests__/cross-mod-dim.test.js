// Cross Mod panel dim rule (0242). The Amount fader drives PM only, so it must
// grey out for Off / Sync / Ring and light up for FM — and the rule must follow
// the edit layer, since cross-mod is per-layer patch state.
import { describe, it, expect, beforeEach } from 'vitest';
import * as dispatch from '../dispatch.js';

const { collectDimRuleSpecs, rebuildDimRules, model, _resetParamIndex } = dispatch;

const PATCH_COUNT = 100;
const CROSS_MOD_TYPE = 7;
const CROSS_MOD_AMOUNT = 8;

// `CROSS_MOD_LABELS` order from the param table — "FM" is the PM variant.
const VARIANTS = ['Off', 'Sync', 'FM', 'Ring'];

function seedParams() {
  globalThis.window = globalThis;
  window.vxn = {
    patchCount: PATCH_COUNT,
    params: {
      [CROSS_MOD_TYPE]: { name: 'cross_mod_type', variants: VARIANTS, min: 0, max: 3, default: 0 },
      [CROSS_MOD_AMOUNT]: { name: 'cross_mod_amount', variants: [], min: 0, max: 4, default: 0 },
      [CROSS_MOD_TYPE + PATCH_COUNT]: {
        name: 'cross_mod_type', variants: VARIANTS, min: 0, max: 3, default: 0,
      },
      [CROSS_MOD_AMOUNT + PATCH_COUNT]: {
        name: 'cross_mod_amount', variants: [], min: 0, max: 4, default: 0,
      },
    },
  };
  _resetParamIndex();
}

beforeEach(() => {
  document.body.innerHTML = `
    <div class="panel" data-name="Cross Mod" data-layered>
      <div class="ctl" data-control="buttongroup" data-param="cross_mod_type"></div>
      <div class="ctl" data-control="fader" data-param="cross_mod_amount"
           data-dim-unless-fm="cross_mod_type"></div>
    </div>`;
  seedParams();
  collectDimRuleSpecs();
});

describe('cross-mod amount dim rule', () => {
  it('watches the type selector on the active layer', () => {
    rebuildDimRules('upper');
    const rule = model.dimRules.find((r) => r.watchId === CROSS_MOD_TYPE);
    expect(rule).toBeTruthy();
    expect(rule.target.dataset.param).toBe('cross_mod_amount');

    rebuildDimRules('lower');
    const lower = model.dimRules.find((r) => r.watchId === CROSS_MOD_TYPE + PATCH_COUNT);
    expect(lower).toBeTruthy();
  });

  it('dims for every mode that ignores the amount', () => {
    rebuildDimRules('upper');
    const { predicate } = model.dimRules.find((r) => r.watchId === CROSS_MOD_TYPE);
    expect(predicate(0)).toBe(true); // Off
    expect(predicate(1)).toBe(true); // Sync
    expect(predicate(2)).toBe(false); // FM — the one mode the fader drives
    expect(predicate(3)).toBe(true); // Ring
  });
});
