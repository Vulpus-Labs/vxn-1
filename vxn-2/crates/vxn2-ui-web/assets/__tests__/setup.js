// vitest setupFiles entry. The asset bundle is a set of IIFEs that attach to
// `window.__vxn`; `panels/fx-tabs.js` guards with `window.__vxn = window.__vxn
// || {}`, so no seeding is strictly required, but declare the surface up front
// so a test importing the module sees a consistent shape.
globalThis.window = globalThis.window || globalThis;
window.__vxn = window.__vxn || { panels: {} };
