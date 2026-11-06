import { fireEvent, within } from '@testing-library/react';
import { vi } from 'vitest';
import { card, radio, summary, summaryRadio } from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { renderApp, tick } from '../test/helpers';
import { api, serveHistory } from '../test/mockApi';

// At most 100 points per time chart (P4TG's select_time_keys, unit tests in common/selectTimeKeys.test.ts), for the
// rate chart and the Packet loss chart of every link and of the Summary. A 250-sample history (t = 0 .. 249 s, rx_gbps = tx_gbps =
// loss_drops = the sample's position) plots 100 points: the positions floor(j * 249 / 99), the first and the last one kept.

const N = 250;
const POSITIONS = Array.from({ length: 100 }, (_, j) => Math.floor((j * (N - 1)) / 99));
const samples = () =>
  Array.from({ length: N }, (_, i) =>
    fx.historySample(i * 1000, { ...fx.NO_PPS, rx_gbps: i, tx_gbps: i, loss_drops: i }),
  );

interface Chart {
  labels: string[];
  datasets: { label: string; data: number[] }[];
}
const lineOf = (panel: HTMLElement): Chart =>
  JSON.parse(within(panel).getByTestId('chart-line').getAttribute('data-chart') ?? 'null');

async function open() {
  serveHistory({ startedAt: 1762000000, samples: { '0': samples(), '1': samples() } });
  api.getEmulation.mockResolvedValue(fx.emulation('running'));
  api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
  vi.useFakeTimers();
  renderApp('/');
  await tick(0);
  await tick(0);
}

const dataset = (c: Chart, label: string) => {
  const d = c.datasets.find((s) => s.label === label);
  if (!d) throw new Error(`no dataset ${label}`);
  return d.data;
};

describe('Dashboard (spec 50 G): 100 points per time chart', () => {
  it('the rate chart of a link plots the 100 selected samples, the first and the last among them', async () => {
    await open();
    const c = lineOf(card(0));
    expect(c.labels).toHaveLength(100);
    expect(dataset(c, 'RX rate')).toEqual(POSITIONS);
    expect(dataset(c, 'TX rate')).toEqual(POSITIONS);
    expect(c.labels[0]).toBe('0h 0m 0s');
    expect(c.labels[99]).toBe('0h 4m 9s');
  });

  it('the Packet loss chart of a link plots the 100 selected samples, all nine lines', async () => {
    await open();
    fireEvent.click(radio(0, 'Packet loss'));
    const c = lineOf(card(0));
    expect(c.labels).toHaveLength(100);
    expect(c.datasets).toHaveLength(9);
    expect(dataset(c, 'Loss model')).toEqual(POSITIONS);
    for (const d of c.datasets) expect(d.data, d.label).toHaveLength(100);
    expect(c.labels[99]).toBe('0h 4m 9s');
  });

  it('the rate chart of the Summary plots 100 points, the first and the last sample kept', async () => {
    await open();
    const c = lineOf(summary());
    expect(c.labels).toHaveLength(100);
    expect(c.labels[0]).toBe('0h 0m 0s');
    expect(c.labels[99]).toBe('0h 4m 9s');
    for (const d of c.datasets) expect(d.data, d.label).toHaveLength(100);
  });

  it('the Packet loss chart of the Summary plots 100 points', async () => {
    await open();
    fireEvent.click(summaryRadio('Packet loss'));
    const c = lineOf(summary());
    expect(c.labels).toHaveLength(100);
    expect(c.labels[99]).toBe('0h 4m 9s');
    for (const d of c.datasets) expect(d.data, d.label).toHaveLength(100);
  });

  it('a history of at most 100 samples is plotted whole', async () => {
    serveHistory({
      startedAt: 1762000000,
      samples: { '0': samples().slice(0, 100), '1': samples().slice(0, 100) },
    });
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
    vi.useFakeTimers();
    renderApp('/');
    await tick(0);
    await tick(0);
    const c = lineOf(card(0));
    expect(dataset(c, 'RX rate')).toEqual(Array.from({ length: 100 }, (_, i) => i));
  });
});
