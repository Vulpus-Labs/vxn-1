// E015 / 0079: vitest setupFiles entry. The four asset files carry
// `__PARAMS_JSON__` / `__SUBDIVISIONS_JSON__` / `__PATCH_COUNT__`
// placeholders that Rust string-replaces at splice time. Under Node ESM
// they appear as bare identifiers; seed each one on `globalThis` so the
// bridge.js IIFE evaluates cleanly when a test imports it.
globalThis.__PARAMS_JSON__ = {};
globalThis.__SUBDIVISIONS_JSON__ = [];
globalThis.__PATCH_COUNT__ = 100;
