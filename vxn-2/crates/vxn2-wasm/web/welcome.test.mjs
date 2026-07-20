// First-run welcome card (task: vxn-2 browser splash). Run: node --test
//
// The card is pure DOM, so a tiny createElement stub (no jsdom) is enough to
// exercise structure, the product-page link, and dismissal.

import { test } from "node:test";
import assert from "node:assert/strict";
import { createWelcome } from "./faceplate-bridge.mjs";

function makeEl() {
  return {
    style: { cssText: "" },
    children: [],
    append(...c) { this.children.push(...c); },
    appendChild(c) { this.children.push(c); return c; },
    addEventListener() {},
    removeEventListener() {},
    remove() { this._removed = true; },
    set textContent(v) { this._t = v; },
    get textContent() { return this._t; },
  };
}

function fakeDoc() {
  const created = [];
  return {
    _created: created,
    _byId: {},
    getElementById(id) { return this._byId[id] || null; },
    createElement() { const el = makeEl(); created.push(el); return el; },
    createTextNode(text) { return { text }; },
    addEventListener() {},
    removeEventListener() {},
    body: makeEl(),
  };
}

test("returns a no-op card with no document", () => {
  const w = createWelcome(null);
  assert.equal(w.el, null);
  assert.doesNotThrow(() => w.close());
});

test("mounts a VXN-2 card linking to the product page", () => {
  const doc = fakeDoc();
  const w = createWelcome(doc);
  assert.ok(w.el, "card mounted");
  assert.equal(doc.body.children.length, 1, "appended to body");

  const heading = doc._created.find((e) => e.textContent === "VXN-2");
  assert.ok(heading, "heading shows VXN-2");

  const link = doc._created.find((e) => e.href === "https://vulpuslabs.com/products/vxn-2/");
  assert.ok(link, "links to the vxn-2 product page");
  assert.equal(link.target, "_blank", "opens in a new tab");
  assert.equal(link.rel, "noopener noreferrer", "safe rel on the new-tab link");
});

test("close() removes the backdrop", () => {
  const doc = fakeDoc();
  const w = createWelcome(doc);
  w.close();
  assert.equal(w.el._removed, true);
});

test("idempotent: a second call with the same id reuses the card", () => {
  const doc = fakeDoc();
  const first = createWelcome(doc);
  doc._byId["vxn-welcome"] = first.el; // simulate the element now being in the DOM
  const second = createWelcome(doc);
  assert.equal(second.el, first.el);
  assert.equal(doc.body.children.length, 1, "not appended twice");
});
