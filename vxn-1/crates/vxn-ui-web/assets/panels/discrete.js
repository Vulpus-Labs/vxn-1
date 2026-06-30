// panels/discrete.js — the click-to-pick widgets (Switch / ButtonGroup /
// Dropdown / HeaderSwitch) and the FX tab strip. Split out of the panels.js
// god-file in ticket 0141.
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

// ─── FX panel tabs (E018 / 0098) ──────────────────────────────────────────
//
// Pure DOM wiring: click a `.fx-tab` button → set the parent panel's
// `data-active-tab` and toggle the `.active` class on the buttons. CSS does
// the visibility — the panel's `data-active-tab="…"` attribute selectors
// pick which `.fx-pane-…` and `.fx-header-…` show. Nothing here touches
// params or the controls table; every header-switch / fader inside the FX
// panel is bound normally by `dispatch.bindCell`, and the inactive tabs'
// primitives stay live (just hidden) so DAW automation still echoes them.
export function wireFxTabs() {
  document.querySelectorAll('[data-name="FX"]').forEach((panel) => {
    const buttons = Array.from(panel.querySelectorAll('.fx-tab'));
    if (buttons.length === 0) return;

    const setActive = (name) => {
      panel.dataset.activeTab = name;
      for (const b of buttons) {
        b.classList.toggle('active', b.dataset.tab === name);
      }
      // Re-paint faders in the newly-visible pane. While the pane was
      // `display: none`, `paintFader` saw `fader.clientHeight = 0` and
      // pinned every thumb to the top. The cached `--fader-norm` CSS var
      // still holds the correct value from that earlier echo, so re-running
      // `paintFader` with the now-real height puts thumbs back where they
      // belong. (Other primitives — switches, the per-tab header switch —
      // don't depend on element height, so they don't need this dance.)
      const pane = panel.querySelector(`.fx-pane-${name}`);
      if (pane) {
        pane.querySelectorAll('.ctl-fader').forEach((fader) => {
          const thumb = fader.querySelector('.ctl-fader-thumb');
          if (!thumb) return;
          const n = parseFloat(
            getComputedStyle(fader).getPropertyValue('--fader-norm'),
          );
          if (!Number.isNaN(n)) paintFader(fader, thumb, n);
        });
      }
    };

    for (const btn of buttons) {
      btn.addEventListener('click', (ev) => {
        // Each tab hosts its own on/off switch (a `.fx-tab-switch`
        // header-switch primitive). The switch fires on `pointerdown`
        // and toggles the param; the bubbled `click` then swaps the
        // active pane so flipping an effect on/off also brings its
        // controls into view.
        ev.preventDefault();
        setActive(btn.dataset.tab);
      });
    }
    // Seed the active class from whatever `data-active-tab` was authored
    // into the HTML (phaser by default per faceplate.html).
    setActive(panel.dataset.activeTab || buttons[0].dataset.tab);
  });
}
