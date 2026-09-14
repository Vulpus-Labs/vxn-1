import { defineConfig } from "vitest/config";

// jsdom so the panel factories can be built against a real document — they
// return DOM, so a node environment could not exercise them at all.
export default defineConfig({
  // The shared widget primitives live in the sibling crate
  // (../../../../../crates/vxn-core-ui-web/assets/); allow vite to serve them
  // so a panel that imports `valuePop` / `wireDrag` resolves to the real owner
  // rather than a stub that could drift from it.
  server: { fs: { allow: ["../../../../.."] } },
  test: {
    environment: "jsdom",
    include: ["__tests__/**/*.test.js"],
  },
});
