/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// The dev server proxies /api to a running dpds-controller (default bind port 8100). In production the
// controller serves the built dist/ itself, so there is no proxy.
export default defineConfig({
  plugins: [react()],
  build: {
    rolldownOptions: {
      output: {
        // Vendor chunks (rolldown `output.codeSplitting.groups`; a lower `priority` loses to a higher one).
        // The result stays a static index.html plus hashed assets under assets/.
        codeSplitting: {
          groups: [
            { name: 'react', test: /node_modules[\\/](react|react-dom|react-router|scheduler)[\\/]/, priority: 3 },
            {
              name: 'chart',
              test: /node_modules[\\/](chart\.js|react-chartjs-2|chartjs-plugin-[^\\/]+|@kurkle)[\\/]/,
              priority: 2,
            },
            {
              name: 'bootstrap',
              test: /node_modules[\\/](bootstrap|react-bootstrap|@restart|@popperjs|react-transition-group|dom-helpers)[\\/]/,
              priority: 1,
            },
          ],
        },
      },
    },
  },
  server: { proxy: { '/api': process.env.DPDS_CONTROLLER ?? 'http://127.0.0.1:8100' } },
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    include: ['src/**/*.test.{ts,tsx}'],
    testTimeout: 15000,
  },
});
