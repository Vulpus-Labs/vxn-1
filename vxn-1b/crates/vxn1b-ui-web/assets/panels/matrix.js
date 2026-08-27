// panels/matrix.js — the mod-matrix overlay (0219, absorbing 0210).
//
// Ported from vxn-2's `panels/mod-matrix.js` (styling + interaction) and adapted
// to vxn-1b's two-layer model: a **modal** (backdrop + panel + close) holding a
// 16-slot editor — one row per slot: source / dest / depth / curve / scale. It
// is **per layer** — the same DOM rebinds to the active edit layer.
//
// Custom div-combos, NOT native <select>: on macOS (WKWebView, the in-DAW wry
// backend) a native <select> opens an NSMenu that steals first-responder from
// the webview, so the first click after picking is swallowed. A DOM-only combo
// (`buildCombo`) keeps focus in the page. (vxn-2 mod-matrix.js §buildSelect.)
//
// Depth is the automatable `matrix_slot{n}_depth` CLAP dial (a data-control cell
// dispatch binds + rebinds + echoes); the four topology selectors are
// non-automatable and post `set_matrix` custom ops, reflecting from the local
// `window.vxn.matrix` snapshot (MVC: never read the engine model).
import '../bridge.js';
import { paramIdByNameAtLayer } from '../dispatch.js';

const layerIdx = (layer) => (layer === 'lower' ? 1 : 0);
const FIELDS = ['source', 'dest', 'curve', 'scale'];

function matrixData() {
  return (
    (window.vxn && window.vxn.matrix) || { sources: [], dests: [], curves: [], slots: [[], []] }
  );
}

// A custom div dropdown mimicking enough of <select>: settable `.value`, a
// "change" event on commit, and a body-attached fixed popup (so the overlay's
// overflow can't clip it). `vocab` entries are `{ value, label }`.
//
// A class rather than a stack of closures over shared `let`s: the popup's
// lifecycle is four methods that all read the same three fields (`_vocab`,
// `_value`, `_popup`), and the two document-level capture listeners need
// stable references to remove themselves with. Callers only ever see the
// element — `buildCombo` hands that back — so `.value` is installed on it and
// forwards here.
class Combo {
  constructor(vocab, field) {
    this._vocab = vocab;
    this._value = vocab.length ? String(vocab[0].value) : '0';
    this._popup = null;

    const btn = document.createElement('div');
    btn.className = 'vxn-mm-combo';
    btn.tabIndex = 0;
    btn.dataset.field = field;
    this._label = document.createElement('span');
    this._label.className = 'vxn-mm-combo-label';
    const caret = document.createElement('span');
    caret.className = 'vxn-mm-combo-caret';
    caret.textContent = '▾';
    btn.appendChild(this._label);
    btn.appendChild(caret);
    this.el = btn;
    this._render();

    Object.defineProperty(btn, 'value', {
      get: () => this._value,
      set: (v) => {
        this._value = String(v);
        this._render();
      },
    });

    // Bound once so `close()` can hand `removeEventListener` the same
    // references `open()` registered — both are capture-phase.
    this._onDocDown = (e) => {
      if (this._popup && !this._popup.contains(e.target) && !btn.contains(e.target)) {
        this._close();
      }
    };
    this._onKey = (e) => {
      if (e.key === 'Escape' || e.keyCode === 27) {
        e.stopPropagation(); // dismiss only the popup, not the whole overlay
        this._close();
      }
    };

    btn.addEventListener('mousedown', (e) => {
      e.preventDefault();
      btn.focus();
      if (this._popup) this._close();
      else this._open();
    });
  }

  _render() {
    const hit = this._vocab.find((o) => String(o.value) === String(this._value));
    this._label.textContent = hit ? hit.label : '';
  }

  _close() {
    if (this._popup) {
      this._popup.remove();
      this._popup = null;
    }
    document.removeEventListener('mousedown', this._onDocDown, true);
    document.removeEventListener('keydown', this._onKey, true);
    this.el.classList.remove('open');
  }

  _pick(entry) {
    const changed = String(entry.value) !== this._value;
    this._value = String(entry.value);
    this._render();
    this._close();
    this.el.blur();
    if (changed) this.el.dispatchEvent(new Event('change'));
  }

  _open() {
    const popup = document.createElement('div');
    this._popup = popup;
    popup.className = 'vxn-mm-combo-pop';
    for (const entry of this._vocab) {
      const opt = document.createElement('div');
      opt.className = 'vxn-mm-combo-opt';
      opt.textContent = entry.label;
      if (String(entry.value) === this._value) opt.classList.add('sel');
      opt.addEventListener('mousedown', (e) => {
        e.preventDefault(); // keep webview focus
        this._pick(entry);
      });
      popup.appendChild(opt);
    }
    document.body.appendChild(popup);
    // Fixed-position under the button, flipped above it when it would run off
    // the bottom of the window.
    const r = this.el.getBoundingClientRect();
    popup.style.position = 'fixed';
    popup.style.left = `${r.left}px`;
    popup.style.top = `${r.bottom}px`;
    popup.style.minWidth = `${r.width}px`;
    const pr = popup.getBoundingClientRect();
    if (pr.bottom > window.innerHeight) {
      popup.style.top = `${Math.max(0, r.top - pr.height)}px`;
    }
    this.el.classList.add('open');
    document.addEventListener('mousedown', this._onDocDown, true);
    document.addEventListener('keydown', this._onKey, true);
  }
}

function buildCombo(vocab, field) {
  return new Combo(vocab, field).el;
}

export const matrixOverlay = {
  _layer: 'upper',

  // Build the 16 rows into `#matrix-rows`, wire the combos + bin + the modal
  // open/close. Run from `init()` BEFORE the [data-control] sweep so the depth
  // dials are present to be bound.
  build() {
    const list = document.getElementById('matrix-rows');
    if (!list) return;
    const mx = matrixData();
    list.innerHTML = '';

    // Column header.
    const header = document.createElement('div');
    header.className = 'vxn-mm-header';
    for (const t of ['#', 'Source', 'Destination', 'Amount', 'Curve', 'Scale By', '']) {
      const h = document.createElement('span');
      h.className = 'vxn-mm-h';
      h.textContent = t;
      header.appendChild(h);
    }
    list.appendChild(header);

    // Row count comes from the snapshot, not a local constant: the Rust side
    // already tells us how many slots a layer has, and a second declaration of
    // `MATRIX_SLOTS` here would be one nothing compares (0316).
    for (let slot = 0; slot < mx.slots[0].length; slot++) {
      list.appendChild(this._buildRow(slot, mx));
    }

    // Modal open / close (toggle button, close button, backdrop click, Esc).
    const backdrop = document.getElementById('matrix-backdrop');
    const toggle = document.getElementById('matrix-toggle');
    const closeBtn = document.getElementById('matrix-close');
    const open = () => {
      if (backdrop) backdrop.hidden = false;
      if (toggle) toggle.classList.add('on');
    };
    const close = () => {
      if (backdrop) backdrop.hidden = true;
      if (toggle) toggle.classList.remove('on');
    };
    if (toggle) toggle.addEventListener('click', () => (backdrop && backdrop.hidden ? open() : close()));
    if (closeBtn) closeBtn.addEventListener('click', close);
    if (backdrop) {
      backdrop.addEventListener('mousedown', (e) => {
        if (e.target === backdrop) close();
      });
    }
    window.addEventListener('keydown', (e) => {
      if (backdrop && !backdrop.hidden && (e.key === 'Escape' || e.keyCode === 27)) {
        e.preventDefault();
        close();
      }
    });

    this.refreshForLayer(this._layer);
  },

  _buildRow(slot, mx) {
    const row = document.createElement('div');
    row.className = 'vxn-mm-row';
    row.dataset.slot = String(slot);

    const num = document.createElement('span');
    num.className = 'vxn-mm-slot-num';
    num.textContent = String(slot + 1);

    const src = buildCombo(mx.sources, 'source');
    const dst = buildCombo(mx.dests, 'dest');
    const curve = buildCombo(mx.curves, 'curve');
    const scale = buildCombo(mx.sources, 'scale');
    scale.classList.add('vxn-mm-scale');

    // Depth: the automatable per-layer bipolar CLAP fader (center-origin, signed
    // fill). dispatch binds/rebinds/echoes it like any other cell.
    // NB: no `.ctl` class — that imposes the 90px panel-column height, which
    // would top-pin the fader and blow out the row. `.vxn-mm-depth` sizes compact.
    const depth = document.createElement('div');
    depth.className = 'vxn-mm-depth';
    depth.dataset.control = 'bipolar';
    depth.dataset.param = `matrix_slot${slot}_depth`;
    depth.setAttribute('data-layered', '');
    depth.dataset.label = '';

    const bin = document.createElement('button');
    bin.type = 'button';
    bin.className = 'vxn-mm-bin';
    bin.title = 'Clear slot';
    bin.textContent = '✕';

    for (const sel of [src, dst, curve, scale]) {
      sel.addEventListener('change', () => {
        this._edit(slot, sel.dataset.field, Number(sel.value));
        this._markActive(row);
      });
    }
    bin.addEventListener('click', () => this._clear(slot, row, { src, dst, curve, scale }));

    for (const child of [num, src, dst, depth, curve, scale, bin]) row.appendChild(child);
    return row;
  },

  // Post a topology edit + keep the local snapshot in step (no model read).
  _edit(slot, field, value) {
    const snap = matrixData().slots[layerIdx(this._layer)][slot];
    if (snap) snap[field] = value;
    window.vxn.send.setMatrix(this._layer, slot, field, value);
  },

  // Clear a slot: zero the four topology fields + the depth CLAP param.
  _clear(slot, row, combos) {
    for (const f of FIELDS) {
      this._edit(slot, f, 0);
      combos[f === 'source' ? 'src' : f === 'dest' ? 'dst' : f].value = '0';
    }
    const id = paramIdByNameAtLayer(`matrix_slot${slot}_depth`, this._layer);
    if (id != null) window.vxn.send.setParam(id, 0);
    this._markActive(row);
  },

  // Reseed every combo from the active layer's snapshot; update the label.
  refreshForLayer(layer) {
    this._layer = layer;
    const list = document.getElementById('matrix-rows');
    if (!list) return;
    const label = document.getElementById('matrix-layer-label');
    if (label) label.textContent = layer === 'lower' ? 'Layer 2' : 'Layer 1';
    const slots = matrixData().slots[layerIdx(layer)] || [];
    list.querySelectorAll('.vxn-mm-row').forEach((row) => {
      const s = slots[Number(row.dataset.slot)] || { source: 0, dest: 0, curve: 0, scale: 0 };
      for (const f of FIELDS) {
        const el = row.querySelector(`.vxn-mm-combo[data-field="${f}"]`);
        if (el) el.value = String(s[f] ?? 0);
      }
      this._markActive(row);
    });
  },

  // Calm-when-sparse: a row reads active only when both endpoints are real.
  _markActive(row) {
    const s = matrixData().slots[layerIdx(this._layer)][Number(row.dataset.slot)] || {};
    row.dataset.active = s.source > 0 && s.dest > 0 ? '1' : '0';
  },
};
