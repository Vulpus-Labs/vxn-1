import { defineConfig } from 'vitest/config';

export default defineConfig({
  // The preset browser logic lives in the shared crate
  // (../../../../crates/vxn-core-ui-web/assets/preset-browser.js); allow vite
  // to serve it for the browser test suites.
  server: { fs: { allow: ['../../../..'] } },
  test: {
    environment: 'jsdom',
    include: ['__tests__/**/*.test.js'],
    setupFiles: ['__tests__/setup.js'],
  },
});
