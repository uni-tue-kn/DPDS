// Spec 35 (C1, C2, C3): the hand-written DTO types follow the controller's new fields. Types do not exist at run time, so the
// source is read (as typesSpec29b.test.ts does). The tests of the dashboard compile against these names: `fx.totals()` returns
// `HistoryTotals`, `fx.counters()` fills `Counters.nic_rx_missed` / `nic_rx_nombuf` (spec 47: renamed from nic_rx_imissed), `fx.history()` sets `HistoryView.totals`.
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
  it("Counters (the `counters` of a direction in /api/stats) has the ports' cumulative NIC loss counts, nic_rx_missed renamed from nic_rx_imissed (spec 47)", () => {
    const m = members('Counters');
    for (const k of [
      'nic_rx_missed',
      'nic_rx_nombuf',
      'nic_rx_port_buffer_discards',
      'nic_rx_phy_discards',
      'nic_tx_phy_discards',
    ]) {
      expect(m, k).toMatch(new RegExp(`\\b${k}:\\s*number\\s*;`));
    }
    expect(m).not.toMatch(/nic_rx_imissed/);
  });

  it('Counters has the three NIC diagnostics, which no loss total contains (spec 47 C4)', () => {
    const m = members('Counters');
    for (const k of ['nic_rx_crc_errors', 'nic_rx_symbol_errors', 'nic_rx_length_errors']) {
      expect(m, k).toMatch(new RegExp(`\\b${k}:\\s*number\\s*;`));
    }
  });

  it('the diagnostics are in Counters only, not in HistorySample or HistoryTotals (spec 47 C4)', () => {
    for (const name of ['HistorySample', 'HistoryTotals']) {
      expect(members(name), name).not.toMatch(/crc_errors|symbol_errors|length_errors/);
    }
  });

  it('HistoryTotals is exported with exactly the rx_pkts and the nine cumulative drop counters (spec 35 C1, spec 47 C4), all numbers', () => {
    const m = members('HistoryTotals');
    const keys = Array.from(m.matchAll(/^\s*(\w+)(\??):\s*([^;]+);/gm));
    expect(keys.map((k) => k[1]).sort()).toEqual(
      [
        'buffer_full_drops',
        'limiter_drops',
        'loss_drops',
        'nic_rx_missed',
        'nic_rx_nombuf',
        'nic_rx_phy_discards',
        'nic_rx_port_buffer_discards',
        'nic_tx_phy_discards',
        'nic_tx_drops',
        'rx_pkts',
        'tx_sent', // spec 51: the forwarded frames
        'frame_sizes', // spec 51
        'out_of_order', // spec 51
      ].sort(),
    );
    for (const k of keys) {
      expect(k[2], `${k[1]} is required`).toBe('');
      expect(k[3].trim(), `${k[1]} type`).toBe(k[1] === 'frame_sizes' ? 'number[]' : 'number');
    }
  });

  it('HistoryView.totals maps the link index (a string key) to HistoryTotals, required: `{}` before any run', () => {
    expect(members('HistoryView')).toMatch(/\btotals:\s*Record<string,\s*HistoryTotals>\s*;/);
  });
});
