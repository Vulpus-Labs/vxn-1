// util/drag.js — generic drag / paint / value-popup primitives, plus the two
// tiny shared control helpers (`clampVariant`, `tgRow`). Consumes the shared
// `wireDrag` (0140).
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

/// The shared **relative-norm** drag: a delta-mapped `[0, 1]` value with
/// gesture brackets, the hover/drag value popup, and a clamped write that
/// paints locally and posts the norm.
///
/// The rotary dial and the bipolar depth fader carried a copy each before
/// 0319 — same `writeFromDrag` clamp, same `attachValuePop`
/// forward-declaration dance, same `wireDrag({raf: true, downContext})` call,
/// same gesture brackets — differing only in axis, sign and travel. (The
/// vertical faders are *absolute*-mapped over their own height by
/// `wireFaderDrag`, which is why they are not this.)
///
/// - `axis` — `'y'` (up raises, i.e. a negative `dy`) or `'x'` (right raises).
/// - `rangePx` — pointer travel for the full 0..1 span.
/// - `paint(norm)` — the caller's local repaint; also called on grab to
///   re-anchor at the pointer.
/// - `getNorm()` / `getLabel()` — the caller's current norm and popup text.
///
/// Returns the value popup, which the caller's `update` refreshes on an echo.
export function wireNormDrag(el, id, { axis, rangePx, paint, getNorm, getLabel }) {
  const write = (rawNorm) => {
    const n = rawNorm < 0 ? 0 : rawNorm > 1 ? 1 : rawNorm;
    paint(n);
    window.vxn.send.setParamNorm(id, n);
  };
  // `drag` is forward-declared because the popup's host reads the drag's
  // hover/drag getters, and `wireDrag` needs the popup's callbacks.
  let drag;
  const pop = attachValuePop({
    isHovered:  () => drag.isHovered(),
    isDragging: () => drag.isDragging(),
  }, getLabel);
  drag = wireDrag(el, {
    axis,
    raf: true,
    downContext: () => ({ startNorm: getNorm() }),
  }, {
    onEnter: (ev) => pop.markEntered(ev),
    onLeave: () => pop.markLeft(),
    onDown: (ev) => {
      window.vxn.send.beginGesture(id);
      write(getNorm()); // re-anchor at the grab point
      pop.markGrabbed(ev);
    },
    onMove: (_ev, info) =>
      write(info.ctx.startNorm + (axis === 'x' ? info.dx : -info.dy) / rangePx),
    onUp: () => {
      window.vxn.send.endGesture(id);
      pop.markReleased();
    },
  });
  return pop;
}

// Paint a vertical fader's thumb at a [0, 1] norm. Norm 0 = bottom, 1 = top.
// Pins in pixel space against the live element height so the thumb's
// bounding box stays inside `.ctl-fader` exactly at both ends regardless of
// `--fader-h` / `--thumb-h` tweaks. Also sets `--fader-norm` for dependent
// CSS (track fill colour, etc).
export function paintFader(fader, thumb, norm) {
  const n = Math.max(0, Math.min(1, norm));
  // The lit fill is percentage-driven off this custom property, so it is
  // correct whether or not the element has been laid out. Always set it.
  fader.style.setProperty('--fader-norm', n);

  // The thumb, by contrast, is positioned in PIXELS, which needs real layout.
  // Inside a `display: none` container (an inactive tab pane, a closed
  // overlay) both measurements read 0, and `top: 0px` pins the thumb to the
  // TOP of the track — i.e. it reads as full scale while the fill underneath
  // shows the true value. Leave the thumb alone in that case; whoever reveals
  // the container repaints from the cached value (`repaintAllControls`).
  const travel = fader.clientHeight - thumb.offsetHeight;
  if (travel <= 0) return;
  thumb.style.top = (thumb.offsetHeight / 2 + (1 - n) * travel) + 'px';
}

// Plain → variant index clamp. Round to nearest, clamp to [0, len - 1].
// The four enum-shaped primitives (Switch, ButtonGroup, Dropdown, Wave-
// knob drag) all need exactly this.
export function clampVariant(plain, variants) {
  return Math.max(0, Math.min(variants.length - 1, Math.round(plain)));
}

// `tgRow(name)` returns a fresh `.ctl-tg-row` containing the box + label
// pair. `tgRow(name, { mount })` instead fills the supplied target and returns
// it, leaving the caller's classes alone — for a container that is already
// classed and only wants the inner markup. No production caller takes the mount
// form today; the option and its suite stay because the next composite cell
// will want it and it is three lines.
export function tgRow(name, opts) {
  const target = (opts && opts.mount) || document.createElement('div');
  if (!opts || !opts.mount) target.className = 'ctl-tg-row';
  target.innerHTML =
    '<div class="ctl-tg-box"></div>' +
    '<div class="ctl-tg-lbl">' + name.toUpperCase() + '</div>';
  return target;
}
