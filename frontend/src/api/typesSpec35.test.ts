// Spec 35 (C1, C2, C3): the hand-written DTO types follow the controller's new fields. Types do not exist at run time, so the
// source is read (as typesSpec29b.test.ts does). The tests of the dashboard compile against these names: `fx.totals()` returns
// `HistoryTotals`, `fx.counters()` fills `Counters.nic_rx_imissed` / `nic_rx_nombuf`, `fx.history()` sets `HistoryView.totals`.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const types = readFileSync(resolve(__dirname, 'types.ts'), 'utf8');

/** The members of `export interface <name> { ... }`, comments removed. */
function members(name: string): string {
  const start = types.indexOf(`export interface ${name} {`);
  expect(start, `interface ${name}`).toBeGreaterThanOrEqual(0);
  const end = types.indexOf('\n}', start);
  return types
    .slice(start, end)
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/\/\/.*$/gm, '');
}

describe('types.ts (spec 35)', () => {
  it("Counters (the `counters` of a direction in /api/stats) has the receiving port's cumulative `nic_rx_imissed` and `nic_rx_nombuf`", () => {
    const m = members('Counters');
    expect(m).toMatch(/\bnic_rx_imissed:\s*number\s*;/);
    expect(m).toMatch(/\bnic_rx_nombuf:\s*number\s*;/);
  });

  it('HistoryTotals is exported with exactly the seven counters of C1, all numbers', () => {
    const m = members('HistoryTotals');
    const keys = Array.from(m.matchAll(/^\s*(\w+)(\??):\s*([^;]+);/gm));
    expect(keys.map((k) => k[1]).sort()).toEqual(
      [
        'buffer_full_drops',
        'limiter_drops',
        'loss_drops',
        'nic_rx_imissed',
        'nic_rx_nombuf',
        'nic_tx_drops',
        'rx_pkts',
      ].sort(),
    );
    for (const k of keys) {
      expect(k[2], `${k[1]} is required`).toBe('');
      expect(k[3].trim(), `${k[1]} type`).toBe('number');
    }
  });

  it('HistoryView.totals maps the link index (a string key) to HistoryTotals, required: `{}` before any run', () => {
    expect(members('HistoryView')).toMatch(/\btotals:\s*Record<string,\s*HistoryTotals>\s*;/);
  });
});
