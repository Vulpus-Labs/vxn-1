import { describe, it, expect, beforeEach } from 'vitest';
import { wireFxTabs } from '../panels.js';

function buildFxPanel() {
  document.body.innerHTML = `
    <div class="panel" data-name="FX" data-active-tab="phaser">
      <div class="panel-body fx-body">
        <div class="fx-tabs">
          <button type="button" class="fx-tab fx-tab-phaser" data-tab="phaser">
            <span class="panel-header-toggle-slot fx-tab-switch" data-control="header-switch" data-param="phaser_on"></span>
            <span class="fx-tab-label">PHASER</span>
          </button>
          <button type="button" class="fx-tab fx-tab-chorus" data-tab="chorus">
            <span class="panel-header-toggle-slot fx-tab-switch" data-control="header-switch" data-param="chorus_on"></span>
            <span class="fx-tab-label">CHORUS</span>
          </button>
          <button type="button" class="fx-tab fx-tab-delay" data-tab="delay">
            <span class="panel-header-toggle-slot fx-tab-switch" data-control="header-switch" data-param="delay_on"></span>
            <span class="fx-tab-label">DELAY</span>
          </button>
          <button type="button" class="fx-tab fx-tab-reverb" data-tab="reverb">
            <span class="panel-header-toggle-slot fx-tab-switch" data-control="header-switch" data-param="reverb_on"></span>
            <span class="fx-tab-label">REVERB</span>
          </button>
        </div>
      </div>
    </div>
  `;
}

describe('wireFxTabs (E018 / 0098)', () => {
  beforeEach(() => {
    buildFxPanel();
    wireFxTabs();
  });

  it('seeds the authored data-active-tab and marks that button .active', () => {
    const panel = document.querySelector('.panel[data-name="FX"]');
    expect(panel.dataset.activeTab).toBe('phaser');
    const phaser = panel.querySelector('.fx-tab[data-tab="phaser"]');
    expect(phaser.classList.contains('active')).toBe(true);
  });

  it('swaps data-active-tab and the .active class on click', () => {
    const panel = document.querySelector('.panel[data-name="FX"]');
    const delayBtn = panel.querySelector('.fx-tab[data-tab="delay"]');
    delayBtn.click();

    expect(panel.dataset.activeTab).toBe('delay');
    expect(delayBtn.classList.contains('active')).toBe(true);
    // Previously-active button no longer carries .active.
    const phaser = panel.querySelector('.fx-tab[data-tab="phaser"]');
    expect(phaser.classList.contains('active')).toBe(false);
  });

  it('only one tab carries .active at any time', () => {
    const panel = document.querySelector('.panel[data-name="FX"]');
    for (const t of ['chorus', 'reverb', 'phaser', 'delay']) {
      panel.querySelector(`.fx-tab[data-tab="${t}"]`).click();
      const active = panel.querySelectorAll('.fx-tab.active');
      expect(active.length).toBe(1);
      expect(active[0].dataset.tab).toBe(t);
    }
  });

  it('CSS attribute selectors gate the panes via data-active-tab', () => {
    // wireFxTabs only sets the attribute — visibility is CSS-driven. Confirm
    // the contract by reading the attribute back and reasoning about which
    // `.panel[data-active-tab="X"] .fx-pane-X` selector would match.
    const panel = document.querySelector('.panel[data-name="FX"]');
    panel.querySelector('.fx-tab[data-tab="reverb"]').click();
    expect(panel.matches('[data-active-tab="reverb"]')).toBe(true);
    expect(panel.matches('[data-active-tab="phaser"]')).toBe(false);
  });

  it('click on the per-tab switch does NOT swap the active tab', () => {
    // The on/off switch lives inside each tab button (revised 0098). The
    // tab-click handler must bail when the event originated inside the
    // switch slot so the param toggle doesn't double-fire a pane swap.
    const panel = document.querySelector('.panel[data-name="FX"]');
    expect(panel.dataset.activeTab).toBe('phaser');
    const reverbSwitch = panel.querySelector(
      '.fx-tab[data-tab="reverb"] .fx-tab-switch',
    );
    reverbSwitch.click();
    expect(panel.dataset.activeTab).toBe('phaser');
  });
});
