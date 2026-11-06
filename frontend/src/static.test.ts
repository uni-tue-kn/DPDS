// Static checks of how the frontend is delivered (spec v13, spec v15) and of leftovers in the source tree (F6).
// The controller embeds `dist/` and serves it, `dpds.sh install` builds it on the host with Node (spec v15:
// no CI release asset any more), and the Vite dev server proxies /api to a running controller
// (src/viteConfig.test.ts). These tests read the files as text from the repository
// (frontend/ = the directory vitest runs in, its parent is the repository root); nothing is built or
// started.
import { execSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, utimesSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const frontendDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const repoDir = resolve(frontendDir, '..');
const read = (...p: string[]) => readFileSync(resolve(frontendDir, ...p), 'utf8');

describe('no Docker or nginx deployment of the frontend (spec v13)', () => {
  it.each(['docker-compose.yml', 'frontend/Dockerfile', 'frontend/nginx.conf', 'frontend/.dockerignore'])(
    '%s is gone',
    (file) => {
      expect(existsSync(resolve(repoDir, file)), file).toBe(false);
    },
  );
});

describe('index.html works under the controller CSP (default-src self, no inline code)', () => {
  const html = read('index.html');

  it('has no inline <script> body, inline <style> element, style= or on*= attributes', () => {
    const scripts = Array.from(html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/gi));
    expect(scripts.length).toBeGreaterThan(0);
    for (const [, attrs, body] of scripts) {
      expect(attrs).toMatch(/\bsrc=/);
      expect(body.trim()).toBe('');
    }
    expect(html).not.toMatch(/<style\b/i);
    expect(html).not.toMatch(/\sstyle\s*=/i);
    expect(html).not.toMatch(/\son[a-z]+\s*=/i);
  });

  it('loads nothing from another origin', () => {
    expect(html).not.toMatch(/(src|href)\s*=\s*["']?(https?:)?\/\//i);
  });
});

describe('.github/workflows/ci.yml: no frontend release asset any more (spec v15)', () => {
  const ci = readFileSync(resolve(repoDir, '.github/workflows/ci.yml'), 'utf8');

  /** The text of the job `name` (2-space indented key under `jobs:`), up to the next job. */
  function job(name: string): string {
    const lines = ci.split('\n');
    const start = lines.indexOf(`  ${name}:`);
    if (start < 0) return '';
    let end = lines.findIndex((l, i) => i > start && /^ {2}[\w-]+:\s*$/.test(l));
    if (end < 0) end = lines.length;
    return lines.slice(start, end).join('\n');
  }
  /** The job ids: the 2-space indented keys under `jobs:`. */
  const jobIds = (() => {
    const lines = ci.split('\n');
    const start = lines.indexOf('jobs:');
    return lines
      .slice(start + 1)
      .map((l) => /^ {2}([\w-]+):\s*$/.exec(l)?.[1])
      .filter((id): id is string => id !== undefined);
  })();

  it('has exactly the jobs backend, frontend, installer and release (spec v23), and no frontend-asset-* job', () => {
    expect([...jobIds].sort()).toEqual(['backend', 'frontend', 'installer', 'release']);
    for (const old of ['fmt', 'rust', 'packaging']) {
      expect(jobIds, `the job ${old} was merged or renamed`).not.toContain(old);
      expect(job(old), old).toBe('');
    }
    expect(jobIds.filter((id) => /^frontend-asset/.test(id))).toEqual([]);
    expect(job('frontend-asset-build')).toBe('');
    expect(job('frontend-asset-publish')).toBe('');
    expect(job('frontend-asset')).toBe('');
  });

  // Spec 18 allows exactly one exception: the `release` job on main (tag + GitHub release with generated notes,
  // no build assets). Everything else stays read-only and publishes nothing. The Rust side pins the job itself
  // (crates/dpds-controller/tests/integration/release.rs).
  const release = job('release');
  const rest = ci.replace(release, '');

  it('is read-only apart from the release job: contents: write only there, and the workflow default stays contents: read', () => {
    expect(release).not.toBe('');
    expect(release.match(/contents:\s*write/g)).toHaveLength(1);
    expect(rest).not.toMatch(/contents:\s*write/);
    expect(ci.match(/:\s*write\b/g)).toHaveLength(1);
    expect(ci).toMatch(/^permissions:\s*\n\s+contents:\s*read/m);
  });

  it('publishes nothing but the release: gh release and the token only in the release job, no assets or artifacts', () => {
    expect(release).toMatch(/\bgh release create\b/);
    expect(release).toMatch(/GH_TOKEN:\s*\$\{\{\s*secrets\.GITHUB_TOKEN\s*\}\}/);
    expect(rest).not.toMatch(/\bgh release\b/);
    expect(rest).not.toMatch(/GH_TOKEN|GITHUB_TOKEN/);
    expect(ci).not.toMatch(/frontend-builds/);
    expect(ci).not.toMatch(/actions\/(upload|download)-artifact/);
    expect(ci).not.toMatch(/gh release upload|upload-release-asset|--clobber/);
    expect(ci).not.toMatch(/dpds-frontend-/);
  });

  it('no longer says that dpds.sh downloads the frontend or that it is a release asset', () => {
    expect(ci).not.toMatch(/frontend-asset|release asset|frontend asset|dpds\.sh install downloads/i);
  });

  /** The job's text without comment lines, so a step named in a comment doesn't count. */
  const code = (name: string) =>
    job(name)
      .split('\n')
      .filter((l) => !l.trim().startsWith('#'))
      .join('\n');

  /** Asserts that every pattern occurs in `text` and that their first occurrences are in this order. */
  function expectInOrder(text: string, steps: [string, RegExp][]) {
    const at = steps.map(([label, re]) => ({ label, i: text.search(re) }));
    for (const { label, i } of at) expect(i, `step missing: ${label}`).toBeGreaterThanOrEqual(0);
    for (let k = 1; k < at.length; k++) {
      expect(at[k - 1].i, `${at[k - 1].label} must come before ${at[k].label}`).toBeLessThan(at[k].i);
    }
  }

  it('the frontend job runs npm ci, npm run format:check, npm run lint, then build, tsc and vitest, in frontend/ (spec v23)', () => {
    const frontend = code('frontend');
    expect(frontend).not.toBe('');
    expect(frontend).toMatch(/working-directory:\s*frontend/);
    expectInOrder(frontend, [
      ['npm ci', /\bnpm ci\b/],
      ['format check', /\bnpm run format:check\b/],
      ['lint', /\bnpm run lint\b/],
      ['build', /\bnpm run build\b/],
      ['tsc', /\btsc --noEmit\b/],
      ['vitest', /\bvitest run\b/],
    ]);
    // the checks are plain `npm run` steps: neither writes (`format` would hide an unformatted tree)
    expect(frontend).not.toMatch(/\bnpm run format\s*$/m);
    expect(frontend).not.toMatch(/prettier --write/);
  });

  it('the backend job runs fmt first, then both clippy runs, the tests, the telemetry build and the controller e2e (spec v23)', () => {
    const backend = code('backend');
    expect(backend).not.toBe('');
    expectInOrder(backend, [
      ['cargo fmt', /cargo fmt --all -- --check/],
      ['clippy', /cargo clippy --workspace --all-targets -- -D warnings/],
      ['clippy, all features', /cargo clippy --workspace --all-targets --all-features -- -D warnings/],
      ['workspace tests', /cargo test --workspace --release/],
      ['dpds telemetry tests', /cargo test -p dpds --release --features telemetry/],
      ['telemetry build', /cargo build --release -p dpds --features telemetry/],
      ['controller e2e', /cargo test --release -p dpds-controller --test integration -- --ignored/],
    ]);
    // cargo fmt is the only fmt step: once, and not as a separate job any more
    expect(backend.match(/cargo fmt/g)).toHaveLength(1);
  });

  it('the backend job installs rustfmt and clippy with its toolchain (it runs both now)', () => {
    const toolchain = /dtolnay\/rust-toolchain[\s\S]*?components:\s*([^\n]*)/.exec(code('backend'));
    expect(toolchain, 'a rust-toolchain step with components').not.toBeNull();
    expect(toolchain?.[1]).toMatch(/\brustfmt\b/);
    expect(toolchain?.[1]).toMatch(/\bclippy\b/);
  });

  it('the backend job keeps the DPDK build and cache, the plain-build env and the pinned toolchain (spec v23: nothing else changes)', () => {
    const backend = code('backend');
    expect(backend).toMatch(/\.github\/scripts\/install-dpdk\.sh/);
    expect(backend).toMatch(/actions\/cache\/restore@/);
    expect(backend).toMatch(/actions\/cache\/save@/);
    expect(backend).toMatch(/RUSTFLAGS:\s*""/);
    expect(backend).toMatch(/RUSTDOCFLAGS:\s*""/);
    expect(backend).toMatch(/CC:\s*gcc/);
    expect(backend).toMatch(/toolchain:\s*1\.99\.1/);
    expect(backend).toMatch(/timeout-minutes:\s*60/);
  });

  it('the installer job runs the dpds.sh tests', () => {
    expect(code('installer')).toMatch(/packaging\/tests\/dpds_sh\.test\.sh/);
  });

  it('the release job needs exactly backend, frontend and installer', () => {
    const needs = /^ {4}needs:\s*\[([^\]]*)\]/m.exec(job('release'));
    expect(needs, 'needs: [..] in the release job').not.toBeNull();
    const ids = (needs?.[1] ?? '').split(',').map((x) => x.trim());
    expect([...ids].sort()).toEqual(['backend', 'frontend', 'installer']);
  });

  it('the DPDK comment names the new minimum, not 25.11 (user, 2026-11-03)', () => {
    expect(ci).not.toMatch(/needs\s*>=\s*25\.11/);
  });

  it('the workflow has no Docker left', () => {
    expect(ci).not.toMatch(/docker/i);
  });
});

describe('package.json engines (dpds.sh builds the frontend with Node on the host, spec v15)', () => {
  const enginesMin = (JSON.parse(read('package.json')) as { engines: { node: string } }).engines.node.replace(
    /^>=\s*/,
    '',
  );

  it('the engines.node minimum is written as >=X.Y (dpds.sh quotes it)', () => {
    expect(enginesMin).toMatch(/^\d+\.\d+/);
  });
});

describe('frontend build script (v13b item 2)', () => {
  const pkg = JSON.parse(read('package.json')) as { scripts: Record<string, string> };

  it('touches crates/dpds-controller/build.rs after vite build so the next cargo build replaces the placeholder', () => {
    const script = pkg.scripts.build;
    const at = script.indexOf('vite build');
    expect(at, script).toBeGreaterThanOrEqual(0);
    const after = script.slice(at + 'vite build'.length);
    expect(after).toMatch(/\.\.\/crates\/dpds-controller\/build\.rs/);
  });

  it('what follows vite build really bumps the mtime of build.rs (run in a copy of the layout)', () => {
    const script = pkg.scripts.build;
    const after = script.slice(script.indexOf('vite build') + 'vite build'.length).replace(/^\s*&&\s*/, '');
    expect(after.trim(), script).not.toBe('');
    const root = mkdtempSync(join(tmpdir(), 'dpds-build-'));
    try {
      mkdirSync(join(root, 'frontend'));
      mkdirSync(join(root, 'crates/dpds-controller'), { recursive: true });
      const buildRs = join(root, 'crates/dpds-controller/build.rs');
      writeFileSync(buildRs, 'fn main() {}\n');
      const old = new Date('2000-01-01T00:00:00Z');
      utimesSync(buildRs, old, old);
      execSync(after, { cwd: join(root, 'frontend'), stdio: 'pipe' });
      expect(statSync(buildRs).mtimeMs).toBeGreaterThan(old.getTime() + 24 * 3600 * 1000);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

describe('leftovers (F6)', () => {
  it('the unused openapi.json snapshot is gone and nothing refers to it', () => {
    expect(existsSync(resolve(frontendDir, 'src/api/openapi.json'))).toBe(false);
    expect(read('src/api/types.ts')).not.toMatch(/openapi\.json/);
  });

  it('types.ts no longer carries the stale "controller does not serve yet" v6 note', () => {
    expect(read('src/api/types.ts')).not.toMatch(/does not serve yet|at the time of writing/i);
  });
});

describe('host uplink guard (spec v27)', () => {
  it('src has no kernel_route left (renamed to uplink everywhere)', () => {
    const hits = execSync('grep -rlE "kernel_route|kernelRoute" src || true', { cwd: frontendDir })
      .toString()
      .split('\n')
      .filter((f) => f && f !== 'src/static.test.ts');
    expect(hits).toEqual([]);
  });

  it('the Interface type carries a boolean `uplink` and no management-NIC wording', () => {
    const types = read('src/api/types.ts');
    expect(types).toMatch(/\buplink\s*:\s*boolean\b/);
    expect(types).not.toMatch(/management NIC/i);
  });

  it('no non-test source says "management NIC" any more', () => {
    const hits = execSync('grep -rliE "management NIC" src || true', { cwd: frontendDir })
      .toString()
      .split('\n')
      .filter((f) => f && !/\.test\.tsx?$/.test(f) && f !== 'src/test/helpers.tsx');
    expect(hits).toEqual([]);
  });
});

// Frontend formatting and linting (spec v23, changed 2026-11-04): Biome for both, because typescript-eslint
// doesn't support TypeScript 7. Static checks of the files only; the tool itself runs in CI
// (`npm run format:check`, `npm run lint`). Flags and config keys checked against @biomejs/biome 2.6.4:
// `biome lint --error-on-warnings`, `biome format [--write]`, `javascript.formatter.quoteStyle`/`semicolons`,
// `formatter.lineWidth`, `files.includes` with `!` entries.
describe('frontend lint and format tooling: Biome (spec v23)', () => {
  const pkg = JSON.parse(read('package.json')) as {
    scripts: Record<string, string>;
    dependencies: Record<string, string>;
    devDependencies: Record<string, string>;
  };
  const lock = JSON.parse(read('package-lock.json')) as { packages: Record<string, unknown> };
  const biomeConfig = () => read('biome.json');

  it('@biomejs/biome is a devDependency pinned to an exact version and is installed by npm ci (package-lock.json)', () => {
    expect(pkg.devDependencies['@biomejs/biome']).toMatch(/^\d+\.\d+\.\d+$/);
    expect(pkg.dependencies['@biomejs/biome']).toBeUndefined();
    expect(lock.packages['node_modules/@biomejs/biome'], 'package-lock.json: @biomejs/biome').toBeDefined();
  });

  it('has no ESLint, Prettier or typescript-eslint left: no dependency, no config file, no ignore file', () => {
    const all = { ...pkg.dependencies, ...pkg.devDependencies };
    const stale = Object.keys(all).filter((n) => /eslint|prettier/i.test(n));
    expect(stale).toEqual([]);
    const lockStale = Object.keys(lock.packages).filter((n) => /node_modules\/(.*eslint|prettier)/i.test(n));
    expect(lockStale, 'package-lock.json entries').toEqual([]);
    for (const f of [
      'eslint.config.js',
      'eslint.config.mjs',
      'eslint.config.ts',
      '.eslintrc',
      '.eslintrc.json',
      '.eslintrc.js',
      '.eslintrc.cjs',
      '.eslintignore',
      '.prettierrc',
      '.prettierrc.json',
      '.prettierignore',
      'prettier.config.js',
    ]) {
      expect(existsSync(resolve(frontendDir, f)), f).toBe(false);
    }
  });

  it('keeps the existing scripts', () => {
    expect(pkg.scripts.build).toMatch(/^tsc --noEmit && vite build/);
    expect(pkg.scripts.test).toBe('vitest run');
    expect(pkg.scripts.typecheck).toBe('tsc --noEmit');
  });

  it('the scripts mention neither eslint nor prettier', () => {
    expect(JSON.stringify(pkg.scripts)).not.toMatch(/eslint|prettier/i);
  });

  it('lint runs biome lint over the project and treats warnings as errors', () => {
    expect(pkg.scripts.lint).toMatch(/^biome lint\b/);
    expect(pkg.scripts.lint).toMatch(/--error-on-warnings\b/);
    expect(pkg.scripts.lint).toMatch(/\s\.$/);
    expect(pkg.scripts.lint).not.toMatch(/--write|--fix|--unsafe|--max-diagnostics/);
  });

  it('format runs biome format with --write over the project', () => {
    expect(pkg.scripts.format).toMatch(/^biome format\b/);
    expect(pkg.scripts.format).toMatch(/--write\b/);
    expect(pkg.scripts.format).toMatch(/\s\.$/);
  });

  it('format:check runs biome format without writing, so an unformatted file fails it', () => {
    expect(pkg.scripts['format:check']).toMatch(/^biome format\b/);
    expect(pkg.scripts['format:check']).not.toMatch(/--write|--fix/);
    expect(pkg.scripts['format:check']).toMatch(/\s\.$/);
  });

  describe('biome.json', () => {
    it('exists in frontend/ and uses the schema of the pinned version', () => {
      expect(existsSync(resolve(frontendDir, 'biome.json'))).toBe(true);
      const version = pkg.devDependencies['@biomejs/biome'];
      expect(biomeConfig()).toContain(`https://biomejs.dev/schemas/${version}/schema.json`);
    });

    it('formats with the style of the existing code: single quotes, semicolons, 2 spaces, an explicit width of 100 to 160', () => {
      const text = biomeConfig();
      expect(text).toMatch(/"quoteStyle"\s*:\s*"single"/);
      // "always" is Biome's default, so the key may be left out; "asNeeded" would drop the existing semicolons
      expect(text).not.toMatch(/"semicolons"\s*:\s*"asNeeded"/);
      expect(text).toMatch(/"indentStyle"\s*:\s*"space"/);
      expect(text).toMatch(/"indentWidth"\s*:\s*2\b/);
      const width = /"lineWidth"\s*:\s*(\d+)/.exec(text);
      expect(width, 'lineWidth is set explicitly').not.toBeNull();
      expect(Number(width?.[1])).toBeGreaterThanOrEqual(100);
      expect(Number(width?.[1])).toBeLessThanOrEqual(160);
    });

    it('keeps the formatter and the linter on, with no rule switched off or relaxed to hide findings', () => {
      const text = biomeConfig();
      expect(text).not.toMatch(/"enabled"\s*:\s*false/);
      expect(text).not.toMatch(/:\s*"off"/);
    });

    it('does not lint or format dist (a `!` entry in files.includes, or the .gitignore through vcs)', () => {
      const text = biomeConfig();
      const excluded = /"!+[^"]*dist[^"]*"/.test(text);
      const viaGitignore =
        /"useIgnoreFile"\s*:\s*true/.test(text) &&
        /"enabled"\s*:\s*true/.test(text) &&
        read('.gitignore')
          .split('\n')
          .some((l) => l.trim().replace(/^\//, '').replace(/\/$/, '') === 'dist');
      expect(excluded || viaGitignore).toBe(true);
    });
  });

  it('node_modules and dist stay in .gitignore', () => {
    const lines = read('.gitignore')
      .split('\n')
      .map((l) => l.trim().replace(/^\//, '').replace(/\/$/, ''));
    expect(lines).toContain('dist');
    expect(lines).toContain('node_modules');
  });

  it('the source has no suppression comments (no biome-ignore, no eslint-disable)', () => {
    const hits = execSync('grep -rlE "biome-ignore|eslint-disable" src || true', { cwd: frontendDir })
      .toString()
      .split('\n')
      .filter((f) => f && f !== 'src/static.test.ts');
    expect(hits).toEqual([]);
  });
});

// Spec 27 (user 2026-11-04): presets are gone, replaced by Import/Export on the links page and Duplicate in its list (the page is called Settings since spec 41).
describe('presets are removed from the source (spec27)', () => {
  const src = (f: string) => read('src', f);

  it.each(['pages/Presets.tsx', 'pages/PresetEditor.tsx', 'pages/SavePresetModal.tsx'])('%s is gone', (file) => {
    expect(existsSync(resolve(frontendDir, 'src', file)), file).toBe(false);
  });

  it.each(['pages/presets.test.tsx', 'pages/presetsInEditor.test.tsx'])('the old test %s is gone', (file) => {
    expect(existsSync(resolve(frontendDir, 'src', file)), file).toBe(false);
  });

  // spec 41: the Links page is the Settings page now (its file may keep either name); the old settings form is gone
  it.each(
    [
      'App.tsx',
      'pages/ConfigFieldset.tsx',
      'pages/LinkEditor.tsx',
      'pages/Links.tsx',
      'pages/Settings.tsx',
      'pages/fieldHelp.ts',
      'pages/LoadRunning.tsx',
    ].filter((f) => existsSync(resolve(frontendDir, 'src', f))),
  )('%s does not mention presets', (file) => {
    expect(src(file)).not.toMatch(/preset/i);
  });

  it('the DTO types have no Preset, PresetInput or presets member', () => {
    const types = src('api/types.ts');
    expect(types).not.toMatch(/\bPreset(Input)?\b/);
    expect(types).not.toMatch(/\bpresets\??\s*:/);
  });

  it('the local store has no preset functions and no presets member in its empty document', () => {
    const store = src('common/localConfig.ts');
    expect(store).not.toMatch(/savePreset|deletePreset|isPreset|PresetInput/);
    expect(store).not.toMatch(/presets\s*:\s*\[/);
  });

  it('there is no help text for presets, the old "Import setup file" or the preset-only "Description" field', () => {
    const help = src('pages/fieldHelp.ts');
    expect(help).not.toMatch(/Import setup file|Apply preset|Preset name/);
    expect(help).not.toMatch(/^\s*Description:/m);
  });
});
