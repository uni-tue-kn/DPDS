import { fireEvent, within } from '@testing-library/react';
import { vi } from 'vitest';
import { card, radio, tab } from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { renderApp, tick } from '../test/helpers';
import { api } from '../test/mockApi';

// Spec 35 review findings, round 2.
//  1. A link tab that is not shown does not poll its delay histogram (react-bootstrap keeps every pane mounted); the chosen radio
//     survives a tab switch and polling resumes when the tab shows again.
//  2. The error of a refused start survives polls that change the state; only the user's next action (Start, Stop, Restart) clears it.

const histogramCalls = (d: number) => api.getDelayHistogram.mock.calls.filter(([dir]) => dir === d).length;

describe('Dashboard (spec 35 review): hidden link tabs do not poll the delay histogram', () => {
  async function open() {
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(radio(0, 'Delay histogram'));
    await tick(0);
    await tick(2500);
    expect(histogramCalls(0)).toBeGreaterThanOrEqual(2); // it polls while shown
  }

  it('link 0 makes no getDelayHistogram call while link 1 is the selected tab', async () => {
    await open();
    fireEvent.click(tab(1));
    await tick(0);
    const before = histogramCalls(0);
    await tick(5000);
    expect(histogramCalls(0)).toBe(before);
  });

  it('back on link 0 the radio is still "Delay histogram" and the polling resumes', async () => {
    await open();
    fireEvent.click(tab(1));
    await tick(5000);
    const before = histogramCalls(0);
    fireEvent.click(tab(0));
    await tick(0);
    expect(within(card(0)).getByRole('radio', { name: 'Delay histogram' })).toBeChecked();
    await tick(3000);
    expect(histogramCalls(0)).toBeGreaterThan(before);
  });
});
