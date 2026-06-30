// panels/op-faders.js — per-op tuning-column input widgets (split out of
// op-row.js in ticket 0141).
//
// Two builders, both taking the op-row binding context `b`:
//   makeFader(parent, label, opUnprefixed, b) — one labelled vertical fader
//       bound to `op{n}-{opUnprefixed}`. Returns the wrap (so callers can grey
//       it per tuning mode / carrier role) or null if the param is absent.
//   makeRatioButtonGroup(parent, faders, b)   — the Ratio/Fixed selector for
//       the op, greying the inert faders per mode.
//
// `b` carries: b.op (1-indexed), b.vxn (window.__vxn), b.makeCtxForId, and
// b.register(id, prim, wrap) which both registers the prim with the host echo
// pump and tracks it for teardown on the next op-detail re-render.
(function () {
  window.__vxn = window.__vxn || {};
  window.__vxn.panels = window.__vxn.panels || {};

  function makeFader(parent, label, opUnprefixed, b) {
    const name = "op" + b.op + "-" + opUnprefixed;
    const desc = b.vxn.paramsByName[name];
    if (!desc) return null;
    const wrap = document.createElement("div");
    wrap.className = "fader";
    wrap.setAttribute("data-vxn-param", name);
    wrap.innerHTML =
      '<div class="fader-label">' + label + '</div>' +
      '<div class="fader-track"><div class="fader-track-fill"></div><div class="fader-thumb"></div></div>';
    parent.appendChild(wrap);
    const localCtx = b.makeCtxForId(desc, desc.id);
    const prim = b.vxn.panels.fader.create(wrap, localCtx);
    b.register(desc.id, prim, wrap);
    return wrap;
  }

  // Ratio / Fixed tuning selector for the current op. Bound to the
  // `op{n}-ratio-mode` CLAP enum (0 = Ratio, 1 = Fixed). `faders` carries
  // the wraps to grey per mode: `.ratio` (Hz) inert in Ratio mode,
  // `.fixed` (num/den/fine/cents) inert in Fixed mode.
  function makeRatioButtonGroup(parent, faders, b) {
    const name = "op" + b.op + "-ratio-mode";
    const desc = b.vxn.paramsByName[name];
    const cgrp = document.createElement("div");
    cgrp.className = "op-tuning-mode";
    cgrp.innerHTML =
      '<div class="bgrp"><div class="bgrp-row op-tuning-mode-row">' +
      '<button class="bgrp-btn" data-op-tuning="0">Ratio</button>' +
      '<button class="bgrp-btn" data-op-tuning="1">Fixed</button>' +
      '</div></div>';
    parent.appendChild(cgrp);
    const btns = cgrp.querySelectorAll("[data-op-tuning]");

    function apply(modeIdx) {
      for (let i = 0; i < btns.length; i++) {
        const idx = parseInt(btns[i].getAttribute("data-op-tuning"), 10);
        btns[i].classList.toggle("active", idx === modeIdx);
      }
      const fixed = modeIdx === 1;
      for (let i = 0; i < faders.ratio.length; i++) {
        if (faders.ratio[i]) faders.ratio[i].classList.toggle("disabled", !fixed);
      }
      for (let i = 0; i < faders.fixed.length; i++) {
        if (faders.fixed[i]) faders.fixed[i].classList.toggle("disabled", fixed);
      }
    }

    if (!desc) { apply(0); return; }
    const localCtx = b.makeCtxForId(desc, desc.id);
    for (let i = 0; i < btns.length; i++) {
      const btn = btns[i];
      btn.addEventListener("click", function (ev) {
        ev.preventDefault();
        const idx = parseInt(btn.getAttribute("data-op-tuning"), 10);
        localCtx.setParam(idx);
        apply(idx); // optimistic; host echo confirms via the registered prim
      });
    }
    const prim = { set: function (plain) { apply(Math.round(plain) === 1 ? 1 : 0); } };
    b.register(desc.id, prim, cgrp);
    apply(Math.round(desc.default) === 1 ? 1 : 0);
  }

  window.__vxn.panels.opFaders = {
    makeFader: makeFader,
    makeRatioButtonGroup: makeRatioButtonGroup,
  };
})();
