// VXN2 button-group primitive — discrete enum / bool select.
//
// Each `.bgrp-row[data-vxn-param]` carries one param; child
// `.bgrp-btn[data-vxn-value]` buttons set that param's plain value. One
// discrete edit per click — no gesture brackets (matches
// `set_param` opcode in the shared vocabulary).
//
// Also handles `.panel-header.toggleable[data-vxn-param]` (delay-on /
// reverb-on) as a one-button bool toggle: click flips 0 ↔ 1 and updates
// the panel's toggle-on / toggle-off class to reflect state.

(function () {
  function setActive(rowEl, plain) {
    const btns = rowEl.querySelectorAll(".bgrp-btn");
    for (let i = 0; i < btns.length; i++) {
      const v = parseFloat(btns[i].getAttribute("data-vxn-value"));
      btns[i].classList.toggle("active", Math.abs(v - plain) < 0.5);
    }
  }

  function createRow(rowEl, ctx) {
    let currentPlain = ctx.desc ? ctx.desc.default : 0;
    setActive(rowEl, currentPlain);

    const btns = rowEl.querySelectorAll(".bgrp-btn");
    for (let i = 0; i < btns.length; i++) {
      const btn = btns[i];
      btn.addEventListener("click", function (ev) {
        ev.preventDefault();
        const v = parseFloat(btn.getAttribute("data-vxn-value"));
        if (isNaN(v) || v === currentPlain) return;
        ctx.setParam(v);
        currentPlain = v;
        setActive(rowEl, v);
      });
    }

    return {
      set: function (plain) {
        currentPlain = plain;
        setActive(rowEl, plain);
      },
    };
  }

  // Single-button bool toggle (rate-fader Sync sub-button).
  // Click flips 0 ↔ 1 on the button's `data-vxn-param`. Active state
  // paints via the same `.active` class as bgrp-btn.
  function createBoolToggle(btnEl, ctx) {
    let currentPlain = ctx.desc ? ctx.desc.default : 0;
    function paint() {
      btnEl.classList.toggle("active", currentPlain >= 0.5);
    }
    paint();
    btnEl.addEventListener("click", function (ev) {
      ev.preventDefault();
      const next = currentPlain >= 0.5 ? 0 : 1;
      ctx.setParam(next);
      currentPlain = next;
      paint();
    });
    return {
      set: function (plain) {
        currentPlain = plain;
        paint();
      },
    };
  }

  function createToggleHeader(headerEl, ctx) {
    const panel = headerEl.closest(".panel");
    let currentPlain = ctx.desc ? ctx.desc.default : 0;

    function paint() {
      const on = currentPlain >= 0.5;
      if (panel) {
        panel.classList.toggle("toggle-on", on);
        panel.classList.toggle("toggle-off", !on);
      }
    }
    paint();

    headerEl.addEventListener("click", function (ev) {
      ev.preventDefault();
      const next = currentPlain >= 0.5 ? 0 : 1;
      ctx.setParam(next);
      currentPlain = next;
      paint();
    });

    return {
      set: function (plain) {
        currentPlain = plain;
        paint();
      },
    };
  }

  window.__vxn.panels.buttonGroup = {
    createRow: createRow,
    createBoolToggle: createBoolToggle,
    createToggleHeader: createToggleHeader,
  };
})();
