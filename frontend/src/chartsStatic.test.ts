// The percentile lines need chartjs-plugin-annotation. The Bar/Line stubs of test/setup.ts never run Chart.js, so what a real
// browser needs is pinned from the files: the dependency, and the plugin registered next to the other Chart.js components. Whether
// its Chart.js peer range fits chart.js 4 is checked by `npm ci` and by the build, not here.
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

  it('the existing registrations stay (Tooltip for hover, Filler for the filled rate areas, Legend, BarElement)', () => {
    const src = read('src/components/charts.ts');
    for (const part of ['Tooltip', 'Filler', 'Legend', 'BarElement']) {
      expect(src, part).toMatch(new RegExp(`\\b${part}\\b`));
    }
  });

  // The Chart.js stubs never run Chart.js, so what a real browser needs is pinned from the file: without ArcElement a doughnut throws
  // '"arc" is not a registered element', and without the Title plugin the histogram's title line is silently not drawn.
  it('registers ArcElement (the doughnut) and Title (the histogram title line)', () => {
    const src = read('src/components/charts.ts');
    const call = /ChartJS\.register\(([^)]*)\)/.exec(src);
    expect(call, 'ChartJS.register(...)').not.toBeNull();
    for (const part of ['ArcElement', 'Title']) {
      expect(call?.[1], part).toMatch(new RegExp(`\\b${part}\\b`));
      expect(src, `${part} imported from chart.js`).toMatch(
        new RegExp(`import\\s*\\{[^}]*\\b${part}\\b[^}]*\\}\\s*from\\s*['"]chart\\.js['"]`),
      );
    }
  });
});
