// util/drag.js — generic drag / paint / value-popup primitives, plus the two
// tiny shared control helpers (`clampVariant`, `tgRow`). Split out of the
// panels.js god-file in ticket 0141; consumes the shared `wireDrag` (0140).
//
// The splice loader drops these `import` lines for the inline `<script>` (the
// stripped shared bindings are spliced ahead of this module via the bridge
// slot, so `valuePop` / `wireDrag` are already in scope); under Node ESM the
// bindings resolve through the shared modules so the suites can exercise the
// helpers. The panels.js barrel re-exports everything here so tests that pull
// these from `../panels.js` keep working.
import { valuePop } from '../../../../../crates/vxn-core-ui-web/assets/value-pop.js';
import { wireDrag } from '../../../../../crates/vxn-core-ui-web/assets/wire-drag.js';

// One detent = one variant step. The drag sensitivity: pixels of vertical
// pointer travel per detent. ~30 feels close to hardware knobs.
export const PIXELS_PER_DETENT = 30;

// Smoothing transition on the wave-knob indicator. Long enough that
// automation moves don't strobe between detents; short enough that drag
// still feels responsive.
export const KNOB_INDICATOR_TRANSITION_MS = 120;

// Thin wrapper: the fader-shaped controls (Fader, DetuneLegato) all want
// the same vertical [0, 1] norm.
export function wireFaderDrag(fader, callbacks) {
  const pointerToValue = (ev) => {
    const r = fader.getBoundingClientRect();
    return Math.max(0, Math.min(1, 1 - (ev.clientY - r.top) / r.height));
  };
  return wireDrag(fader, { pointerToValue }, callbacks);
}

// Attaches the floating value popup's lifecycle to a control. `getLabel()`
// returns the current display string. The host control invokes the
// `markX` methods from its drag callbacks; `refresh()` runs on the
// ParamChanged echo. `host` is any object with `isHovered()` and
// `isDragging()` getters (the `wireFaderDrag` return value, or a shim
// over makeWave's local vars).
export function attachValuePop(host, getLabel) {
  return {
    markEntered(ev) {
      if (host.isDragging()) return;
      valuePop.show(getLabel(), ev.clientX, ev.clientY);
    },
    markLeft() {
      if (!host.isDragging()) valuePop.hide();
    },
    markGrabbed(ev) {
      valuePop.show(getLabel(), ev.clientX, ev.clientY);
    },
    markReleased() {
      if (!host.isHovered()) valuePop.hide();
    },
    refresh() {
      if (host.isHovered() || host.isDragging()) {
        valuePop.update(getLabel());
      }
    },
  };
}

// Paint a vertical fader's thumb at a [0, 1] norm. Norm 0 = bottom, 1 = top.
// Pins in pixel space against the live element height so the thumb's
// bounding box stays inside `.ctl-fader` exactly at both ends regardless of
// `--fader-h` / `--thumb-h` tweaks. Also sets `--fader-norm` for dependent
// CSS (track fill colour, etc).
export function paintFader(fader, thumb, norm) {
  const halfThumb = thumb.offsetHeight / 2;
  const travel = fader.clientHeight - thumb.offsetHeight;
  const n = Math.max(0, Math.min(1, norm));
  thumb.style.top = (halfThumb + (1 - n) * travel) + 'px';
  fader.style.setProperty('--fader-norm', n);
}

// Plain → variant index clamp. Round to nearest, clamp to [0, len - 1].
// The four enum-shaped primitives (Switch, ButtonGroup, Dropdown, Wave-
// knob drag) all need exactly this.
export function clampVariant(plain, variants) {
  return Math.max(0, Math.min(variants.length - 1, Math.round(plain)));
}

// `tgRow(name)` returns a fresh `.ctl-tg-row` containing the box + label
// pair. `tgRow(name, { mount })` instead fills the supplied target and
// returns it — used by composites whose container is already classed
// (`.ctl-detune-legato.ctl-tg-row`) and need to drop the same inner markup
// in place.
export function tgRow(name, opts) {
  const target = (opts && opts.mount) || document.createElement('div');
  if (!opts || !opts.mount) target.className = 'ctl-tg-row';
  target.innerHTML =
    '<div class="ctl-tg-box"></div>' +
    '<div class="ctl-tg-lbl">' + name.toUpperCase() + '</div>';
  return target;
}
