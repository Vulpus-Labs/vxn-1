// panels/fx-tabs.js — FX tab-strip wiring (E025 / 0090).
//
// Ported from VXN-1's `wireFxTabs` (vxn-ui-web/assets/panels.js). Pure DOM:
// click a `.fx-tab` button → set the parent `.fx-panel`'s `data-active-tab`
// and toggle the `.active` class on the buttons. CSS does the visibility — the
// panel's `[data-active-tab="…"] .fx-pane-…` selectors pick which pane shows.
// Nothing here touches params: every per-tab on/off switch (`.fx-tab-switch`,
// a `.bgrp-toggle`) and every fader inside a pane is bound normally by main.js,
// and the inactive tabs' controls stay live (just hidden) so DAW automation
// still echoes them.
//
// `repaint(pane, name)` is an optional callback invoked when a pane becomes
// active. VXN-2 faders position from a cached norm (percentage-based, so they
// paint correctly even while hidden), but re-applying the cached value on show
// keeps the contract explicit and matches VXN-1's repaint-on-reveal behaviour.
(function () {
  window.__vxn = window.__vxn || {};

  function wireFxTabs(root, repaint) {
    const scope = root || document;
    const panels = scope.querySelectorAll(".fx-panel");
    panels.forEach(function (panel) {
      const buttons = Array.prototype.slice.call(
        panel.querySelectorAll(".fx-tab"),
      );
      if (buttons.length === 0) return;

      const setActive = function (name) {
        panel.dataset.activeTab = name;
        for (let i = 0; i < buttons.length; i++) {
          buttons[i].classList.toggle("active", buttons[i].dataset.tab === name);
        }
        const pane = panel.querySelector(".fx-pane-" + name);
        if (pane && typeof repaint === "function") repaint(pane, name);
      };

      for (let i = 0; i < buttons.length; i++) {
        buttons[i].addEventListener("click", function (ev) {
          // The per-tab on/off switch (`.fx-tab-switch`) fires its own click
          // (toggles the param) and the event bubbles here, so flipping an
          // effect on/off also brings its pane into view.
          ev.preventDefault();
          setActive(this.dataset.tab);
        });
      }

      // Seed the active tab from whatever `data-active-tab` was authored into
      // the HTML (phaser by default per index.html), falling back to the first
      // tab.
      setActive(panel.dataset.activeTab || buttons[0].dataset.tab);
    });
  }

  window.__vxn.wireFxTabs = wireFxTabs;
})();
