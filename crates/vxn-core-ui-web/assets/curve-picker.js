// Curve control — one glyph button per curve, opening a 3×3 polarity × shape
// picker (0340). Shared by both faceplates.
//
// Both synths spelled a route's shaping as two adjacent text pick-lists, which
// cost two grid columns per curve — four per row, once the scale VCA got its
// own axes at 0341 — and still left the player reading "Bipolar" + "Exp" and
// imagining the composition. One button showing the *resulting* mapping says it
// in the space of a single column.
//
// ## The glyph is not drawn here
//
// `glyphs` arrives from the engine, plotted by `vxn_core_matrix::glyph` from
// the same `shape(polarity, bend, v)` the audio thread runs. Re-deriving
// `sign(v)·√|v|` in this file would be a second spelling of arithmetic the
// shared crate exists to hold once — and one that could drift silently, since
// nothing would fail if it did. This module owns pixels and events; it owns no
// curve maths.
//
// ## DOM elements, never a native <select>
//
// Same reason both panels hand-roll their pick-lists: an NSMenu steals webview
// first-responder under macOS/WKWebView, so the first click after one closes
// goes nowhere. The picker is body-attached and `position: fixed` so an
// overlay's scroll container cannot clip it.
//
// ## What a panel supplies
//
// The vocabulary (`glyphs`, `labels`, `codes`) and an `onPick(code, polarity,
// shape)` callback. The callback gets all three because the two synths write
// the value differently: VXN2 carries one flat code in its packed row, while
// VXN1b stores the axes as separate bytes and sends one edit per axis. Neither
// arrangement belongs in here.
//
// ES module so the vitest suite can pull these in directly; the markers are
// stripped at splice time (`strip_esm_exports`). Prose in a spliced asset says
// "pull in" rather than the ES keyword for it, here and in the sibling
// primitives, because `esm_exports_stripped` greps the whole assembled page for
// that keyword and cannot tell a comment from a statement.

/// Shapes across the picker's columns. Rows come from `codes`, which the engine
/// orders None / Abs / Bipolar — see `vxn_core_matrix::glyph::POLARITY_ROWS`.
const SHAPE_HEADS = ['Lin', 'Exp', 'Log'];
const N_SHAPES = SHAPE_HEADS.length;

// One `<svg>` for a curve. `big` adds the frame a picker cell has room for —
// both axes, the identity diagonal, and the band shading the source range this
// polarity is written for. The row button drops all of it but a faint zero
// line: at 38×22 the axis cross turns all nine into hash marks.
export function curveGlyphSvg(glyph, big) {
  if (!glyph) return '';
  const band = big
    ? `<rect class="cg-band" x="${glyph.band_x}" y="0" width="${glyph.band_w}" height="100"/>`
    : '';
  const ident = big ? '<polyline class="cg-curve cg-ident" points="0,100 100,0"/>' : '';
  const axes = big
    ? '<line class="cg-axis" x1="50" y1="0" x2="50" y2="100" vector-effect="non-scaling-stroke"/>'
      + '<line class="cg-axis" x1="0" y1="50" x2="100" y2="50" vector-effect="non-scaling-stroke"/>'
    : '<line class="cg-axis cg-faint" x1="0" y1="50" x2="100" y2="50"'
      + ' vector-effect="non-scaling-stroke"/>';
  return `<svg viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true">`
    + `${band}${axes}${ident}<polyline class="cg-curve" points="${glyph.points}"/></svg>`;
}

// The one body-attached picker, created lazily so importing this module does
// not touch the DOM. Only one can be open, which is also what makes "click the
// open button again" a close rather than a reopen.
const picker = (() => {
  let el = null;
  let anchor = null;      // the button the open picker belongs to
  let onPick = null;
  let listening = false;

  function ensure() {
    // Re-attach rather than only re-create: a page that replaces `body`'s
    // contents (the vitest suites do it between cases, and a faceplate reload
    // could) detaches this node while the closure still holds it, and a
    // detached picker opens invisibly — `hidden = false` on an orphan.
    if (el && el.isConnected) return el;
    if (!el) {
      el = document.createElement('div');
      el.className = 'cg-picker';
    }
    el.hidden = true;
    document.body.appendChild(el);
    return el;
  }

  // Cancel paths — Esc, a click outside, a resize or scroll that would leave
  // the picker floating away from its button. None of them edit.
  function onKey(ev) {
    if (ev.key === 'Escape') {
      ev.stopPropagation();
      close();
    }
  }
  function onDocClick(ev) {
    if (el && el.contains(ev.target)) return;
    if (anchor && anchor.contains(ev.target)) return;
    close();
  }
  function listen(on) {
    if (on === listening) return;
    listening = on;
    // The picker's Esc / arrow keys only reach the page while the page holds
    // the keyboard (0364). Claimed for exactly as long as the listeners are
    // attached, so the two can't drift apart.
    if (window.__vxnKeyboard) {
      window.__vxnKeyboard[on ? 'claim' : 'release']('curve-picker');
    }
    const fn = on ? 'addEventListener' : 'removeEventListener';
    // Capture phase for the key, so Esc closes the picker before an overlay
    // above it reads the same key as "close the whole panel".
    document[fn]('keydown', onKey, true);
    document[fn]('click', onDocClick, true);
    window[fn]('resize', close);
    window[fn]('scroll', close, true);
  }

  function place(btn) {
    const r = btn.getBoundingClientRect();
    const p = el.getBoundingClientRect();
    let left = r.left + r.width / 2 - p.width / 2;
    left = Math.max(8, Math.min(left, window.innerWidth - p.width - 8));
    let top = r.bottom + 6;
    // Flip above the button when there is no room below.
    if (top + p.height > window.innerHeight - 8) top = Math.max(8, r.top - p.height - 6);
    el.style.left = `${left}px`;
    el.style.top = `${top}px`;
  }

  function open(btn, cfg) {
    const e = ensure();
    const { glyphs, labels, codes, title, code } = cfg;
    onPick = cfg.onPick;
    anchor = btn;
    const rows = [];
    for (let r = 0; r < codes.length / N_SHAPES; r++) {
      const first = codes[r * N_SHAPES];
      // The row's own name is the polarity half of its first cell's label; the
      // resting row's labels are bare shape names, so it is spelled out here.
      const head = first === 0 ? 'None' : String(labels[first] || '').split(' ')[0];
      const cells = [];
      for (let c = 0; c < N_SHAPES; c++) {
        const k = codes[r * N_SHAPES + c];
        const sel = k === code ? ' cg-sel' : '';
        cells.push(
          `<button type="button" class="cg-opt${sel}" data-code="${k}"`
          + ` title="${labels[k] || ''}" aria-label="${labels[k] || ''}"`
          + `${k === code ? ' aria-current="true"' : ''}>${curveGlyphSvg(glyphs[k], true)}</button>`,
        );
      }
      rows.push(`<span class="cg-rh">${head}</span>${cells.join('')}`);
    }
    e.innerHTML =
      `<div class="cg-title"><span>${title || 'Curve'}</span>`
      + `<em>${labels[code] || ''}</em></div>`
      + `<div class="cg-grid"><span></span>`
      + SHAPE_HEADS.map((s) => `<span class="cg-ch">${s}</span>`).join('')
      + rows.join('')
      + '</div>';
    e.hidden = false;
    place(btn);
    btn.classList.add('cg-open');
    btn.setAttribute('aria-expanded', 'true');
    listen(true);
  }

  function close() {
    if (!el) return;
    el.hidden = true;
    el.innerHTML = '';
    if (anchor) {
      anchor.classList.remove('cg-open');
      anchor.setAttribute('aria-expanded', 'false');
    }
    anchor = null;
    onPick = null;
    listen(false);
  }

  // The pick itself. Bound on the picker element rather than the document so a
  // stray click elsewhere reaches `onDocClick` (which cancels) instead.
  function bindOnce(e) {
    if (e.dataset.bound) return;
    e.dataset.bound = '1';
    e.addEventListener('click', (ev) => {
      const opt = ev.target.closest('.cg-opt');
      if (!opt) return;
      ev.stopPropagation();
      const code = Number(opt.dataset.code);
      const fn = onPick;
      close();
      if (fn) fn(code);
    });
  }

  return {
    open(btn, cfg) {
      const reopen = anchor === btn;
      close();
      if (reopen) return;
      open(btn, cfg);
      bindOnce(el);
    },
    close,
    isOpenFor(btn) {
      return anchor === btn;
    },
    el() {
      return el;
    },
  };
})();

// A curve button. Returns the element, with `setCode(code)` for the panel's
// repaint path.
//
// `onPick(code, polarity, shape)` fires only on a pick — never on a cancel —
// and the caller decides what to send. The axes are handed over already split
// so a panel that stores them separately does not re-derive the stride.
export function createCurveButton({
  glyphs,
  labels,
  codes,
  title = 'Curve',
  className = '',
  code = 0,
  onPick = null,
} = {}) {
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = `cg-btn${className ? ` ${className}` : ''}`;
  btn.setAttribute('aria-haspopup', 'true');
  btn.setAttribute('aria-expanded', 'false');
  let current = code;

  function paint() {
    const g = glyphs[current];
    btn.innerHTML = curveGlyphSvg(g, false);
    const label = labels[current] || '';
    btn.title = `${title} — ${label}`;
    btn.setAttribute('aria-label', `${title}: ${label}`);
    btn.dataset.code = String(current);
  }
  paint();

  btn.addEventListener('click', (ev) => {
    ev.stopPropagation();
    picker.open(btn, {
      glyphs,
      labels,
      codes,
      title,
      code: current,
      onPick(picked) {
        // Repaint optimistically: the engine echo that would otherwise be the
        // first visible change is a round trip away, and the panels all repaint
        // idempotently from the snapshot afterwards.
        btn.setCode(picked);
        if (onPick) onPick(picked, Math.floor(picked / N_SHAPES), picked % N_SHAPES);
      },
    });
  });

  btn.setCode = (next) => {
    const n = Number(next) | 0;
    if (n === current) return;
    current = n;
    paint();
  };
  btn.code = () => current;
  return btn;
}

// Close whatever is open — for a panel that hides its overlay out from under
// the picker.
export function closeCurvePicker() {
  picker.close();
}

// Test seam: the singleton's element, or `null` before first use.
export function curvePickerElement() {
  return picker.el();
}
