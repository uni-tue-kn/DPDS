import '@testing-library/jest-dom/vitest';
import React from 'react';
import { vi } from 'vitest';
import { resetApi } from './mockApi';
import { setPrefersDark } from './matchMedia';

// Whole API module replaced by the mocks (the real one is only tested in api/client.test.ts).
vi.mock('../api/client', async () => (await import('./mockApi')).clientMock());

// Chart.js needs a canvas; the stubs expose the chart data as JSON so tests can inspect it.
//   histogram = <Bar data=...>   -> data-testid="chart-bar"
//   rate/drop charts = <Line data=...> -> data-testid="chart-line"
// Both also expose their chart options as JSON in `data-options` (functions are dropped by JSON.stringify).
vi.mock('react-chartjs-2', () => {
  const stub = (testid: string) => (props: { data?: unknown; options?: unknown }) =>
    React.createElement('div', {
      'data-testid': testid,
      'data-chart': JSON.stringify(props.data ?? null),
      'data-options': JSON.stringify(props.options ?? null),
    });
  return { Bar: stub('chart-bar'), Line: stub('chart-line') };
});

// Spec 18: the sidebar fetches the release version file from raw.githubusercontent.com in the browser.
// No test may reach the network, so `fetch` is down by default (an unreachable GitHub means "no update");
// updateCheck.test.tsx installs its own mock.
beforeEach(() => {
  vi.stubGlobal(
    'fetch',
    vi.fn(() => Promise.reject(new TypeError('Failed to fetch'))),
  );
  localStorage.clear();
  document.documentElement.removeAttribute('data-theme');
  setPrefersDark(false);
  resetApi();
});

afterEach(() => {
  vi.useRealTimers();
});
