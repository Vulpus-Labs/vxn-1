import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'jsdom',
    include: ['__tests__/**/*.test.js'],
    setupFiles: ['__tests__/setup.js'],
  },
});
