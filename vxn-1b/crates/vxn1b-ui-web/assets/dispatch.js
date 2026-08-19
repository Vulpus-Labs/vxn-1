// ─── Init + dispatch ───────────────────────────────────────────────────────
//
// `import` lines are dropped by the splice loader (`strip_esm_exports`), so in
// production every binding below rides the single concatenated scope — the
// same arrangement `panels/keys.js` documents. Declaring the imports anyway
// lets the vitest suites drive functions that touch these helpers under Node
// ESM, where the concatenated scope doesn't exist. Only the helpers the suites
// actually reach are declared; the rest still resolve implicitly.
// Deliberately NOT declared here: the `make*` primitive factories. The
// orchestration suite stubs them on `globalThis` to observe binding, and a
// module-level binding would shadow those stubs. They stay implicit, resolved
// from the concatenated scope in production and from the stubs under test.
import { tgRow } from './util/drag.js';
import { noteName } from '../../../../crates/vxn-core-ui-web/assets/cutoff-tuned.js';

// Cached name → lowest-id index, built once in `init()` against
// `window.vxn.params`. Null before init or in pure-helper test paths that
// never call init — the first `paramIdByName` lookup builds it lazily.
// `window.vxn.params` is set once at editor open and never reassigned; if
// that ever changes, set `_paramIdByName = null` at the reassign site.
let _paramIdByName = null;
// Test-visible counter — increments every time the cache is built.
// Production code ignores this; the param-id-by-name suite asserts the
// build happens exactly once per init.
export let _paramIndexBuilds = 0;

export function buildParamIndex() {
  _paramIndexBuilds += 1;
  const ix = new Map();
  const params = (window.vxn && window.vxn.params) || {};
  for (const k in params) {
    const name = params[k].name;
    if (name != null && !ix.has(name)) ix.set(name, parseInt(k, 10));
  }
  return ix;
}

// Lowest-id name lookup — for a per-patch param this is the Upper-layer id
// (id < patchCount); for a global it's the global id directly. Layer
// rebinding (0045) translates Upper → Lower with `+patchCount`.
export function paramIdByName(name) {
  if (_paramIdByName == null) _paramIdByName = buildParamIndex();
  const id = _paramIdByName.get(name);
  return id == null ? null : id;
}

// Test-only cache reset. Production code never reassigns `window.vxn.params`,
// but the unit suite swaps the fixture between tests.
export function _resetParamIndex() {
  _paramIdByName = null;
  _paramIndexBuilds = 0;
}

// Test-only reset of the view's KeyState mirrors (`_layer2On` / `_splitEnabled`
// / `_lfo2Link`). These are module-level in production because the editor is a
// single long-lived page; the suites mount the faceplate repeatedly, so without
// this one test's toggle state leaks into the next.
export function _resetKeyStateView() {
  _layer2On = false;
  _splitEnabled = false;
  _lfo2Link = false;
}

// Per-layer name → id lookup. Globals (id ≥ 2·patchCount) are layer-
// independent and pass through unchanged. Per-patch ids translate from
// Upper to Lower by adding `patchCount` (the slot offset that
// `vxn_app::patch_clap_id` bakes in: lower_id = upper_id + PATCH_COUNT).
export function paramIdByNameAtLayer(name, layer) {
  const upper = paramIdByName(name);
  if (upper == null) return null;
  const pc = window.vxn.patchCount;
  if (upper >= 2 * pc) return upper;
  return layer === 'lower' ? upper + pc : upper;
}

// Look up a variant's plain index on an enum param at the current
// layer. Returns -1 if either the param or the variant name is
// unknown — callers treat that as "rule does not apply".
export function variantIdx(paramName, variantName, layer) {
  const id = paramIdByNameAtLayer(paramName, layer);
  if (id == null) return -1;
  const variants = window.vxn.params[id].variants || [];
  return variants.indexOf(variantName);
}

export function isLayeredEl(el) {
  return el.closest('[data-layered]') != null;
}

// Per-tick mutable state the dispatcher owns. Grouped here so the module
// reads as "init builds the model; dispatch reads + mutates it" rather
// than a dozen free-floating globals.
export const model = {
  // ParamChanged routing: id → [updater closures]. Composite cells
  // (detune-legato) register secondary watchers on related ids; dispatch
  // fans each echo out to every updater on the id.
  controls: new Map(),
  // Last (plain, norm, display) seen per id. Sync-partner refresh /
  // dim refresh / layer rebind reseed from here.
  lastParam: new Map(),
  // sync_partner pairings: rateId ↔ syncId for LFO1 / LFO2 / Delay,
  // resolved per layer in rebindAllForLayer.
  syncOfRate: new Map(),
  rateOfSync: new Map(),
  // Cutoff ↔ Tuned pairing (per layer). When Tuned flips the cutoff
  // fader must re-paint with a different norm + display.
  tunedOfCutoff: new Map(),
  cutoffOfTuned: new Map(),
  // Active edit layer ('upper' | 'lower'). EditLayerChanged mutates.
  currentLayer: 'upper',
  // Dim-rule specs collected from HTML attributes + builtins (0066).
  dimRuleSpecs: [],
  // Resolved rules for the current layer: { watchId, predicate, target }.
  dimRules: [],
  // Per-cell binding info captured at init; layered entries rebuild
  // against new layer ids on EditLayerChanged, static entries don't.
  cells: [],
};

export function addCtl(id, ctl) {
  let arr = model.controls.get(id);
  if (!arr) model.controls.set(id, arr = []);
  arr.push(ctl);
}

// Pair sync-able rate/time faders with their sync-toggle partners (E004 /
// 0015). Mirrors `vxn_ui_vizia::sync_partner`: LFO 1 rate ↔ LFO 1 sync
// (per-patch), LFO 2 rate ↔ LFO 2 sync (global), Delay Time ↔ Delay Sync
// (global, 0045). Resolved per current layer.
export function locateSyncPartners(layer) {
  model.syncOfRate.clear();
  model.rateOfSync.clear();
  // VXN1b: LFO 1 / LFO 2 rate↔sync pairs. Delay sync stays dropped from the
  // compact faceplate (0209) — no such param in vxn1b-engine. (Cutoff "Tuned"
  // came back in 0250 and is paired below, not here — it is not a rate/sync
  // pair.) The `paramIdByNameAtLayer` guard below tolerates any name that
  // resolves to null, so a stale entry would no-op, but keep the list honest.
  const pairs = [
    ['lfo1_rate', 'lfo1_sync'],
    ['lfo2_rate', 'lfo2_sync'],
  ];
  for (const [rateName, syncName] of pairs) {
    const r = paramIdByNameAtLayer(rateName, layer);
    const s = paramIdByNameAtLayer(syncName, layer);
    if (r == null || s == null) continue;
    model.syncOfRate.set(r, s);
    model.rateOfSync.set(s, r);
  }
  // Cutoff ↔ Tuned (0250, ported from VXN1). Paired per layer: the toggle
  // switches that layer's Cutoff fader between exp-Hz and note-quantised
  // (MIDI C0..C4) mapping, so the two ids have to find each other on every
  // rebind. Engine-side the toggle is inert — it only changes how the fader
  // reads and writes.
  model.tunedOfCutoff.clear();
  model.cutoffOfTuned.clear();
  const cutoffId = paramIdByNameAtLayer('cutoff', layer);
  const tunedId = paramIdByNameAtLayer('cutoff_tuned', layer);
  if (cutoffId != null && tunedId != null) {
    model.tunedOfCutoff.set(cutoffId, tunedId);
    model.cutoffOfTuned.set(tunedId, cutoffId);
  }
}

// ─── Generic dim rules (0044) ──────────────────────────────────────────────
//
// Per-cell HTML markers register a dim rule resolved at bind time:
//   `data-dim-when-src-off="srcName"` — dim self when the named source
//     selector reads `Off` (the depth fader paired with a source
//     buttongroup: Pitch/PWM Mod, Cross Mod's osc2 Mod). Source
//     selectors themselves stay bright; only their paired fader dims so
//     a routed-Off path is still readable + clickable.
//   `data-dim-unless-fm="typeName"` — dim self unless the named type
//     selector reads the `FM` variant. Cross Mod's Amount fader only
//     drives PM (labelled FM, ADR 0004 §3), so it greys out for Off and
//     Sync, matching `vxn_ui_vizia::xmod_pair`.
//
// One pass collects the HTML-attribute specs from the DOM into
// `model.dimRuleSpecs`; resolution to current-layer CLAP ids happens on
// every (re)bind into `model.dimRules` so a layer flip rebuilds them
// without touching the markup.

// Built-in dim specs that don't fit the HTML-attribute model (targets are
// named params resolved at bind time, not DOM elements picked up by a
// querySelectorAll). Each entry fans out into N `DIM_RULES` entries that
// share one `watchId` and `predicate`.
//   - `free-run`: LFO 1's delay/fade dim when Free toggles on (0042).
//   - `filter-notch`: Slope strip dims when Filter Mode = Notch (0043).
export const BUILTIN_DIM_SPECS = [
  {
    kind: 'free-run',
    watch: 'lfo1_free_run',
    buildPredicate: () => (plain) => plain >= 0.5,
    targets: ['lfo1_delay_time', 'lfo1_fade'],
  },
  {
    kind: 'filter-notch',
    watch: 'filter_mode',
    buildPredicate: (layer) => {
      const notchIdx = variantIdx('filter_mode', 'Notch', layer);
      return (plain) => notchIdx >= 0 && Math.round(plain) === notchIdx;
    },
    targets: ['filter_slope'],
  },
];

export function collectDimRuleSpecs() {
  model.dimRuleSpecs.length = 0;
  document.querySelectorAll('[data-dim-when-src-off]').forEach((el) => {
    model.dimRuleSpecs.push({
      kind: 'src-off',
      watchName: el.dataset.dimWhenSrcOff,
      target: el,
    });
  });
  document.querySelectorAll('[data-dim-unless-fm]').forEach((el) => {
    model.dimRuleSpecs.push({
      kind: 'unless-fm',
      watchName: el.dataset.dimUnlessFm,
      target: el,
    });
  });
}

export function rebuildDimRules(layer) {
  model.dimRules.length = 0;
  for (const spec of model.dimRuleSpecs) {
    const watchId = paramIdByNameAtLayer(spec.watchName, layer);
    if (watchId == null) continue;
    let predicate;
    if (spec.kind === 'src-off') {
      predicate = (plain) => Math.round(plain) === 0;
    } else if (spec.kind === 'unless-fm') {
      const fmIdx = variantIdx(spec.watchName, 'FM', layer);
      predicate = (plain) => fmIdx < 0 || Math.round(plain) !== fmIdx;
    } else {
      continue;
    }
    model.dimRules.push({ watchId, predicate, target: spec.target });
  }
  for (const spec of BUILTIN_DIM_SPECS) {
    const watchId = paramIdByNameAtLayer(spec.watch, layer);
    if (watchId == null) continue;
    const predicate = spec.buildPredicate(layer);
    for (const name of spec.targets) {
      const target = document.querySelector(`[data-param="${name}"]`);
      if (target) model.dimRules.push({ watchId, predicate, target });
    }
  }
}

export function applyDimRulesFor(id, plain) {
  for (const r of model.dimRules) {
    if (r.watchId !== id) continue;
    r.target.classList.toggle('dimmed', r.predicate(plain));
  }
}

// Re-apply every dim rule from cached last-known values. Called after a
// layer rebind so the new layer's bindings reflect the correct dim state
// before any fresh ParamChanged echoes arrive.
export function refreshAllDimRules() {
  for (const r of model.dimRules) {
    const last = model.lastParam.get(r.watchId);
    if (!last) continue;
    r.target.classList.toggle('dimmed', r.predicate(last.plain));
  }
}

// Returns the `displayOverride` callback for `id` if it's a rate fader
// whose sync partner is currently on. The fader's `update` runs this
// before settling on a popup label.
export function rateDisplayOverride(id) {
  const syncId = model.syncOfRate.get(id);
  if (syncId == null) return null;
  return (plain, norm, display) => {
    const last = model.lastParam.get(syncId);
    if (last && last.plain >= 0.5) return subdivisionLabel(norm);
    return null;
  };
}

// Cutoff fader overrides (per Filter Cutoff "Tuned" toggle). When tuned:
//   - drag maps norm linearly to MIDI 12..60, snaps to int, sends Hz
//   - thumb position derives from the (snapped) Hz of the current value
//   - display reads as a note name (e.g. "C2") instead of Hz
// Tuned off: return null, default fader behaviour kicks in.
function cutoffTunedOn(id) {
  const tunedId = model.tunedOfCutoff.get(id);
  if (tunedId == null) return false;
  const last = model.lastParam.get(tunedId);
  return !!(last && last.plain >= 0.5);
}
export function cutoffInteractionOverride(id) {
  if (model.tunedOfCutoff.get(id) == null) return null;
  return (rawNorm) => {
    if (!cutoffTunedOn(id)) return null;
    const hz = cutoffTunedNormToHz(rawNorm);
    return { plain: hz, norm: cutoffTunedHzToNorm(hz) };
  };
}
export function cutoffNormOverride(id) {
  if (model.tunedOfCutoff.get(id) == null) return null;
  return (plain) => (cutoffTunedOn(id) ? cutoffTunedHzToNorm(plain) : null);
}
export function cutoffDisplayOverride(id) {
  if (model.tunedOfCutoff.get(id) == null) return null;
  return (plain, norm, display) =>
    cutoffTunedOn(id) ? cutoffTunedNoteName(plain) : null;
}

// Layer pan readout (0248): a bipolar `[-1, 1]` position reads as a mixer
// does — `L50` / `C` / `R50` — rather than as a signed fraction. The engine
// keeps the raw number (it is what the host automates), so this is purely the
// faceplate's label.
export function panLabel(plain) {
  const pct = Math.round(Math.abs(plain) * 100);
  if (pct === 0) return 'C';
  return (plain < 0 ? 'L' : 'R') + pct;
}
export function panDisplayOverride(name) {
  return name === 'layer_pan' ? (plain) => panLabel(plain) : null;
}

export function bindCell(entry, layer) {
  const { el, kind, name } = entry;
  // `data-fixed-layer` pins a cell to one layer regardless of the edit-layer
  // tab (0220). The mixer needs it: both layer strips must be visible and
  // adjustable at once, which is the opposite of the `data-layered` rebind.
  // Such cells are static — collected once, never reset on a tab flip.
  const id = paramIdByNameAtLayer(name, entry.fixedLayer || layer);
  if (id == null) return null;
  const desc = window.vxn.params[id];
  let ctl = null;
  switch (kind) {
    case 'fader': {
      // Cutoff fader: tuned mode replaces both the drag mapping and the
      // popup display. Other faders share the existing rate/sync display
      // override path.
      const opts = {
        displayOverride: cutoffDisplayOverride(id) || rateDisplayOverride(id),
        normOverride: cutoffNormOverride(id),
        interactionOverride: cutoffInteractionOverride(id),
      };
      ctl = makeFader(el, id, desc, opts);
      break;
    }
    case 'wave':          ctl = makeWave(el, id, desc); break;
    case 'dial':          ctl = makeDial(el, id, desc, { displayOverride: panDisplayOverride(name) }); break;
    case 'bipolar':       ctl = makeBipolar(el, id, desc); break;
    case 'switch':        ctl = makeSwitch(el, id, desc); break;
    case 'rocker':        ctl = makeRocker(el, id, desc); break;
    case 'buttongroup':   ctl = makeButtonGroup(el, id, desc); break;
    case 'dropdown':      ctl = makeDropdown(el, id, desc); break;
    case 'header-switch': ctl = makeHeaderSwitch(el, id, desc); break;
    case 'detune-legato': {
      const legatoId = paramIdByNameAtLayer(entry.extras.legatoName, layer);
      const modeId   = paramIdByNameAtLayer(entry.extras.modeName, layer);
      if (legatoId == null || modeId == null) return null;
      const composite = makeDetuneLegato(
        el,
        { detune: id, legato: legatoId, mode: modeId },
        {
          detune: desc,
          legato: window.vxn.params[legatoId],
          mode:   window.vxn.params[modeId],
        },
        entry.extras.modeName,
        layer,
      );
      // Fan composite updaters through model.controls by id. Mode is also bound
      // by the AssignMode buttongroup cell — `addCtl` keeps both updaters
      // alive on the same id so the buttongroup repaints and the detune-
      // legato visuals (top-override, Legato dim, Twin clamp) follow the
      // same echo.
      addCtl(id,       { update: (p, n, d) => composite.detuneUpdate(p, n, d) });
      addCtl(legatoId, { update: (p) => composite.legatoUpdate(p) });
      addCtl(modeId,   { update: (p) => composite.modeUpdate(p) });
      return { ids: [id, legatoId, modeId] };
    }
    default:
      console.warn('vxn: unknown control type', kind);
      return null;
  }
  addCtl(id, ctl);

  // Double-click resets the param to its descriptor default — mirrors
  // the vizia editor's `.on_double_click` (bracketed by a gesture so
  // the host records one edit). Wired on the cell root so it covers
  // every primitive uniformly; the intermediate single-click value
  // changes still fire and the reset lands last. (Header-switch is a
  // bool toggle — double-clicking it would just toggle twice; skip.)
  if (kind !== 'header-switch') {
    el.addEventListener('dblclick', (ev) => {
      ev.preventDefault();
      window.vxn.send.discrete(id, desc.default);
    });
  }
  return { ids: [id] };
}

// Swap a cell's element for a pristine copy of itself: same tag, same
// attributes (`data-control` / `data-param` / the dim markers), no children, no
// classes or inline styles the last primitive added — and, crucially, **no
// event listeners**.
//
// Clearing `innerHTML` is not enough. It disposes of listeners bound to the
// children a primitive built, which is most of them, but not of any bound to the
// cell root: the rocker's click, the detune composite's double-click, and
// `bindCell`'s reset-to-default double-click all attach there. Those survived
// the reset and accumulated one closure per layer flip, each still holding the
// id of the layer it was bound under — so after visiting Layer 2, a click on the
// Voice rocker wrote Poly/Solo to layer 1 *and* layer 2, and a double-click
// reset both layers' values.
//
// Replacing the node fixes every such case at once, including ones nobody has
// written yet: a new primitive can attach listeners wherever it likes and stay
// correct across a rebind by construction.
function freshenCell(entry) {
  const next = entry.el.cloneNode(false);
  // The clone carries whatever classes the last primitive left; put the
  // markup's own back. (`init` always records this; the guard is for entries
  // hand-built by tests.)
  if (entry.baseClass != null) next.className = entry.baseClass;
  next.removeAttribute('style');
  entry.el.replaceWith(next);
  entry.el = next;
}

export function rebindAllForLayer(layer) {
  // Drop every prior binding — closures held the old ids; the only safe
  // way to retarget is to start fresh. `model.controls` is the routing
  // table for ParamChanged dispatch, so emptying it before re-bind
  // avoids stale updates landing on the old (now-orphaned) primitives.
  model.controls.clear();
  // New nodes first, before anything captures an element reference. Static
  // (`data-fixed-layer`) cells are freshened too: they rebind on every flip like
  // any other cell, so their root-level listeners would pile up the same way —
  // harmlessly, since the id never changes, but a pile-up either way.
  for (const entry of model.cells) freshenCell(entry);
  // The HTML-attribute dim specs hold element references, which the sweep above
  // has just invalidated; re-collect before resolving them to ids.
  collectDimRuleSpecs();
  // Resolve per-layer quirk ids BEFORE bindCell so each fader's
  // `rateDisplayOverride` closure captures the *current* layer's
  // sync-partner id.
  locateSyncPartners(layer);
  rebuildDimRules(layer);
  for (const entry of model.cells) {
    bindCell(entry, layer);
  }
  // Non-cell control subscribers (Keys panel's per-layer Level sliders)
  // re-register here so the model.controls clear above doesn't strand
  // them — without this they'd miss every ParamChanged echo after the
  // first rebind. DOM event listeners are guarded inside `wireLayerLevels`
  // and only attach once per layer.
  keysPanel.wireLayerLevels();
  // The mod-matrix overlay's topology selectors track the edit layer too (the
  // depth dials are layered cells rebound above); reseed them from the new
  // layer's snapshot (0219).
  matrixOverlay.refreshForLayer(layer);
  // Reseed the visual dim state from cached last-known values so a layer
  // rebind reflects the new layer's state before any echo arrives.
  refreshAllDimRules();
  // Feed cached values into freshly-rebound controls (the new ids are
  // already in model.lastParam from the editor-ready broadcast).
  repaintAllControls();
}

// Re-apply every control's last-known value from cache.
//
// Needed because a fader's thumb is positioned in pixels and so needs real
// layout: painted while its container is `display: none` it cannot be placed,
// and `paintFader` deliberately skips it (the fill, being percentage-driven,
// stays correct — which is why the symptom is a thumb pinned to full scale over
// a correctly-lit track). Call this whenever a hidden container is revealed.
export function repaintAllControls() {
  for (const [id, ctls] of model.controls) {
    const last = model.lastParam.get(id);
    if (!last) continue;
    for (const c of ctls) c.update(last.plain, last.norm, last.display);
  }
}

// Three-tab shell (0219, ADR 0002 §8). Layer 1 / Layer 2 select the edit layer
// (the layer pane's cells rebind via `rebindAllForLayer`); FX / Global swaps to
// the global pane. The tab strip subsumes VXN1's separate edit-layer toggle, so
// tabbing to a layer both shows the layer pane and flips `model.currentLayer`.
export function wireTabs() {
  const strip = document.getElementById('tab-strip');
  if (!strip) return;
  const btns = Array.from(strip.querySelectorAll('.tab-btn'));
  const panes = Array.from(document.querySelectorAll('[data-tab-pane]'));
  const layerPane = document.querySelector('[data-tab-pane="layer"]');

  const showPane = (name) => {
    for (const p of panes) p.classList.toggle('active', p.dataset.tabPane === name);
  };
  const selectLayer = (code) => {
    if (model.currentLayer === code) return;
    model.currentLayer = code;
    // The pane's edit layer drives which layer-specific panels show (the Layer 2
    // enable lives on Layer 2's tab only — CSS gates it on this attribute).
    if (layerPane) layerPane.dataset.editLayer = code;
    rebindAllForLayer(code);
    // Keep the controller's edit-layer in step so preset/reset context and the
    // KeyModeChanged/EditLayerChanged echoes target the right layer.
    window.vxn.send.setEditLayer(code);
  };

  for (const btn of btns) {
    btn.addEventListener('click', () => {
      for (const b of btns) b.classList.toggle('active', b === btn);
      if (btn.dataset.tab === 'layer') {
        showPane('layer');
        // selectLayer repaints via rebindAllForLayer, but only when the layer
        // actually changed — re-showing the same layer still needs a repaint.
        selectLayer(btn.dataset.layer);
        repaintAllControls();
      } else {
        showPane('global');
        repaintAllControls();
      }
      // The scope follows the visible pane: the edit layer's tap on a layer
      // tab, nothing at all on FX/Global.
      syncScopeSource();
    });
  }
}

// Layer 2 on/off toggle (0219). Off (default) → Single (synth 2 bypassed); on →
// Dual. Split (mode 2) is the FX/Global tab's concern (0220). Posts the derived
// KeyMode via `setKeyMode`; the engine-side apply (KeyState → audio thread) lands
// with the topology wire, so this is currently the view's own state until an echo
// reconciles it. Exposed for `setLayer2On` so a KeyModeChanged echo can reflect.
let _layer2On = false;
export function wireLayer2Toggle() {
  const el = document.getElementById('layer2-enable');
  if (!el) return;
  const render = () => {
    el.classList.toggle('on', _layer2On);
    // Panels that only mean something with two layers (the Layer 2 mixer strip,
    // the whole Split panel) dim rather than disappear, so the layout doesn't
    // jump as the layer toggles (0220).
    document.querySelectorAll('[data-layer2-gated]').forEach((p) => {
      p.classList.toggle('dimmed', !_layer2On);
    });
  };
  render();
  el.addEventListener('click', () => {
    _layer2On = !_layer2On;
    render();
    // KeyMode is derived, so the posted mode must carry BOTH toggles: turning
    // Layer 2 on with split already armed goes straight to Split (2), not Dual.
    window.vxn.send.setKeyMode(_layer2On ? (_splitEnabled ? 2 : 1) : 0);
    // The switch sits inside the Layer 2 tab button, so this click also bubbles
    // to wireTabs — turning it on selects the Layer 2 tab (0219 §4).
  });
  // Reflect a controller echo (KeyModeChanged) without re-posting: mode ≥ 1 means
  // layer 2 is live.
  model.setLayer2On = (on) => {
    _layer2On = !!on;
    render();
  };
}

// Cross-layer LFO 2 link (0217, ADR 0002 §5). Layer 2's LFO 2 slaves to Layer
// 1's — rate + phase lock — so both layers' LFO2-driven routes move together.
// It is KeyState, not a CLAP param (and not `lfo2_sync`, which is per-layer
// tempo sync), so the cell is hand-built here rather than bound by
// `rebindAllForLayer`: same `.ctl-tg-row` markup as a `switch` strip cell, but
// posting `setLfo2Link`. Layer-1-only in the DOM sense — CSS hides it on the
// Layer 1 tab, since the flag describes the slave layer.
let _lfo2Link = false;
export function wireLfo2Link() {
  const el = document.getElementById('lfo2-link');
  if (!el) return;
  el.innerHTML = '';
  const row = tgRow('Link');
  el.appendChild(row);
  const render = () => row.classList.toggle('active', _lfo2Link);
  render();
  row.addEventListener('pointerdown', (ev) => {
    ev.preventDefault();
    _lfo2Link = !_lfo2Link;
    render();
    window.vxn.send.setLfo2Link(_lfo2Link);
  });
  // Reflect an engine-side echo (state / preset load) without re-posting.
  model.setLfo2Link = (on) => {
    _lfo2Link = !!on;
    render();
  };
}

// Confirmation modal — title, message, Cancel / confirm.
//
// Elements are looked up per call rather than captured at module load: this
// module is spliced into the page and imported headless by the suites, so
// binding at load time would either crash or capture a DOM that isn't there
// yet. Listeners are attached per call and torn down on close, so a dialogue
// opened twice can't fire its callback twice.
//
// Cancel is the resting choice: the backdrop and Esc both dismiss without
// running the action. With no dialogue markup on the page `ask` does nothing at
// all — an unconfirmable destructive action must not silently become an
// unconfirmed one.
export const confirmDialog = {
  ask(opts, onConfirm) {
    const backdrop = document.getElementById('confirm-backdrop');
    const okBtn = document.getElementById('confirm-ok');
    const cancelBtn = document.getElementById('confirm-cancel');
    const titleEl = document.getElementById('confirm-title');
    const messageEl = document.getElementById('confirm-message');
    if (!backdrop || !okBtn || !cancelBtn || !titleEl || !messageEl) return;

    titleEl.textContent = opts.title || '';
    messageEl.textContent = opts.message || '';
    okBtn.textContent = opts.okLabel || 'OK';

    const onKey = (ev) => {
      if (ev.key === 'Escape') close();
    };
    const onBackdrop = (ev) => {
      // Only a click on the backdrop itself — one landing inside the panel
      // bubbles here too, and dismissing on that would make the dialogue
      // impossible to read without closing it.
      if (ev.target === backdrop) close();
    };
    function close() {
      backdrop.hidden = true;
      okBtn.removeEventListener('click', confirm);
      cancelBtn.removeEventListener('click', close);
      backdrop.removeEventListener('click', onBackdrop);
      document.removeEventListener('keydown', onKey);
    }
    function confirm() {
      close();
      onConfirm();
    }

    okBtn.addEventListener('click', confirm);
    cancelBtn.addEventListener('click', close);
    backdrop.addEventListener('click', onBackdrop);
    document.addEventListener('keydown', onKey);
    backdrop.hidden = false;
  },
};

// Copy Layer 1 → Layer 2 (0265). Duplicates Layer 1's patch params and matrix
// topology onto Layer 2, leaves the mixer strip alone, and stamps a small
// detune on the copy so the pair beats rather than sums.
//
// A `PatchOp`, not a param and not KeyState, so — like the LFO 2 link cell —
// it is hand-wired here rather than bound by `rebindAllForLayer`.
//
// Destructive: it overwrites whatever Layer 2 held, and lands ~66 param changes
// in the host's undo stack as one burst — hence the confirmation. The direction
// is fixed rather than "copy the edit layer to the other one", so the button
// means the same thing on either tab and the label can say which way it goes.
export function wireCopyLayer() {
  const btn = document.getElementById('copy-layer');
  if (!btn) return;
  btn.addEventListener('click', () => {
    confirmDialog.ask(
      {
        title: 'Copy Layer 1 → Layer 2',
        message:
          "Layer 2's patch and mod-matrix routing will be replaced by Layer 1's. "
          + 'Its mixer strip — level, pan, mute — is left alone, and the copy is '
          + 'detuned slightly so the two layers beat rather than sum.',
        okLabel: 'Copy',
      },
      () => window.vxn.send.copyLayer('upper', 'lower'),
    );
  });
}

// Keyboard split (0220 / ADR 0002 §3). Enable + point are KeyState, not CLAP
// params, so they post `setKeyMode` / `setSplitPoint` custom opcodes rather
// than going through the param path.
//
// KeyMode is DERIVED, never stored: the wire carries 0/1/2 and the engine maps
// it back onto the layer-2 and split-enable toggles. So the split switch must
// send a mode that also respects whether Layer 2 is on — enabling a split while
// Layer 2 is off would otherwise silently turn Layer 2 on as a side effect.
// While Layer 2 is off the control is inert (the panel is dimmed by CSS) and
// the flag is simply remembered for when Layer 2 comes back.
export const SPLIT_MIN = 12;
export const SPLIT_MAX = 96;
export const SPLIT_DEFAULT = 60;

let _splitEnabled = false;
export function wireSplit() {
  const enableEl = document.getElementById('split-enable');
  const slider = document.getElementById('split-point-slider');
  const readout = document.getElementById('split-point-readout');
  if (!enableEl || !slider || !readout) return;

  const pointRow = document.getElementById('split-point-row');
  enableEl.innerHTML = '';
  const row = tgRow('Split');
  enableEl.appendChild(row);
  const render = () => {
    row.classList.toggle('active', _splitEnabled);
    // The point only means anything with the split on, so grey it out
    // otherwise. The toggle itself stays live — it is the way back.
    if (pointRow) pointRow.classList.toggle('dimmed', !_splitEnabled);
  };
  render();

  row.addEventListener('pointerdown', (ev) => {
    ev.preventDefault();
    _splitEnabled = !_splitEnabled;
    render();
    // Only post while Layer 2 is live — see above. `_layer2On` is the view's
    // mirror of the same KeyState.
    if (_layer2On) window.vxn.send.setKeyMode(_splitEnabled ? 2 : 1);
  });

  const clampNote = (n) =>
    Math.max(SPLIT_MIN, Math.min(SPLIT_MAX, Math.round(Number(n))));

  slider.addEventListener('input', () => {
    const note = clampNote(slider.value);
    // Optimistic local repaint; a `split_point_changed` echo overwrites it.
    readout.textContent = noteName(note);
    window.vxn.send.setSplitPoint(note);
  });
  slider.addEventListener('dblclick', (ev) => {
    ev.preventDefault();
    slider.value = String(SPLIT_DEFAULT);
    readout.textContent = noteName(SPLIT_DEFAULT);
    window.vxn.send.setSplitPoint(SPLIT_DEFAULT);
  });

  // Engine-side echoes (state / preset load) reflect without re-posting.
  model.setSplitEnabled = (on) => {
    _splitEnabled = !!on;
    render();
  };
  model.setSplitPoint = (note) => {
    const n = clampNote(note);
    slider.value = String(n);
    readout.textContent = noteName(n);
  };
}

// Level meters (0240). Every `[data-meter]` element becomes a meter keyed by
// its attribute value, which is also the field name in the frame the Rust side
// ships (`master`, `l1`, `l2`, `dynIn`, `dynGr`). Declaring the mount in HTML
// keeps meters out of the param-cell machinery entirely — they carry no CLAP
// id, are never bound or rebound by layer, and hold no model state.
export function wireMeters() {
  document.querySelectorAll('[data-meter]').forEach((el) => {
    const key = el.dataset.meter;
    if (!key) return;
    // `dynGr` is the compressor's reduction — one channel, drawn downward from
    // 0 dB, and with no ballistics of its own (the compressor's own envelope is
    // the movement). Everything else is a stereo level pair.
    const isGr = el.dataset.meterKind === 'gr';
    meterRegistry.register(
      key,
      makeMeter(el, { channels: isGr ? 1 : 2, kind: isGr ? 'gr' : 'level' }),
    );
  });
}

// Layer scope (`[data-scope]`). Like a meter mount it carries no CLAP id and
// holds no model state, so it stays out of the param-cell machinery entirely.
//
// The panel and the audio-side capture are one switch: `syncScopeSource` posts
// the tap that the layer pane is currently showing — or `off` whenever the
// scope is not on screen — and that same opcode is what makes the audio thread
// write into the ring at all. Nothing is captured for a layer nobody is
// looking at, and nothing at all while the FX/Global tab is up.
let _scope = null;
let _scopeSource = null;

// Test-only reset, for the same reason as `_resetKeyStateView`: these are
// module-level in production because the editor is one long-lived page, but the
// suites mount the faceplate repeatedly and would otherwise carry one test's
// selected tap into the next.
export function _resetScopeView() {
  _scope = null;
  _scopeSource = null;
}

export function wireScope() {
  const el = document.querySelector('[data-scope]');
  if (!el) return;
  _scope = makeScope(el);
}

export function syncScopeSource() {
  if (!_scope) return;
  const pane = document.querySelector('[data-tab-pane="layer"]');
  const next = pane && pane.classList.contains('active') ? model.currentLayer : 'off';
  if (next === _scopeSource) return;
  _scopeSource = next;
  // The engine clears the ring on a tap change, so whatever is on the canvas
  // belongs to the layer we just left. Blank it rather than leave the wrong
  // layer's waveform up for the ~30 ms the ring takes to refill.
  _scope.clear();
  window.vxn.send.setScopeSource(next);
}

export function init() {
  // Categorize every mount point by descriptor name + kind, layer-
  // agnostic. The actual id resolution + primitive instantiation happens
  // in `rebindAllForLayer`, which is also what a layer flip calls.
  // Build the mod-matrix overlay first so its depth dials (data-control cells)
  // are present for the sweep below and get bound + rebound like any other cell.
  matrixOverlay.build();
  document.querySelectorAll('[data-control]').forEach((el) => {
    const name = el.dataset.param;
    if (!name) return;
    const kind = el.dataset.control;
    // A fixed-layer cell is never `layered`, even if it sits inside a
    // `data-layered` container — the two markers are mutually exclusive by
    // construction, and honouring `layered` would re-bind it off its pin.
    const fixedLayer = el.dataset.fixedLayer || null;
    const entry = {
      el, kind, name, fixedLayer,
      layered: fixedLayer ? false : isLayeredEl(el),
      // The markup's own classes, captured before any primitive has run.
      // `freshenCell` restores exactly this on a rebind, so the reset needs no
      // hand-maintained list of the classes each kind adds.
      baseClass: el.className,
    };
    if (kind === 'detune-legato') {
      entry.extras = {
        legatoName: el.dataset.legatoParam,
        modeName: el.dataset.modeParam,
      };
    }
    model.cells.push(entry);
  });
  // Tab shell + Layer 2 toggle (0219). Wired before the first rebind so the
  // layer pane starts on Layer 1 (upper) and the toggle reflects single mode.
  wireTabs();
  wireLayer2Toggle();
  // Cross-layer LFO 2 link (0217) — a hand-wired KeyState cell in the LFO 2
  // panel strip, so it must not be left to `rebindAllForLayer`.
  wireLfo2Link();
  // Copy Layer 1 → Layer 2 (0265) — a hand-wired PatchOp cell in the Voice
  // panel strip, same reason.
  wireCopyLayer();
  // Keyboard split (0220) — KeyState cells on the FX/Global tab, hand-wired
  // for the same reason as the LFO 2 link.
  wireSplit();
  // Level meters (0240). Mount points are `data-meter="<frame key>"`, so a
  // panel opts in from HTML and the registry needs no per-panel wiring.
  wireMeters();
  // Layer scope — mounted before the first `syncScopeSource` below, which is
  // what actually turns capture on.
  wireScope();
  // Build the name → id reverse index once, before the first rebind so
  // every per-cell `paramIdByName` lookup hits the cached map (N5).
  _paramIdByName = buildParamIndex();
  rebindAllForLayer(model.currentLayer);

  // Dispatch one ViewEvent from Rust. ParamChanged routes by id (with the
  // partner-rate / free-run / filter-mode / generic-dim side effects pulled
  // in from 0042–0044). EditLayerChanged triggers a full layered-cell
  // rebind (0045). Status flashes the lower-right pill (0046). KeyModeChanged
  // / PresetLoaded / PresetCorpusChanged are still pre-wiring — log when
  // verbose tracing is on so the contract is visible without spamming the
  // console during automation.
  const dispatch = function (ev) {
    if (ev.kind === 'param_changed') {
      // Cache last-seen value so the sync-flip / dim-refresh / layer-
      // rebind reseed paths can reapply without waiting for the next echo.
      model.lastParam.set(ev.id, { plain: ev.plain, norm: ev.norm, display: ev.display });
      const ctls = model.controls.get(ev.id);
      if (ctls) for (const c of ctls) c.update(ev.plain, ev.norm, ev.display);
      // If this is an LFO/Delay sync toggle, the partnered rate/time fader
      // display label needs to flip Hz/s ↔ subdivision. Re-update the
      // partner with its last-seen value — the fader's displayOverride
      // will recompute.
      const rateId = model.rateOfSync.get(ev.id);
      if (rateId != null) {
        const last = model.lastParam.get(rateId);
        const rateCtls = model.controls.get(rateId);
        if (last && rateCtls) {
          for (const c of rateCtls) c.update(last.plain, last.norm, last.display);
        }
      }
      // Cutoff Tuned toggled: refresh the cutoff fader so its norm + popup
      // pick up the new mode (linear MIDI map + note-name display, or
      // exp-Hz default).
      const cutoffId = model.cutoffOfTuned.get(ev.id);
      if (cutoffId != null) {
        const last = model.lastParam.get(cutoffId);
        const cutoffCtls = model.controls.get(cutoffId);
        if (last && cutoffCtls) {
          for (const c of cutoffCtls) c.update(last.plain, last.norm, last.display);
        }
      }
      // Unified dim rules: source-Off / Cross Mod Type ≠ FM (0044) plus
      // the built-in Free-run (0042) and Filter Mode = Notch (0043).
      applyDimRulesFor(ev.id, ev.plain);
      return;
    }
    if (ev.kind === 'edit_layer_changed') {
      const layer = ev.layer === 'lower' ? 'lower' : 'upper';
      // The Keys panel's Upper/Lower toggle always follows — it owns
      // its own active-row paint regardless of whether the layer
      // actually flipped (cheap idempotent setter).
      keysPanel.setLayer(layer);
      if (layer === model.currentLayer) return;
      model.currentLayer = layer;
      rebindAllForLayer(layer);
      // A controller-driven layer flip moves the scope's tap too — the page
      // does not always originate the change.
      syncScopeSource();
      return;
    }
    if (ev.kind === 'key_mode_changed') {
      keysPanel.setMode(ev.mode);
      // KeyMode is derived from the two toggles, so an echo decomposes back
      // into them (0220): 0 = Single, 1 = Dual, 2 = Split. Both setters are
      // reflect-only — they repaint without re-posting, so a state/preset load
      // can't bounce an opcode back at the engine.
      if (model.setLayer2On) model.setLayer2On(ev.mode >= 1);
      if (model.setSplitEnabled) model.setSplitEnabled(ev.mode === 2);
      return;
    }
    if (ev.kind === 'split_point_changed') {
      keysPanel.setSplit(ev.note);
      if (model.setSplitPoint) model.setSplitPoint(ev.note);
      return;
    }
    // Keyboard echo (0221). KeyState — the Layer 2 toggle, the split and its
    // point, the LFO 2 link — is not a CLAP param, so a preset load / host state
    // load / undo moves it with nothing in the param machinery to carry the
    // news. The engine diffs it each tick and pushes this on any drift; without
    // it a loaded split patch plays split while the faceplate still reads
    // Single. Reflect-only: every setter here repaints without posting, so an
    // echo can't bounce an opcode back at the engine.
    if (ev.kind === 'keys') {
      // `mode` is the derived 0/1/2 (Single/Dual/Split), the same encoding
      // `setKeyMode` posts — decompose it back into the two toggles.
      keysPanel.setMode(ev.mode);
      if (model.setLayer2On) model.setLayer2On(ev.mode >= 1);
      if (model.setSplitEnabled) model.setSplitEnabled(ev.mode === 2);
      if (ev.split != null) {
        keysPanel.setSplit(ev.split);
        if (model.setSplitPoint) model.setSplitPoint(ev.split);
      }
      if (model.setLfo2Link) model.setLfo2Link(!!ev.link);
      return;
    }
    // Matrix topology echo (0247). Topology is not a CLAP param, so a preset
    // load / host state load / undo rewrites it with nothing in the param
    // machinery to carry the news; without this the source/dest combos keep
    // showing the previous patch until the editor is reopened. Reflect-only —
    // swapping the snapshot and repainting posts no `set_matrix`, so an echo
    // can't bounce back at the engine.
    if (ev.kind === 'matrix') {
      if (window.vxn && window.vxn.matrix && Array.isArray(ev.slots)) {
        window.vxn.matrix.slots = ev.slots;
        matrixOverlay.refreshForLayer(model.currentLayer);
      }
      return;
    }
    // Meter frame (0240). Raw linear peaks since the previous frame; the
    // registry fans them to whichever meters are mounted and the rAF loop
    // renders the ballistics. Purely view-bound — nothing here touches the
    // model, so the MVC parity rule is unaffected.
    if (ev.kind === 'meters') {
      meterRegistry.apply(ev);
      return;
    }
    // Scope window (oldest → newest) for whichever layer the page asked for.
    // Purely view-bound, like the meter frame: the trigger search and the
    // drawing are the widget's business, and nothing here touches the model.
    if (ev.kind === 'scope') {
      if (_scope && Array.isArray(ev.s)) _scope.push(ev.s);
      return;
    }
    if (ev.kind === 'status') {
      statusPill.flash(ev.line);
      return;
    }
    if (ev.kind === 'text_input_result') {
      // Fire-once: drop the entry before invoking so a re-entrant
      // promptText() from inside the callback can't see a stale id.
      const cb = _textInputCallbacks.get(ev.id);
      if (cb) {
        _textInputCallbacks.delete(ev.id);
        try { cb(ev.value == null ? null : ev.value); }
        catch (e) { console.warn('promptText callback threw', e); }
      }
      return;
    }
    if (ev.kind === 'preset_loaded') {
      // 0049: preset bar name binds here. Warnings (if any) flash
      // through the status chip — they belong with the load result,
      // not in the corner.
      presetBar.setName(ev.name);
      // 0094: also seeds the Save (overwrite) button — enabled iff the
      // source is a user preset AND a later write marks the patch dirty.
      presetBar.setSource(ev.source || null);
      // 0050: feed the browser panel's "currently loaded" highlight
      // from the same event. `source` is null on host state-load
      // (no on-disk anchor) — the panel just clears the highlight.
      browserPanel.setCurrentSource(ev.source || null);
      if (Array.isArray(ev.warnings) && ev.warnings.length) {
        statusPill.flash(ev.warnings.join('; '));
      }
      return;
    }
    // 0050: corpus snapshot arrives via __vxn.applyPresetCorpus
    // (separate Rust→JS channel), not through this batch. The
    // PresetCorpusChanged ViewEvent is the trigger for that push, so
    // by the time we get here the corpus is already rendered.
    // 0052: a non-null `follow` means a Move/Rename produced a new
    // path — jump the panel to its new folder and scroll it into view.
    if (ev.kind === 'preset_corpus_changed') {
      if (ev.follow) browserPanel.followPath(ev.follow);
      return;
    }
    // key_mode_changed lands here too. Uncomment for verbose
    // tracing during development:
    // console.log('vxn:view', ev);
  };
  // Batched bridge entry — Rust calls this once per controller tick.
  const applyViewEvents = function (arr) {
    for (const ev of arr) dispatch(ev);
  };
  // Replay any events buffered between bootstrap and init.
  for (const ev of _earlyViewEvents) dispatch(ev);
  _earlyViewEvents.length = 0;
  window.__vxn.applyViewEvents = applyViewEvents;
  window.vxn.onViewEvent = dispatch;

  // Point the capture ring at the pane that is actually showing (Layer 1 on a
  // fresh open). Until this lands the audio thread captures nothing.
  syncScopeSource();

  // Tell the controller we're ready — it re-broadcasts every param + key
  // mode so any first-tick `push_param_diffs` that ran before
  // `window.vxn` even existed (real race against wry's HTML load) gets
  // re-sent into a now-wired dispatcher. Without this, sliders that
  // never received their seed `ParamChanged` show an empty hover popup
  // until the user wiggles them.
  window.vxn.send.ready();
}

// E015 / 0077: skip the auto-bootstrap when this module is loaded headless
// under Node (no faceplate DOM mounted, no bridge.js side-effects, no
// `window.vxn`). The pure-helper test suite must not trigger `init`.
if (typeof document !== 'undefined' && document.getElementById('faceplate')) {
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
}
