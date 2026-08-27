import { describe, it, expect, beforeEach } from 'vitest';
import {
  collectDimRuleSpecs,
  rebuildDimRules,
  applyDimRulesFor,
  refreshAllDimRules,
  model,
  BUILTIN_DIM_SPECS,
} from '../dispatch.js';
import { installFixture, PATCH_COUNT } from '../fixtures/params.js';

// Helper: mount a fixture DOM with dim-rule attribute targets and the
// builtin spec target divs (`[data-param="..."]`). The dim-rule
// resolution depends on both attribute-driven specs and `BUILTIN_DIM_SPECS`
// lookups against `[data-param]`.
function mountDimDOM() {
  document.body.innerHTML = `
    <div id="src-off-target" data-dim-when-src-off="assign_mode"></div>
    <div id="unless-fm-target" data-dim-unless-fm="xmod_type"></div>
    <div data-param="lfo1_delay_time"></div>
    <div data-param="lfo1_fade"></div>
    <div data-param="filter_slope"></div>
  `;
}

beforeEach(() => {
  installFixture();
  mountDimDOM();
  // Reset the dispatch model so successive tests don't see prior state.
  model.controls.clear();
  model.lastParam.clear();
  model.dimRules.length = 0;
  model.dimRuleSpecs.length = 0;
});

describe('collectDimRuleSpecs', () => {
  it('picks up data-dim-when-src-off attributes', () => {
    collectDimRuleSpecs();
    const srcOff = model.dimRuleSpecs.filter((s) => s.kind === 'src-off');
    expect(srcOff.length).toBe(1);
    expect(srcOff[0].watchName).toBe('assign_mode');
    expect(srcOff[0].target.id).toBe('src-off-target');
  });

  it('picks up data-dim-unless-fm attributes', () => {
    collectDimRuleSpecs();
    const unlessFm = model.dimRuleSpecs.filter((s) => s.kind === 'unless-fm');
    expect(unlessFm.length).toBe(1);
    expect(unlessFm[0].watchName).toBe('xmod_type');
    expect(unlessFm[0].target.id).toBe('unless-fm-target');
  });

  it('clears prior specs before re-collecting', () => {
    // Seed a stale spec; collect must wipe it.
    model.dimRuleSpecs.push({ kind: 'src-off', watchName: 'stale', target: null });
    collectDimRuleSpecs();
    expect(model.dimRuleSpecs.some((s) => s.watchName === 'stale')).toBe(false);
  });
});

describe('rebuildDimRules', () => {
  it('resolves watch names to upper-layer ids', () => {
    collectDimRuleSpecs();
    rebuildDimRules('upper');
    // src-off rule watches assign_mode (upper id 0).
    const r = model.dimRules.find((rl) => rl.target.id === 'src-off-target');
    expect(r.watchId).toBe(0);
    // unless-fm rule watches xmod_type (upper id 4).
    const fm = model.dimRules.find((rl) => rl.target.id === 'unless-fm-target');
    expect(fm.watchId).toBe(4);
  });

  it('translates per-patch watch ids on the lower layer', () => {
    collectDimRuleSpecs();
    rebuildDimRules('lower');
    const r = model.dimRules.find((rl) => rl.target.id === 'src-off-target');
    expect(r.watchId).toBe(0 + PATCH_COUNT);
    const fm = model.dimRules.find((rl) => rl.target.id === 'unless-fm-target');
    expect(fm.watchId).toBe(4 + PATCH_COUNT);
  });

  it('emits rules for every BUILTIN_DIM_SPECS target with a matching [data-param]', () => {
    // free-run has 2 targets (lfo1_delay_time, lfo1_fade); both mounted.
    // filter-notch has 1 target (filter_slope); mounted.
    // Plus our 2 attribute-driven rules → 5 total.
    expect(BUILTIN_DIM_SPECS.length).toBe(2);
    collectDimRuleSpecs();
    rebuildDimRules('upper');
    expect(model.dimRules.length).toBe(5);
  });

  it('src-off predicate fires when watch plain rounds to 0', () => {
    collectDimRuleSpecs();
    rebuildDimRules('upper');
    const r = model.dimRules.find((rl) => rl.target.id === 'src-off-target');
    expect(r.predicate(0)).toBe(true);
    expect(r.predicate(0.4)).toBe(true);
    expect(r.predicate(0.6)).toBe(false);
    expect(r.predicate(2)).toBe(false);
  });

  it('unless-fm predicate fires unless watch rounds to the FM variant index', () => {
    collectDimRuleSpecs();
    rebuildDimRules('upper');
    const r = model.dimRules.find((rl) => rl.target.id === 'unless-fm-target');
    // xmod_type variants = ['Off','Sync','FM'] → fmIdx=2.
    expect(r.predicate(2)).toBe(false);   // exact FM → no dim
    expect(r.predicate(1.6)).toBe(false); // rounds to 2 → no dim
    expect(r.predicate(0)).toBe(true);    // Off → dim
    expect(r.predicate(1)).toBe(true);    // Sync → dim
  });

  it('free-run builtin predicate flips on lfo1_free_run >= 0.5', () => {
    collectDimRuleSpecs();
    rebuildDimRules('upper');
    const target = document.querySelector('[data-param="lfo1_delay_time"]');
    const r = model.dimRules.find((rl) => rl.target === target);
    expect(r.predicate(0)).toBe(false);
    expect(r.predicate(0.49)).toBe(false);
    expect(r.predicate(0.5)).toBe(true);
    expect(r.predicate(1)).toBe(true);
  });

  it('filter-notch builtin predicate fires only on the Notch variant', () => {
    collectDimRuleSpecs();
    rebuildDimRules('upper');
    const target = document.querySelector('[data-param="filter_slope"]');
    const r = model.dimRules.find((rl) => rl.target === target);
    // FILTER_VARIANTS = ['Lowpass','Highpass','Bandpass','Notch'] → Notch=3.
    expect(r.predicate(0)).toBe(false);
    expect(r.predicate(3)).toBe(true);
    expect(r.predicate(2.6)).toBe(true); // rounds to 3
  });
});

describe('applyDimRulesFor', () => {
  it('adds .dimmed only on matching watch-id rules whose predicate fires', () => {
    collectDimRuleSpecs();
    rebuildDimRules('upper');
    // Watching assign_mode (id 0): plain=0 dims src-off-target.
    applyDimRulesFor(0, 0);
    expect(document.getElementById('src-off-target').classList.contains('dimmed')).toBe(true);
    // unless-fm-target shouldn't change — watch id differs.
    expect(document.getElementById('unless-fm-target').classList.contains('dimmed')).toBe(false);
  });

  it('removes .dimmed when the predicate flips back to false', () => {
    collectDimRuleSpecs();
    rebuildDimRules('upper');
    const el = document.getElementById('src-off-target');
    applyDimRulesFor(0, 0);
    expect(el.classList.contains('dimmed')).toBe(true);
    applyDimRulesFor(0, 1);
    expect(el.classList.contains('dimmed')).toBe(false);
  });

  it('ignores echoes for ids with no matching dim rule', () => {
    collectDimRuleSpecs();
    rebuildDimRules('upper');
    // id 99 has no rule. No-op should not throw, no class flips.
    applyDimRulesFor(99, 0);
    expect(document.querySelectorAll('.dimmed').length).toBe(0);
  });
});

describe('layer flip + refreshAllDimRules', () => {
  it('re-resolves ids on layer flip and refresh reseeds from lastParam', () => {
    collectDimRuleSpecs();
    rebuildDimRules('upper');
    // Cache an upper-layer value: free_run on (= 1) for id 1.
    model.lastParam.set(1, { plain: 1, norm: 1, display: 'On' });
    refreshAllDimRules();
    expect(document.querySelector('[data-param="lfo1_delay_time"]').classList.contains('dimmed')).toBe(true);

    // Flip to lower — ids re-resolve to +PATCH_COUNT, but the cached
    // upper-id value is no longer relevant. Cache the lower-id value
    // explicitly and refresh.
    rebuildDimRules('lower');
    document.querySelectorAll('.dimmed').forEach((el) => el.classList.remove('dimmed'));
    model.lastParam.set(1 + PATCH_COUNT, { plain: 1, norm: 1, display: 'On' });
    refreshAllDimRules();
    expect(document.querySelector('[data-param="lfo1_delay_time"]').classList.contains('dimmed')).toBe(true);
  });
});
