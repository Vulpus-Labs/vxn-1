import { defineConfig } from "vitest/config";

// jsdom environment so the FX-tab DOM wiring (`panels/fx-tabs.js`) runs against
// a real document. `setup.js` seeds the `window.__vxn` surface the IIFE bundle
// expects.
export default defineConfig({
  test: {
    environment: "jsdom",
    setupFiles: ["./__tests__/setup.js"],
    include: ["__tests__/**/*.test.js"],
  },
});
