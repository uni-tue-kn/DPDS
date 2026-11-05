// Spec 43 (B1, B3, F6): the hand-written DTO types follow the controller. Types do not exist at run time, so the source is read
// (as typesSpec35.test.ts does).
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

const keysOf = (m: string) => Array.from(m.matchAll(/^\s*(\w+)\??:\s*[^;]+;/gm)).map((k) => k[1]);

describe('types.ts (spec 43)', () => {
  it('HistorySample has the rates and the nine cumulative drop counters (spec 47), and no *_pct field', () => {
    const keys = keysOf(members('HistorySample'));
    expect(keys.sort()).toEqual(
      [
        't_ms',
        'rx_gbps',
        'tx_gbps',
        'rx_mpps',
        'tx_mpps',
        'loss_drops',
        'limiter_drops',
        'buffer_full_drops',
        'nic_tx_drops',
        'nic_rx_missed',
        'nic_rx_nombuf',
        'nic_rx_port_buffer_discards',
        'nic_rx_phy_discards',
        'nic_tx_phy_discards',
      ].sort(),
    );
    expect(members('HistorySample')).toMatch(/\bnic_rx_nombuf:\s*number\s*;/);
  });

  it('StatsView has no `ports` and PortStats is gone', () => {
    expect(keysOf(members('StatsView'))).not.toContain('ports');
    expect(types).not.toMatch(/\bPortStats\b/);
  });

  it('DirectionStats keeps its live drop shares (not part of this change)', () => {
    expect(keysOf(members('DirectionStats'))).toEqual(expect.arrayContaining(['loss_pct', 'nic_rx_pct']));
  });
});
