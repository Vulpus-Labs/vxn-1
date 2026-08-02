// panels/matrix.js — the mod-matrix overlay (0219, absorbing 0210).
//
// A 16-slot editor that overlays the layer pane, one row per slot: source /
// dest / bipolar depth / curve / scale-source. It is **per layer** — the same
// DOM rebinds to the active edit layer (Upper / Lower), exactly like the patch
// cells. `depth` is a normal automatable CLAP param (`matrix_slot{n}_depth`), so
// its dial is a `data-control` cell dispatch binds and rebinds; the four topology
// selectors are non-automatable and post `set_matrix` custom ops
// ([[vxn1b-two-layer-param-map]] / the KeyState wire pattern).
//
// MVC discipline (epic risk): the selectors never read the engine model — they
// emit an edit and reflect it from the local `window.vxn.matrix` snapshot. The
// depth dials go through the normal ParamChanged echo path. `import` lines are
// dropped by the splice loader; under Node ESM they resolve for the suites.
import '../bridge.js';

const MATRIX_SLOTS = 16;
const layerIdx = (layer) => (layer === 'lower' ? 1 : 0);
const FIELDS = ['source', 'dest', 'curve', 'scale'];

// The vocab + per-layer factory topology spliced by the Rust side. Read lazily
// so a test can install `window.vxn.matrix` before `build()`.
function matrixData() {
  return (
    (window.vxn && window.vxn.matrix) || { sources: [], dests: [], curves: [], slots: [[], []] }
  );
}

export const matrixOverlay = {
  _layer: 'upper',

  // Build the 16 rows into `#matrix-rows` and wire the selectors + toggle. Run
  // from `init()` BEFORE the [data-control] sweep so the depth dials are bound.
  build() {
    const rowsEl = document.getElementById('matrix-rows');
    if (!rowsEl) return;
    const mx = matrixData();

    const options = (vocab) =>
      vocab.map((o) => `<option value="${o.value}">${o.label}</option>`).join('');
    const sel = (cls, field, vocab) =>
      `<select class="mtx-sel ${cls}" data-field="${field}">${options(vocab)}</select>`;

    let html = '';
    for (let slot = 0; slot < MATRIX_SLOTS; slot++) {
      html +=
        `<div class="mtx-row" data-slot="${slot}">` +
        `<span class="mtx-num">${slot + 1}</span>` +
        sel('mtx-src', 'source', mx.sources) +
        sel('mtx-dst', 'dest', mx.dests) +
        `<div class="ctl mtx-depth" data-control="dial" data-param="matrix_slot${slot}_depth" data-layered data-label="Depth"></div>` +
        sel('mtx-curve', 'curve', mx.curves) +
        sel('mtx-scale', 'scale', mx.sources) +
        `</div>`;
    }
    rowsEl.innerHTML = html;

    rowsEl.querySelectorAll('.mtx-sel').forEach((s) => {
      s.addEventListener('change', () => {
        const row = s.closest('.mtx-row');
        const slot = Number(row.dataset.slot);
        const field = s.dataset.field;
        const value = Number(s.value);
        // Keep the local snapshot in step so a layer flip / re-render reflects
        // the edit without reading the engine.
        const snap = matrixData().slots[layerIdx(this._layer)][slot];
        if (snap) snap[field] = value;
        window.vxn.send.setMatrix(this._layer, slot, field, value);
        this._markActive(row);
      });
    });

    const overlayEl = document.getElementById('matrix-overlay');
    const toggleEl = document.getElementById('matrix-toggle');
    if (toggleEl && overlayEl) {
      toggleEl.addEventListener('click', () => {
        overlayEl.hidden = !overlayEl.hidden;
        toggleEl.classList.toggle('on', !overlayEl.hidden);
      });
    }
    this.refreshForLayer(this._layer);
  },

  // Reseed every selector from the active layer's snapshot. Called on a layer
  // flip (from `rebindAllForLayer`) so the overlay tracks the edit layer.
  refreshForLayer(layer) {
    this._layer = layer;
    const rowsEl = document.getElementById('matrix-rows');
    if (!rowsEl) return;
    const label = document.getElementById('matrix-layer-label');
    if (label) label.textContent = layer === 'lower' ? 'Layer 2' : 'Layer 1';
    const slots = matrixData().slots[layerIdx(layer)] || [];
    rowsEl.querySelectorAll('.mtx-row').forEach((row) => {
      const s = slots[Number(row.dataset.slot)] || { source: 0, dest: 0, curve: 0, scale: 0 };
      for (const f of FIELDS) {
        const el = row.querySelector(`.mtx-sel[data-field="${f}"]`);
        if (el) el.value = String(s[f] ?? 0);
      }
      this._markActive(row);
    });
  },

  // Calm-when-sparse: a slot reads as active only when both endpoints are real
  // (source and dest non-`none`). Inactive rows dim.
  _markActive(row) {
    const s = matrixData().slots[layerIdx(this._layer)][Number(row.dataset.slot)] || {};
    row.classList.toggle('mtx-active', s.source > 0 && s.dest > 0);
  },
};
