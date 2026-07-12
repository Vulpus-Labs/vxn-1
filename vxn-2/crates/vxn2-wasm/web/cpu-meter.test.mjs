// CPU render-load meter (copied from vxn-1, ticket follow-up). Run: node --test
//
// Verifies the load→bar-width / colour-band / percent-text mapping via a minimal
// fake DOM (no jsdom): the meter is pure DOM, so a tiny createElement stub is
// enough to exercise update().

import { test } from "node:test";
import assert from "node:assert/strict";
import { createCpuMeter } from "./faceplate-bridge.mjs";

function fakeDoc() {
  const make = () => ({
    style: { cssText: "", set width(v) { this._w = v; }, get width() { return this._w; },
      set background(v) { this._bg = v; }, get background() { return this._bg; } },
    children: [],
    append(...c) { this.children.push(...c); },
    appendChild(c) { this.children.push(c); return c; },
    set textContent(v) { this._t = v; },
    get textContent() { return this._t; },
  });
  return {
    _byId: {},
    getElementById(id) { return this._byId[id] || null; },
    createElement() { return make(); },
    body: make(),
  };
}

test("returns a no-op meter with no document", () => {
  const m = createCpuMeter(null);
  assert.equal(m.el, null);
  assert.doesNotThrow(() => m.update(0.5, 0.6));
});

test("null load shows n/a and empties the bar", () => {
  const m = createCpuMeter(fakeDoc());
  m.update(null, null);
  assert.equal(m.el._pct.textContent, "n/a");
  assert.equal(m.el._fill.style.width, "0%");
});

test("colour bands: green < .7, amber < .9, red beyond", () => {
  const m = createCpuMeter(fakeDoc());
  m.update(0.4, 0.4);
  assert.equal(m.el._fill.style.background, "#46c46e"); // green
  m.update(0.8, 0.8);
  assert.equal(m.el._fill.style.background, "#e0b341"); // amber
  m.update(1.1, 1.1);
  assert.equal(m.el._fill.style.background, "#e0564b"); // red
});

test("bar follows peak; percent shows the mean (one decimal under 10%)", () => {
  const m = createCpuMeter(fakeDoc());
  m.update(0.03, 0.5); // low mean, higher peak
  assert.equal(m.el._fill.style.width, "50%", "bar tracks peak");
  assert.equal(m.el._pct.textContent, "3.0%", "percent tracks mean, one decimal under 10%");
  m.update(0.42, 0.42);
  assert.equal(m.el._pct.textContent, "42%", "no decimal at/above 10%");
});
