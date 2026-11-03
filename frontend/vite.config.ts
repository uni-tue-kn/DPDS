/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// The dev server proxies /api to a running dpds-controller (default bind port 8100). In production the
// controller serves the built dist/ itself, so there is no proxy.
export default defineConfig({
  plugins: [react()],
  server: { proxy: { '/api': process.env.DPDS_CONTROLLER ?? 'http://127.0.0.1:8100' } },
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    include: ['src/**/*.test.{ts,tsx}'],
    testTimeout: 15000,
  },
});
