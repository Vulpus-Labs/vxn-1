// panels/discrete.js — the click-to-pick widgets (Switch / ButtonGroup /
// Dropdown / HeaderSwitch) and the FX tab strip.
//
// All four pickers share the same write semantics: a click sends
// `begin_gesture` → `set_param` → `end_gesture` (via `send.discrete`) so the
// host records a single discrete edit. No drag, no popup.
//
// `import` lines are dropped by the splice loader (the sibling helpers ride the
// same concatenated scope); under Node ESM they resolve so the suites can pull
// these via the `../panels.js` barrel.
import { clampVariant, tgRow, paintFader } from '../util/drag.js';

// `Switch(id, label)` — vertical toggle for bools; also handles 2-variant
// enums (NoiseColor, FilterSlope, LfoSync, …) the way vizia's
// `Ctl::Switch` does, by rendering one toggle per variant in a row.
export function makeSwitch(el, id, desc) {
  const label = el.dataset.label || desc.label;
  const isEnum = desc.kind === 'enum';
  const entries = isEnum
    ? (desc.variants || []).map((name, i) => ({ idx: i, name }))
    : [{ idx: 1, name: label }];
  el.innerHTML = '';
  el.style.display = 'inline-flex';
  el.style.flexDirection = 'row';
  el.style.gap = '12px';
  el.style.alignItems = 'center';

  const rows = entries.map(({ idx, name }) => {
    const row = tgRow(name);
    row.addEventListener('pointerdown', (ev) => {
      ev.preventDefault();
      let plain;
      if (isEnum) {
        plain = idx;
      } else {
        // Bool: toggle current. `row.classList.contains('active')` is the
        // local truth; the round-trip echo will reconcile if the engine
        // refuses (clamped, gated).
        plain = row.classList.contains('active') ? 0 : 1;
      }
      window.vxn.send.discrete(id, plain);
    });
    el.appendChild(row);
    return { row, idx };
  });

  return {
    update(plain) {
      const p = isEnum
        ? clampVariant(plain, entries)
        : (plain >= 0.5 ? 1 : 0);
      rows.forEach(({ row, idx }) => row.classList.toggle('active', idx === p));
    },
  };
}

// `ButtonGroup(id, label, variants)` — for Oversample, CrossModType,
// AssignMode. Vertical stack of labelled toggles under a column label
// (matches vizia's `enum_list_body`).
//
// `data-no-label` — render no column header (used inside `.route-col`,
// where the route header (LFO/Env) is the only column label).
// `data-order` — comma-separated display permutation of the variant
// indices (e.g. `0,3,1,2` for AssignMode → Poly/Twin/Unison/Solo); the
// stored value stays each variant's own descriptor index. Mirrors
// vxn-ui-vizia's `ASSIGN_DISPLAY_ORDER`.
// `data-columns` — lay the rows out in N columns instead of one tall
// stack (Voice's six Widths). Column-major, so a 6-variant group in 2
// columns reads 1/2/4 · 8/16/32; the row count the CSS grid needs is
// derived here and passed down as `--ctl-rows`.
export function makeButtonGroup(el, id, desc) {
  const label = el.dataset.label || desc.label;
  const variants = desc.variants || [];
  const noLabel = el.hasAttribute('data-no-label');
  const orderRaw = (el.dataset.order || '').split(',')
    .map((s) => parseInt(s, 10))
    .filter((n) => !isNaN(n) && n >= 0 && n < variants.length);
  const order = orderRaw.length === variants.length
    ? orderRaw
    : variants.map((_, i) => i);
  // Tag the cell so `.ctl-buttongroup .ctl-tg-rows { flex-direction: column }`
  // kicks in — without this the inline-flex `.ctl-tg-row` children flow
  // horizontally and overflow the column. The shape (vertical alongside
  // faders inside panel-body) matches vizia's `enum_list_body`.
  el.classList.add('ctl-buttongroup');
  el.innerHTML =
    (noLabel ? '' : '<div class="ctl-label">' + label.toUpperCase() + '</div>') +
    '<div class="ctl-tg-rows"></div>';
  const rowsHost = el.querySelector('.ctl-tg-rows');
  // Rows per column, ceil'd so an odd variant count leaves the short column
  // last rather than dropping a row off the grid.
  const columns = Math.max(1, parseInt(el.dataset.columns, 10) || 1);
  if (columns > 1) {
    el.style.setProperty('--ctl-rows', String(Math.ceil(variants.length / columns)));
  }
  // `rows[i]` corresponds to variant index `i` (not display position), so
  // the update path can flip the active class by plain value directly.
  const rows = new Array(variants.length);
  for (const n of order) {
    const row = tgRow(variants[n]);
    row.addEventListener('pointerdown', (ev) => {
      ev.preventDefault();
      window.vxn.send.discrete(id, n);
    });
    rowsHost.appendChild(row);
    rows[n] = row;
  }
  return {
    update(plain) {
      const p = clampVariant(plain, variants);
      rows.forEach((row, i) => row && row.classList.toggle('active', i === p));
    },
  };
}

// `Rocker(id)` — a two-state switch with its variant names either end of the
// travel, stacked vertically:
//
//     POLY      <- variant 0 ("left")
//      (o)
//      | |      <- travel
//     SOLO      <- variant 1 ("right")
//
// Same wire shape as `makeSwitch` (one gesture-bracketed discrete write) for a
// different reading: a switch renders one checkbox per variant, which says the
// variants are independent things to enable. A mode is not — it is one choice
// with two positions — and the rocker draws exactly that. Clicking anywhere
// flips it, including on either label, so the whole control is the target.
//
// The `left`/`right` names are the wire order (variant 0 / variant 1), kept
// from the horizontal layout this replaced; on screen they read top/bottom.
//
// The two variant names ARE the label — a "COL" / "SLOPE" / "SHAPE" header
// over WHITE/PINK, 12/24, LIN/EXP only repeats what the positions already say.
// The column still lines up with the fader labels beside it; the blank space
// where a header would go is reserved in CSS (`.panel-body > .ctl-rocker`),
// not by an empty element here.
//
// Bools work too (off = left, on = right), taking the cell's `data-label` for
// the right-hand name and `Off` for the left.
export function makeRocker(el, id, desc) {
  const isEnum = desc.kind === 'enum';
  const variants = isEnum ? (desc.variants || []) : ['Off', el.dataset.label || desc.label];
  const left = variants[0] || '';
  const right = variants[1] || '';
  el.classList.add('ctl-rocker');
  el.innerHTML =
    '<div class="ctl-rocker-body">' +
      '<div class="ctl-rocker-lbl ctl-rocker-left">' + left.toUpperCase() + '</div>' +
      '<div class="ctl-rocker-track"><div class="ctl-rocker-knob"></div></div>' +
      '<div class="ctl-rocker-lbl ctl-rocker-right">' + right.toUpperCase() + '</div>' +
    '</div>';
  const leftLbl = el.querySelector('.ctl-rocker-left');
  const rightLbl = el.querySelector('.ctl-rocker-right');

  el.addEventListener('pointerdown', (ev) => {
    ev.preventDefault();
    // Local state is the truth for the flip; the echo reconciles if the engine
    // clamps or refuses (same contract as the switch).
    window.vxn.send.discrete(id, el.classList.contains('right') ? 0 : 1);
  });

  return {
    update(plain) {
      const on = isEnum ? clampVariant(plain, variants) === 1 : plain >= 0.5;
      el.classList.toggle('right', on);
      leftLbl.classList.toggle('active', !on);
      rightLbl.classList.toggle('active', on);
    },
  };
}

// `Dropdown(id, label, variants)` — native <select> fallback. Used when
// the variant list is too long for a row of toggles to fit the cell.
export function makeDropdown(el, id, desc) {
  const label = el.dataset.label || desc.label;
  const variants = desc.variants || [];
  el.classList.add('ctl-dropdown');
  el.innerHTML =
    '<div class="ctl-label">' + label.toUpperCase() + '</div>' +
    '<select></select>';
  const select = el.querySelector('select');
  variants.forEach((v, i) => {
    const opt = document.createElement('option');
    opt.value = String(i);
    opt.textContent = v;
    select.appendChild(opt);
  });
  select.addEventListener('change', () => {
    const i = parseInt(select.value, 10);
    window.vxn.send.discrete(id, i);
  });
  return {
    update(plain) {
      const p = clampVariant(plain, variants);
      select.value = String(p);
    },
  };
}

// ─── Header switch (Chorus / Delay, 0045) ──────────────────────────────────
//
// A small toggle box centred inside a panel header's
// `.panel-header-toggle-slot`. Same wire shape as a plain bool `Switch` —
// gesture-bracketed `set_param` on click; update() flips the `.active`
// class on echo. The box is a child of the slot rather than the slot
// itself so the 16 px slot keeps its layout reservation while the visible
// box stays small enough to sit inside the header bar.
export function makeHeaderSwitch(el, id, _desc) {
  el.innerHTML = '<div class="panel-header-switch"></div>';
  const box = el.querySelector('.panel-header-switch');
  el.addEventListener('pointerdown', (ev) => {
    ev.preventDefault();
    const on = box.classList.contains('active') ? 0 : 1;
    window.vxn.send.discrete(id, on);
  });
  return {
    update(plain) { box.classList.toggle('active', plain >= 0.5); },
  };
}
