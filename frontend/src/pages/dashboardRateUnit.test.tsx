// User 2026-11-05: the rate chart's y axis is titled with the unit only, "Gbit/s" like P4TG (Visuals.tsx: y title 'Gbit/s'),
// and "if it is only Mbit/s we show a lower unit". Contract:
//   - the unit is the largest of Gbit/s, Mbit/s, kbit/s, bit/s in which the chart's largest RX or TX value is >= 1;
//     no samples or only zeros: Gbit/s;
//   - the y title is that unit alone ("Gbit/s", "Mbit/s", ...), and both datasets are scaled into it;
//   - each chart (link or Summary) picks its own unit from its own samples.
import { within } from '@testing-library/react';
import { vi } from 'vitest';
import type { HistorySample } from '../api/types';
import * as fx from '../test/fixtures';
import { api, serveHistory } from '../test/mockApi';
import { renderApp, tick } from '../test/helpers';
import { card } from '../test/dashboardUi';
import { must } from '../test/must';

interface ChartData {
  datasets: { label?: string; data: number[] }[];
}

const chartEl = (d: number) => within(card(d)).getByTestId('chart-line');
const dataOf = (d: number): ChartData => JSON.parse(chartEl(d).getAttribute('data-chart') ?? 'null');
const yTitle = (d: number) => JSON.parse(chartEl(d).getAttribute('data-options') ?? 'null').scales.y.title.text;
const series = (d: number, re: RegExp) =>
  must(
    dataOf(d).datasets.find((s) => re.test(s.label ?? '')),
    `${re} dataset`,
  ).data;

async function open(samples: Record<string, HistorySample[]>) {
  serveHistory({ startedAt: 1762000000, samples });
  vi.useFakeTimers();
  renderApp('/');
  await tick(0);
  await tick(0);
}

const at = (t: number, rx: number, tx: number) => fx.historySample(t, { rx_gbps: rx, tx_gbps: tx });

beforeEach(() => {
  api.getEmulation.mockResolvedValue(fx.emulation('running'));
  api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
});

describe('rate chart unit (P4TG "Gbit/s", a lower unit for slow links)', () => {
  it('at least 1 Gbit/s: the y title is "Gbit/s" and the values stay in Gbit/s', async () => {
    await open({ '0': [at(1000, 9.5, 9.25), at(2000, 0.5, 0.25)], '1': [at(1000, 1, 0.5)] });
    expect(yTitle(0)).toBe('Gbit/s');
    expect(series(0, /RX/)).toEqual([9.5, 0.5]);
    expect(series(0, /TX/)).toEqual([9.25, 0.25]);
    expect(yTitle(1)).toBe('Gbit/s');
  });

  it('below 1 Gbit/s but at least 1 Mbit/s: "Mbit/s", both series scaled by 1000', async () => {
    await open({ '0': [at(1000, 0.5, 0.25), at(2000, 0.75, 0.002)], '1': [at(1000, 9.5, 9.25)] });
    expect(yTitle(0)).toBe('Mbit/s');
    const rx = series(0, /RX/);
    const tx = series(0, /TX/);
    expect(rx[0]).toBeCloseTo(500, 9);
    expect(rx[1]).toBeCloseTo(750, 9);
    expect(tx[0]).toBeCloseTo(250, 9);
    expect(tx[1]).toBeCloseTo(2, 9);
    expect(yTitle(1)).toBe('Gbit/s');
  });

  it('a TX peak alone decides as well', async () => {
    await open({ '0': [at(1000, 0.0005, 0.002)], '1': [at(1000, 9.5, 9.25)] });
    expect(yTitle(0)).toBe('Mbit/s');
  });

  it('below 1 Mbit/s: "kbit/s"; below 1 kbit/s: "bit/s"', async () => {
    await open({ '0': [at(1000, 0.0000015, 0.000001)], '1': [at(1000, 0.0000000005, 0)] });
    expect(yTitle(0)).toBe('kbit/s');
    expect(series(0, /RX/)[0]).toBeCloseTo(1.5, 9);
    expect(yTitle(1)).toBe('bit/s');
    expect(series(1, /RX/)[0]).toBeCloseTo(0.5, 9);
  });

  it('no traffic (zeros) and no samples: "Gbit/s"', async () => {
    await open({ '0': [at(1000, 0, 0)], '1': [] });
    expect(yTitle(0)).toBe('Gbit/s');
    expect(yTitle(1)).toBe('Gbit/s');
  });
});
