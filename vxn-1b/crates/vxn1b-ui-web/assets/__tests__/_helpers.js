import { vi } from 'vitest';

// Shared test helpers for the vxn-ui-web suites. The `_` prefix keeps
// vitest's default `*.test.js` discovery from picking this up; the include
// pattern in `vitest.config.js` also enforces it.

export function pointerEvt(type, { clientX = 0, clientY = 0, pointerId = 7 } = {}) {
  // jsdom doesn't ship `PointerEvent`; build a MouseEvent and graft the
  // pointer fields. Helpers under test only read `clientX` / `clientY` /
  // `pointerId`.
  const ev = new MouseEvent(type, { bubbles: true, cancelable: true });
  Object.defineProperty(ev, 'pointerId', { value: pointerId });
  Object.defineProperty(ev, 'clientX', { value: clientX });
  Object.defineProperty(ev, 'clientY', { value: clientY });
  return ev;
}

export function mountEl() {
  const el = document.createElement('div');
  document.body.appendChild(el);
  el.setPointerCapture = vi.fn();
  el.releasePointerCapture = vi.fn();
  return el;
}

export function mountFader({ top = 100, height = 200 } = {}) {
  const el = mountEl();
  vi.spyOn(el, 'getBoundingClientRect').mockReturnValue({
    top,
    height,
    left: 0,
    right: 0,
    bottom: top + height,
    width: 0,
    x: 0,
    y: top,
    toJSON() {},
  });
  return el;
}

export function browserDOM() {
  return `
    <div id="faceplate">
      <div id="browser-panel" hidden>
        <div id="browser-folders"></div>
        <div id="browser-presets"></div>
        <input id="browser-search-input" type="text" />
        <button id="browser-search-clear" type="button"></button>
        <button id="browser-close" type="button"></button>
      </div>
      <div id="browser-backdrop" hidden></div>
    </div>
  `;
}

export function installVxn(opcodes, { promptValue = null } = {}) {
  const sendCalls = [];
  const send = {};
  for (const op of opcodes) {
    send[op] = (...args) => sendCalls.push([op, ...args]);
  }
  globalThis.window.vxn = {
    send,
    promptText: (_title, _initial, cb) => cb(promptValue),
  };
  return { send, sendCalls };
}

// The browser logic now lives in the shared crate; `browser.js` is just the
// runtime glue (it reads `window.vxn` at load, so it isn't importable in
// isolation). Tests drive the shared factory directly with a VXN1-shaped
// adapter built from the `window.vxn` stub `installVxn` set up — the same
// code path the glue wires at runtime.
export async function loadBrowserPanel() {
  vi.resetModules();
  // Literal specifier (not a variable) so vite can statically resolve the
  // cross-crate import; the path is allow-listed via server.fs in
  // vitest.config.js.
  const { createPresetBrowser } = await import(
    '../../../../../crates/vxn-core-ui-web/assets/preset-browser.js'
  );
  // Late-bind through `window.vxn` so tests that reassign `send` /
  // `promptText` after the panel exists are honoured — matching VXN1's
  // original live `window.vxn.*` references.
  const panel = createPresetBrowser({
    send: new Proxy({}, {
      get: (_t, op) => (...args) => globalThis.window.vxn.send[op](...args),
    }),
    promptText: (title, initial, cb) => globalThis.window.vxn.promptText(title, initial, cb),
    faceplateRoot: () => document.getElementById('faceplate'),
  });
  panel.bind();
  return panel;
}
