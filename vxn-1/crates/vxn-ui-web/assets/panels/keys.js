// panels/keys.js — the Keys panel (mode / edit-layer / split-point / reset +
// the per-layer level sliders).
//
// Mirrors `vxn_ui_vizia::keys_panel`. The mode / edit / split widgets write
// *non-automatable* shared state directly (ADR 0003 §3/§8) — no gestures, no
// host echo — and the controller broadcasts back via `KeyModeChanged` /
// `EditLayerChanged` / `SplitPointChanged` so the panel reseeds after a
// preset/state load.
//
// `import` lines are dropped by the splice loader (the bindings ride the
// concatenated scope: `noteName` from the bridge slot, `tgRow` from
// util/drag.js, `paramIdByNameAtLayer` / `addCtl` from dispatch.js, which is
// concatenated after this module but only referenced from `wireLayerLevels()`
// at editor-ready time). Under Node ESM they resolve so the suites can pull the
// consts + `keysPanel` via the `../panels.js` barrel. The `../bridge.js`
// side-effect `import` guarantees `window.vxn` exists before this evaluates.
import '../bridge.js';
import { noteName } from '../../../../../crates/vxn-core-ui-web/assets/cutoff-tuned.js';
import { tgRow } from '../util/drag.js';
import { paramIdByNameAtLayer, addCtl } from '../dispatch.js';

export const KEY_MODE_NAMES = ['WHOLE', 'DUAL', 'SPLIT'];
export const KEY_LAYERS = [
  { code: 'upper', label: 'UPPER' },
  { code: 'lower', label: 'LOWER' },
];
// Match `DEFAULT_SPLIT_POINT` in vxn-app/src/domain.rs — C4.
export const KEYS_DEFAULT_SPLIT = 60;
// Mirror `vxn_ui_vizia::SPLIT_MIN` / `SPLIT_MAX`: narrower than the full
// MIDI range so every semitone is easy to land on.
export const KEYS_SPLIT_MIN = 12;
export const KEYS_SPLIT_MAX = 96;

export const keysPanel = (() => {
  const bodyEl = document.querySelector('.panel[data-name="Keys"] .panel-body');
  if (!bodyEl) return { setMode() {}, setLayer() {}, setSplit() {}, wireLayerLevels() {} };

  // mode: 0 Whole, 1 Dual, 2 Split. layer: 'upper' | 'lower'. split: MIDI
  // note in [KEYS_SPLIT_MIN, KEYS_SPLIT_MAX]. Controller-side defaults
  // re-arrive on `EditorReady` so the cold-start seed gets overwritten;
  // these initials just keep the markup valid until the first echo lands.
  let mode = 0;
  let layer = 'upper';
  let split = KEYS_DEFAULT_SPLIT;

  bodyEl.innerHTML = `
    <div class="keys-top">
      <div class="keys-tg-list" id="keys-mode-list"></div>
      <div class="keys-tg-list" id="keys-edit-list"></div>
    </div>
    <div class="keys-split-row" id="keys-split-row">
      <span class="keys-split-label">Split</span>
      <input type="range" class="keys-split-slider" id="keys-split-slider"
             min="${KEYS_SPLIT_MIN}" max="${KEYS_SPLIT_MAX}" step="1" />
      <div class="keys-split-readout" id="keys-split-readout"></div>
    </div>
    <div class="keys-level-row" data-layer="upper">
      <span class="keys-level-lbl">Upper</span>
      <div class="keys-level-track">
        <div class="keys-level-thumb"></div>
      </div>
    </div>
    <div class="keys-level-row" data-layer="lower">
      <span class="keys-level-lbl">Lower</span>
      <div class="keys-level-track">
        <div class="keys-level-thumb"></div>
      </div>
    </div>
    <button type="button" class="keys-reset" id="keys-reset">RESET</button>
  `;
  const modeListEl   = bodyEl.querySelector('#keys-mode-list');
  const editListEl   = bodyEl.querySelector('#keys-edit-list');
  const splitRowEl   = bodyEl.querySelector('#keys-split-row');
  const splitSlider  = bodyEl.querySelector('#keys-split-slider');
  const splitReadout = bodyEl.querySelector('#keys-split-readout');
  const resetBtn     = bodyEl.querySelector('#keys-reset');

  function renderModeList() {
    modeListEl.innerHTML = '';
    KEY_MODE_NAMES.forEach((label, i) => {
      const row = tgRow(label);
      if (i === mode) row.classList.add('active');
      // pointerdown not click: the more responsive surface.
      row.addEventListener('pointerdown', (ev) => {
        ev.preventDefault();
        if (i === mode) return;
        window.vxn.send.setKeyMode(i);
      });
      modeListEl.appendChild(row);
    });
  }
  function renderEditList() {
    editListEl.innerHTML = '';
    // In Whole the edit toggle is meaningless (both layers share one patch);
    // keep it visible but dim so the layout doesn't shift and the user sees
    // what control is parked. `.dimmed` greys it out and blocks clicks.
    editListEl.classList.toggle('dimmed', mode === 0);
    KEY_LAYERS.forEach(({ code, label }) => {
      const row = tgRow(label);
      if (code === layer) row.classList.add('active');
      row.addEventListener('pointerdown', (ev) => {
        ev.preventDefault();
        if (code === layer) return;
        window.vxn.send.setEditLayer(code);
      });
      editListEl.appendChild(row);
    });
  }
  function renderSplit() {
    // Only meaningful in Split (mode 2). Dim in Whole/Dual so the layout
    // is stable and the user sees that the slider exists but is parked.
    splitRowEl.classList.toggle('dimmed', mode !== 2);
    splitSlider.value = String(split);
    splitReadout.textContent = noteName(split);
  }

  splitSlider.addEventListener('input', () => {
    const note = Math.max(
      KEYS_SPLIT_MIN,
      Math.min(KEYS_SPLIT_MAX, Math.round(Number(splitSlider.value))),
    );
    // Optimistic local repaint of the readout; the echo from
    // `split_point_changed` will overwrite when it arrives.
    splitReadout.textContent = noteName(note);
    window.vxn.send.setSplitPoint(note);
  });
  splitSlider.addEventListener('dblclick', (ev) => {
    ev.preventDefault();
    window.vxn.send.setSplitPoint(KEYS_DEFAULT_SPLIT);
  });
  resetBtn.addEventListener('pointerdown', (ev) => {
    ev.preventDefault();
    // In Whole the two layers share one patch — reset both. In Dual /
    // Split reset only the layer the edit toggle points at. Globals,
    // key mode and split point are setup state, left untouched.
    if (mode === 0) {
      window.vxn.send.resetLayer('upper');
      window.vxn.send.resetLayer('lower');
    } else {
      window.vxn.send.resetLayer(layer);
    }
  });

  renderModeList();
  renderEditList();
  renderSplit();

  // Per-layer level sliders (Upper/Lower) — wired by dispatch from
  // `rebindAllForLayer` (NOT init alone). Each row paints its own thumb at
  // `norm * 100 %` along the track; the param is per-patch so each layer
  // has its own fixed CLAP id (these rows don't follow the edit-layer
  // toggle). No popup readout — the slider's position is the display.
  //
  // `rebindAllForLayer` clears `model.controls`, so we MUST re-register
  // the `addCtl` subscribers on every call — otherwise our echo updaters
  // would be wiped between the init-time wire and the first ParamChanged
  // burst. DOM event listeners attach exactly once per layer, guarded by
  // `levelEventsWired`.
  const levelEventsWired = { upper: false, lower: false };
  function wireLayerLevels() {
    const paint = (thumb, norm) => {
      const n = Math.max(0, Math.min(1, norm));
      thumb.style.left = (n * 100) + '%';
      // Mirror the fill boundary onto the track so the blue gradient fills up
      // to the knob (matches the vertical faders). parent is `.keys-level-track`.
      if (thumb.parentElement) thumb.parentElement.style.setProperty('--level-norm', n);
    };
    for (const which of ['upper', 'lower']) {
      const id = paramIdByNameAtLayer('layer_level', which);
      if (id == null) continue;
      const row = bodyEl.querySelector(`.keys-level-row[data-layer="${which}"]`);
      if (!row) continue;
      const track = row.querySelector('.keys-level-track');
      const thumb = row.querySelector('.keys-level-thumb');
      if (!track || !thumb) continue;

      // Subscriber — re-registered on every call so the layer-level
      // sliders survive a `rebindAllForLayer` `model.controls.clear()`.
      addCtl(id, { update: (_plain, norm) => paint(thumb, norm) });

      if (levelEventsWired[which]) continue;
      levelEventsWired[which] = true;

      const pointerToNorm = (ev) => {
        const r = track.getBoundingClientRect();
        return Math.max(0, Math.min(1, (ev.clientX - r.left) / r.width));
      };
      let dragging = false;
      // Move / stop live on `window`, not the track: a drag that leaves the
      // control keeps tracking, and a release ANYWHERE reliably ends it. A
      // track-only `pointerup` was missed when the button came up outside the
      // track, leaving the slider stuck in the dragging state. Both handlers
      // no-op unless this row is the one being dragged, so the two rows'
      // window listeners don't interfere.
      const onMove = (ev) => {
        if (!dragging) return;
        const n = pointerToNorm(ev);
        paint(thumb, n);
        window.vxn.send.setParamNorm(id, n);
      };
      const onUp = () => {
        if (!dragging) return;
        dragging = false;
        window.vxn.send.endGesture(id);
      };
      track.addEventListener('pointerdown', (ev) => {
        ev.preventDefault();
        dragging = true;
        window.vxn.send.beginGesture(id);
        const n = pointerToNorm(ev);
        paint(thumb, n);
        window.vxn.send.setParamNorm(id, n);
      });
      window.addEventListener('pointermove', onMove);
      window.addEventListener('pointerup', onUp);
      window.addEventListener('pointercancel', onUp);
      track.addEventListener('dblclick', (ev) => {
        ev.preventDefault();
        // Default = unity (1.0). Wrap in a gesture so it lands as one edit.
        window.vxn.send.beginGesture(id);
        window.vxn.send.setParamNorm(id, 1.0);
        window.vxn.send.endGesture(id);
      });
    }
  }

  return {
    setMode(m) {
      if (m === mode) return;
      mode = m;
      renderModeList();
      renderEditList();
      renderSplit();
    },
    setLayer(l) {
      if (l === layer) return;
      layer = l;
      renderEditList();
    },
    setSplit(n) {
      if (n === split) return;
      split = n;
      // Only the slider/readout change — no mode/layer visibility flip.
      splitSlider.value = String(split);
      splitReadout.textContent = noteName(split);
    },
    wireLayerLevels,
  };
})();
