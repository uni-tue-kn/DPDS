// Spec 28: the percentile lines need chartjs-plugin-annotation (as P4TG's charts do). The Bar/Line stubs of test/setup.ts
// never run Chart.js, so what a real browser needs is pinned from the files: the dependency, and the plugin registered
// next to the other Chart.js components. Whether its Chart.js peer range fits chart.js 4 is checked by `npm ci` (npm
// refuses an unmet peer) and by the build, not here.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const frontendDir = resolve(__dirname, '..');
const read = (p: string) => readFileSync(resolve(frontendDir, p), 'utf8');

describe('chartjs-plugin-annotation (spec 28)', () => {
  const pkg = JSON.parse(read('package.json')) as { dependencies: Record<string, string> };
  const lock = JSON.parse(read('package-lock.json')) as { packages: Record<string, unknown> };

  it('is a runtime dependency with a caret range, and is in package-lock.json (npm ci installs it)', () => {
    expect(pkg.dependencies['chartjs-plugin-annotation']).toMatch(/^\^\d+\.\d+\.\d+/);
    expect(lock.packages['node_modules/chartjs-plugin-annotation']).toBeDefined();
  });

  it('components/charts.ts imports it and registers it with Chart.js', () => {
    const src = read('src/components/charts.ts');
    const imported = /import\s+(\w+)\s+from\s+['"]chartjs-plugin-annotation['"]/.exec(src);
    expect(imported, 'default import of chartjs-plugin-annotation').not.toBeNull();
    const name = imported?.[1] ?? '';
    expect(src).toMatch(new RegExp(`ChartJS\\.register\\([^)]*\\b${name}\\b`));
  });

  it('the existing registrations stay (Tooltip for hover, Filler for the filled rate areas, LineController for the overlay)', () => {
    const src = read('src/components/charts.ts');
    for (const part of ['Tooltip', 'Filler', 'Legend', 'LineController', 'BarElement', 'LogarithmicScale']) {
      expect(src, part).toMatch(new RegExp(`\\b${part}\\b`));
    }
  });
});
