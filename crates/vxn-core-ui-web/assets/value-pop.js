// Floating value popup — one shared <div> across every control, anchored
// at the pointer's first relevant position (entry for hover, grab for
// drag) so the popup stays put while the indicator moves. `fixed`
// positioning + body-level mount means it can't push any layout around
// or be clipped by a panel's overflow.
//
// Shared primitive (0140): both faceplates re-implemented this verbatim —
// VXN1's `bridge.js` `valuePop` and VXN2's `fader.js`
// `ensurePop/showPop/updatePop/hidePop`. Lifted here so there is one
// singleton + one `.value-pop` CSS ruleset (see `value-pop.css`).
//
// Authored as an ES module so the Node/vitest suites can pull `valuePop` in
// directly; the ESM marker is stripped at splice time (the synths concat every
// module into one inline `<script>` where module syntax is illegal) — see
// `strip_esm_exports` in the crate's `lib.rs`. The singleton's <div> is
// created lazily on first `show` so importing the module (e.g. only for the
// cutoff helpers) doesn't touch the DOM until a control actually needs it.
export const valuePop = (() => {
  let el = null;
  function ensure() {
    if (el) return el;
    el = document.createElement('div');
    el.className = 'value-pop';
    document.body.appendChild(el);
    return el;
  }
  return {
    show(text, clientX, clientY) {
      const e = ensure();
      e.textContent = text;
      e.style.left = (clientX + 12) + 'px';
      e.style.top  = (clientY - 8)  + 'px';
      e.style.display = 'block';
    },
    update(text) { if (el) el.textContent = text; },
    hide() { if (el) el.style.display = 'none'; },
  };
})();
