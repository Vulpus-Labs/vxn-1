// panels/matrix.js — the mod-matrix overlay (0219, absorbing 0210).
//
// Ported from vxn-2's `panels/mod-matrix.js` (styling + interaction) and adapted
// to vxn-1b's two-layer model: a **modal** (backdrop + panel + close) holding a
// 16-slot editor — one row per slot: on / source / dest / depth / curve /
// scale / scale-curve. It is **per layer** — the same DOM rebinds to the active
// edit layer.
//
// Each curve is one glyph button opening a shared 3×3 picker (0340,
// `vxn-core-ui-web/assets/curve-picker.js`), not the two pick-lists it used to
// take. VXN1b stores the two axes as separate bytes, so a pick posts two edits;
// the picker hands the pair back already split.
//
// Custom div-combos, NOT native <select>: on macOS (WKWebView, the in-DAW wry
// backend) a native <select> opens an NSMenu that steals first-responder from
// the webview, so the first click after picking is swallowed. A DOM-only combo
// (`buildCombo`) keeps focus in the page. (vxn-2 mod-matrix.js §buildSelect.)
//
// Depth is the automatable `matrix_slot{n}_depth` CLAP dial (a data-control cell
// dispatch binds + rebinds + echoes); the topology controls are
// non-automatable and post `set_matrix` custom ops, reflecting from the local
// `window.vxn.matrix` snapshot (MVC: never read the engine model).
import '../bridge.js';
import { paramIdByNameAtLayer } from '../dispatch.js';

import { createCurveButton } from '../../../../../crates/vxn-core-ui-web/assets/curve-picker.js';

const layerIdx = (layer) => (layer === 'lower' ? 1 : 0);

// Snapshot key ↔ wire field name. They differ for the scale VCA's two axes
// only, because the wire vocabulary is kebab-case (`vocab::MATRIX_FIELD_NAMES`)
// while the snapshot JSON is camelCase like the rest of `window.vxn`. Keeping
// the pairing in one table means neither spelling is written out twice.
const FIELDS = [
  { key: 'source', wire: 'source' },
  { key: 'dest', wire: 'dest' },
  { key: 'polarity', wire: 'polarity' },
  { key: 'shape', wire: 'shape' },
  { key: 'scale', wire: 'scale' },
  // The scale VCA's own two axes (0341), driven by the same glyph picker the
  // route's pair uses (0340). Neither has a combo of its own any more, so the
  // reseed loop below skips them by element lookup and the curve buttons are
  // repainted from the snapshot separately.
  { key: 'scalePolarity', wire: 'scale-polarity' },
  { key: 'scaleShape', wire: 'scale-shape' },
];

// The two curves a row carries, as (button key, snapshot axis keys, label).
// One table so the build, the repaint and the bin's clear cannot disagree about
// which axes a button owns.
// The `curve_code` stride — how many shapes a polarity strides over. Read off
// the engine's own shapes vocabulary rather than written as 3, so composing a
// flat code here cannot disagree with `curve_code` if a bend is ever added.
// Falls back to 3 only for the pre-descriptor blank state, where there are no
// rows to paint anyway.
const curveStride = () => (matrixData().shapes || []).length || 3;

const CURVES = [
  {
    key: 'curve',
    polarity: 'polarity',
    shape: 'shape',
    title: 'Curve',
    cls: 'vxn-mm-curve',
  },
  {
    key: 'scaleCurve',
    polarity: 'scalePolarity',
    shape: 'scaleShape',
    title: 'Scale curve',
    cls: 'vxn-mm-scale-curve',
  },
];
const WIRE_OF = Object.fromEntries(FIELDS.map((f) => [f.key, f.wire]));
// The shape of a slot with nothing set — also what the bin resets a row to.
const BLANK_SLOT = {
  source: 0,
  dest: 0,
  polarity: 0,
  shape: 0,
  scale: 0,
  scalePolarity: 0,
  scaleShape: 0,
  enabled: false,
};

function matrixData() {
  return (
    (window.vxn && window.vxn.matrix) || {
      sources: [],
      dests: [],
      polarities: [],
      shapes: [],
      curves: [],
      curveGlyphs: [],
      pickerCodes: [],
      slots: [[], []],
    }
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
    for (const t of [
      '',
      '#',
      'Source',
      'Destination',
      'Amount',
      // Abbreviated: the curve columns are 38px, and spelling them out wraps
      // the header onto two lines for the sake of two glyph buttons.
      'Crv',
      'Scale By',
      'Scl Crv',
      '',
    ]) {
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

    // The on/off switch. Separate from the wiring: a route can be set up and
    // switched off, which is what makes A/B-ing one non-destructive.
    const onBox = document.createElement('input');
    onBox.type = 'checkbox';
    onBox.className = 'vxn-mm-on';
    onBox.title = 'Route on / off';

    const src = buildCombo(mx.sources, 'source');
    const dst = buildCombo(mx.dests, 'dest');
    const scale = buildCombo(mx.sources, 'scale');
    scale.classList.add('vxn-mm-scale');

    // One glyph button per curve (0340), replacing the polarity + shape combos
    // on each. The button draws the resulting mapping; the 3x3 picker behind it
    // is where the two axes get chosen. VXN1b stores the axes as separate
    // bytes, so a pick sends two edits — the picker hands both back already
    // split, so nothing here re-derives the stride.
    const labels = (mx.curves || []).map((c) => c.label);
    const curveBtns = {};
    for (const c of CURVES) {
      const btn = createCurveButton({
        glyphs: mx.curveGlyphs || [],
        labels,
        codes: mx.pickerCodes || [],
        title: c.title,
        className: c.cls,
        onPick: (_code, pol, shp) => {
          this._edit(slot, c.polarity, pol);
          this._edit(slot, c.shape, shp);
          this._markActive(row);
        },
      });
      btn.dataset.curve = c.key;
      curveBtns[c.key] = btn;
    }

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

    for (const sel of [src, dst, scale]) {
      sel.addEventListener('change', () => {
        const field = sel.dataset.field;
        const value = Number(sel.value);
        // Read the *previous* source before the edit lands: the auto-enable
        // below keys on the None→real edge, which is gone once the snapshot is
        // updated.
        const before = this._snap(slot);
        const wasBlank = !!before && before.source === 0 && !before.enabled;
        this._edit(slot, field, value);
        // First-time source pick: a blank row sits at source=None switched off,
        // so choosing a real source switches it on rather than making the
        // player click twice. Only on that edge — retuning a route the player
        // deliberately switched off must leave it off. (vxn-2 does the same.)
        if (field === 'source' && value !== 0 && wasBlank) {
          this._edit(slot, 'enabled', true);
          onBox.checked = true;
        }
        this._markActive(row);
      });
    }
    onBox.addEventListener('change', () => {
      this._edit(slot, 'enabled', onBox.checked);
      this._markActive(row);
    });
    bin.addEventListener('click', () =>
      this._clear(slot, row, { src, dst, scale, onBox, ...curveBtns })
    );

    for (const child of [
      onBox, num, src, dst, depth,
      curveBtns.curve, scale, curveBtns.scaleCurve, bin,
    ]) {
      row.appendChild(child);
    }
    return row;
  },

  _snap(slot) {
    return matrixData().slots[layerIdx(this._layer)][slot];
  },

  // Post a topology edit + keep the local snapshot in step (no model read).
  // `enabled` is a boolean in the snapshot and 0/1 on the wire; every other
  // field is already the wire `u8`.
  _edit(slot, field, value) {
    const snap = this._snap(slot);
    if (snap) snap[field] = value;
    const wire = WIRE_OF[field] || field;
    const wireValue = value === true ? 1 : value === false ? 0 : value;
    window.vxn.send.setMatrix(this._layer, slot, wire, wireValue);
  },

  // Clear a slot: zero every topology field, switch it off, and zero the depth
  // CLAP param.
  _clear(slot, row, combos) {
    for (const { key } of FIELDS) {
      this._edit(slot, key, 0);
      const el = combos[key === 'source' ? 'src' : key === 'dest' ? 'dst' : key];
      if (el) el.value = '0';
    }
    // The four curve axes have no combo of their own since 0340 — the two
    // buttons repaint from the zeroed axes instead.
    for (const c of CURVES) {
      const btn = combos[c.key];
      if (btn) btn.setCode(0);
    }
    this._edit(slot, 'enabled', false);
    if (combos.onBox) combos.onBox.checked = false;
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
      const s = slots[Number(row.dataset.slot)] || BLANK_SLOT;
      for (const { key } of FIELDS) {
        const el = row.querySelector(`.vxn-mm-combo[data-field="${key}"]`);
        if (el) el.value = String(s[key] ?? 0);
      }
      // The curve buttons are not combos: recompose each pair into the flat
      // code the glyph is indexed by, with the same stride `curve_code` uses
      // and `createCurveButton` splits back on the way out.
      for (const c of CURVES) {
        const btn = row.querySelector(`.cg-btn[data-curve="${c.key}"]`);
        if (btn) {
          btn.setCode((s[c.polarity] ?? 0) * curveStride() + (s[c.shape] ?? 0));
        }
      }
      const onBox = row.querySelector('.vxn-mm-on');
      if (onBox) onBox.checked = !!s.enabled;
      this._markActive(row);
    });
  },

  // Calm-when-sparse: a row reads active only when it is switched on **and**
  // both endpoints are real — the same predicate as `MatrixSlot::is_active`, so
  // a switched-off route greys out exactly as an unwired one does.
  _markActive(row) {
    const s = matrixData().slots[layerIdx(this._layer)][Number(row.dataset.slot)] || {};
    row.dataset.active = s.enabled && s.source > 0 && s.dest > 0 ? '1' : '0';
  },
};
