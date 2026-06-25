import { describe, it, expect, beforeEach, vi } from "vitest";

// `panels/fx-tabs.js` is an IIFE that attaches `window.__vxn.wireFxTabs`.
// Importing it for its side effect mirrors how the production bundle loads it.
import "../panels/fx-tabs.js";

const { wireFxTabs } = window.__vxn;

// VXN-2 FX panel: four tabs (signal order Dyn / Phaser / Delay / Reverb), each
// with an inline on/off switch (`.fx-tab-switch`, a `.bgrp-toggle` bound to
// `dyn-on` / `phaser-on` / `delay-on` / `reverb-on` in production) and a
// label. Panes are CSS-gated off the panel's `data-active-tab`. Dyn was added
// in E028 / 0148; the default `data-active-tab` stays `phaser` so opening a
// saved patch doesn't surface the new tab.
function buildFxPanel() {
  document.body.innerHTML = `
    <div class="panel fx-panel" data-vxn-section="fx" data-active-tab="phaser">
      <div class="panel-header">FX</div>
      <div class="panel-body fx-body">
        <div class="fx-tabs">
          <button type="button" class="fx-tab fx-tab-dyn" data-tab="dyn">
            <span class="fx-tab-switch bgrp-toggle" data-vxn-param="dyn-on"></span>
            <span class="fx-tab-label">DYN</span>
          </button>
          <button type="button" class="fx-tab fx-tab-phaser" data-tab="phaser">
            <span class="fx-tab-switch bgrp-toggle" data-vxn-param="phaser-on"></span>
            <span class="fx-tab-label">PHASER</span>
          </button>
          <button type="button" class="fx-tab fx-tab-delay" data-tab="delay">
            <span class="fx-tab-switch bgrp-toggle" data-vxn-param="delay-on"></span>
            <span class="fx-tab-label">DELAY</span>
          </button>
          <button type="button" class="fx-tab fx-tab-reverb" data-tab="reverb">
            <span class="fx-tab-switch bgrp-toggle" data-vxn-param="reverb-on"></span>
            <span class="fx-tab-label">REVERB</span>
          </button>
        </div>
        <div class="fx-panes">
          <div class="fx-pane fx-pane-dyn"></div>
          <div class="fx-pane fx-pane-phaser"></div>
          <div class="fx-pane fx-pane-delay"></div>
          <div class="fx-pane fx-pane-reverb"></div>
        </div>
      </div>
    </div>
  `;
}

describe("wireFxTabs (E025 / 0090, E028 / 0148)", () => {
  beforeEach(() => {
    buildFxPanel();
    wireFxTabs(document);
  });

  it("renders four tabs in signal order", () => {
    const tabs = [...document.querySelectorAll(".fx-tab")].map(
      (t) => t.dataset.tab,
    );
    expect(tabs).toEqual(["dyn", "phaser", "delay", "reverb"]);
  });

  it("seeds the authored data-active-tab and marks that button .active", () => {
    const panel = document.querySelector(".fx-panel");
    expect(panel.dataset.activeTab).toBe("phaser");
    const phaser = panel.querySelector('.fx-tab[data-tab="phaser"]');
    expect(phaser.classList.contains("active")).toBe(true);
  });

  it("can swap into the dyn pane and back", () => {
    const panel = document.querySelector(".fx-panel");
    const dynBtn = panel.querySelector('.fx-tab[data-tab="dyn"]');
    dynBtn.click();
    expect(panel.dataset.activeTab).toBe("dyn");
    expect(dynBtn.classList.contains("active")).toBe(true);
    panel.querySelector('.fx-tab[data-tab="phaser"]').click();
    expect(panel.dataset.activeTab).toBe("phaser");
    expect(dynBtn.classList.contains("active")).toBe(false);
  });

  it("dyn tab switch is wired to dyn-on", () => {
    const sw = document.querySelector(
      '.fx-tab[data-tab="dyn"] .fx-tab-switch',
    );
    expect(sw.dataset.vxnParam).toBe("dyn-on");
  });

  it("swaps data-active-tab and the .active class on click", () => {
    const panel = document.querySelector(".fx-panel");
    const delayBtn = panel.querySelector('.fx-tab[data-tab="delay"]');
    delayBtn.click();

    expect(panel.dataset.activeTab).toBe("delay");
    expect(delayBtn.classList.contains("active")).toBe(true);
    const phaser = panel.querySelector('.fx-tab[data-tab="phaser"]');
    expect(phaser.classList.contains("active")).toBe(false);
  });

  it("only one tab carries .active at any time", () => {
    const panel = document.querySelector(".fx-panel");
    for (const t of ["reverb", "phaser", "delay", "dyn"]) {
      panel.querySelector(`.fx-tab[data-tab="${t}"]`).click();
      const active = panel.querySelectorAll(".fx-tab.active");
      expect(active.length).toBe(1);
      expect(active[0].dataset.tab).toBe(t);
    }
  });

  it("CSS attribute selectors gate the panes via data-active-tab", () => {
    // wireFxTabs only sets the attribute — visibility is CSS-driven. Confirm
    // the contract by reading the attribute back: the
    // `.fx-panel[data-active-tab="X"] .fx-pane-X` selector would match.
    const panel = document.querySelector(".fx-panel");
    panel.querySelector('.fx-tab[data-tab="reverb"]').click();
    expect(panel.matches('[data-active-tab="reverb"]')).toBe(true);
    expect(panel.matches('[data-active-tab="phaser"]')).toBe(false);
  });

  it("click on the per-tab switch also swaps to that tab", () => {
    // Toggling an effect on/off also brings its pane into view: the
    // header-switch click bubbles to the tab button, which calls setActive.
    const panel = document.querySelector(".fx-panel");
    expect(panel.dataset.activeTab).toBe("phaser");
    const reverbSwitch = panel.querySelector(
      '.fx-tab[data-tab="reverb"] .fx-tab-switch',
    );
    reverbSwitch.click();
    expect(panel.dataset.activeTab).toBe("reverb");
  });

  it("invokes the repaint callback with the newly-active pane", () => {
    // VXN-2 faders position from a cached norm, so reveal repaint is a safety
    // re-apply; wireFxTabs must still call it with the now-visible pane so
    // main.js can re-push the cached value.
    buildFxPanel();
    const repaint = vi.fn();
    wireFxTabs(document, repaint);
    // Seed call fires for the authored (phaser) pane.
    expect(repaint).toHaveBeenCalledTimes(1);
    expect(repaint.mock.calls[0][1]).toBe("phaser");

    const panel = document.querySelector(".fx-panel");
    panel.querySelector('.fx-tab[data-tab="delay"]').click();
    expect(repaint).toHaveBeenCalledTimes(2);
    const lastCall = repaint.mock.calls[1];
    expect(lastCall[1]).toBe("delay");
    expect(lastCall[0]).toBe(panel.querySelector(".fx-pane-delay"));
  });
});
