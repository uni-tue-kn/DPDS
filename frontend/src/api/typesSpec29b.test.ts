// The hand-written DTO types follow the API: `GET /api/stats` omits `elapsed_ms` when nothing has run (it is never `null`), so the
// key is `elapsed_ms?: number` with no `| null`. The history answer carries `run_id` (null before any run) and `links`. Types do
// not exist at run time, so the source is read.
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

describe('types.ts (spec 29b)', () => {
  it('StatsView.elapsed_ms is optional and not nullable: `elapsed_ms?: number;`', () => {
    const m = members('StatsView');
    expect(m).toMatch(/\belapsed_ms\?:\s*number\s*;/);
    expect(m).not.toMatch(/\belapsed_ms\??:[^;]*null/);
  });

  it('HistoryView has `run_id: number | null` and a `links` list of {name, ingress, egress}', () => {
    const m = members('HistoryView');
    expect(m).toMatch(/\brun_id:\s*number\s*\|\s*null\s*;/);
    expect(m).toMatch(/\blinks:\s*[A-Za-z]+\[\]\s*;|\blinks:\s*\{[^}]*\}\[\]\s*;/);
  });
});
