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
  it.each([
    'docker-compose.yml',
    'frontend/Dockerfile',
    'frontend/nginx.conf',
    'frontend/.dockerignore',
  ])('%s is gone', (file) => {
    expect(existsSync(resolve(repoDir, file)), file).toBe(false);
  });

  const stale = /docker-compose|docker compose|Dockerfile|nginx|DPDS_FRONTEND_(BIND|PORT)/i;

  it.each([
    ['frontend/README.md', () => read('README.md')],
    ['README.md', () => readFileSync(resolve(repoDir, 'README.md'), 'utf8')],
  ])('%s no longer describes the container deployment', (_name, text) => {
    expect(text()).not.toMatch(stale);
  });

  it('the repository README tells how to hand the controller a locally built frontend', () => {
    expect(readFileSync(resolve(repoDir, 'README.md'), 'utf8')).toMatch(/DPDS_FRONTEND_DIST/);
  });
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
    const start = lines.findIndex((l) => l === `  ${name}:`);
    if (start < 0) return '';
    let end = lines.findIndex((l, i) => i > start && /^ {2}[\w-]+:\s*$/.test(l));
    if (end < 0) end = lines.length;
    return lines.slice(start, end).join('\n');
  }
  /** The job ids: the 2-space indented keys under `jobs:`. */
  const jobIds = (() => {
    const lines = ci.split('\n');
    const start = lines.findIndex((l) => l === 'jobs:');
    return lines
      .slice(start + 1)
      .map((l) => /^ {2}([\w-]+):\s*$/.exec(l)?.[1])
      .filter((id): id is string => id !== undefined);
  })();

  it('has the jobs fmt, rust, frontend and packaging and no frontend-asset-* job', () => {
    expect(jobIds).toEqual(expect.arrayContaining(['fmt', 'rust', 'frontend', 'packaging']));
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

  it('the frontend job runs npm ci, then the typecheck, the tests and npm run build, in frontend/', () => {
    const frontend = job('frontend');
    expect(frontend).not.toBe('');
    expect(frontend).toMatch(/working-directory:\s*frontend/);
    expect(frontend).toMatch(/npm run build/);
    const at = (re: RegExp) => frontend.search(re);
    expect(at(/npm ci/)).toBeGreaterThanOrEqual(0);
    expect(at(/npm ci/)).toBeLessThan(at(/npm run build/));
    expect(frontend).toMatch(/tsc --noEmit/);
    expect(frontend).toMatch(/vitest run/);
  });

  it('the packaging job still runs the dpds.sh tests', () => {
    expect(job('packaging')).toMatch(/bash packaging\/tests\/dpds_sh\.test\.sh/);
  });

  it('the DPDK comment names the new minimum, not 25.11 (user, 2026-11-03)', () => {
    expect(ci).not.toMatch(/needs\s*>=\s*25\.11/);
  });

  it('the workflow has no Docker left', () => {
    expect(ci).not.toMatch(/docker/i);
  });
});

describe('README.md and frontend/README.md: dpds.sh builds the frontend with Node on the host (spec v15)', () => {
  const readme = readFileSync(resolve(repoDir, 'README.md'), 'utf8');
  const paragraphs = readme.split(/\n\s*\n/).map((p) => p.replace(/\s+/g, ' '));
  const enginesMin = (JSON.parse(read('package.json')) as { engines: { node: string } }).engines.node.replace(/^>=\s*/, '');

  it('the engines.node minimum is written as >=X.Y (the README and dpds.sh quote it)', () => {
    expect(enginesMin).toMatch(/^\d+\.\d+/);
  });

  it('a paragraph on dpds.sh names Node with the minimum version and npm', () => {
    const para = paragraphs.find((p) => /dpds\.sh/.test(p) && /\bNode\b/.test(p) && /\bnpm\b/.test(p) && p.includes(enginesMin));
    expect(para, `no paragraph about dpds.sh, Node ${enginesMin} and npm`).toBeDefined();
  });

  it('says that npm ci needs network access to the npm registry', () => {
    expect(readme).toMatch(/npm registry/i);
  });

  it('says that dpds.sh runs npm ci and npm run build before it builds the controller', () => {
    const para = paragraphs.find(
      (p) => /dpds\.sh/.test(p) && /npm ci/.test(p) && /npm run build/.test(p) && !/download|skip/i.test(p),
    );
    expect(para, 'no paragraph about dpds.sh running npm ci and npm run build (and not about skipping a download)').toBeDefined();
    expect(para!.indexOf('npm ci')).toBeLessThan(para!.indexOf('npm run build'));
  });

  it('has no release-asset, download or trust-model text', () => {
    expect(readme).not.toMatch(
      /frontend-builds|frontend-asset|DPDS_FRONTEND_ASSET_URL|\.sha256|sha256sum|release asset|trust model|CI-built|dpds-frontend-<tree>|\.tar\.gz|frontend asset/i,
    );
  });

  it('no longer claims that the host needs no Node or lists curl and tar as requirements', () => {
    expect(readme).not.toMatch(/no Node on the host/i);
    expect(readme).not.toMatch(/`curl`, `sha256sum` and `tar`/);
  });

  it('the DPDK requirement is at least 26.07 and 25.11 is gone (user, 2026-11-03)', () => {
    expect(readme).not.toMatch(/25\.11/);
    const line = readme.split('\n').find((l) => /\*\*DPDK /.test(l)) ?? '';
    expect(line).toMatch(/at least 26\.07/);
  });

  it('frontend/README.md no longer says that CI builds an asset that dpds.sh downloads', () => {
    expect(read('README.md')).not.toMatch(/release asset|download|uploads it/i);
  });

  it('frontend/README.md says that dpds.sh install builds the frontend', () => {
    const paras = read('README.md').split(/\n\s*\n/).map((p) => p.replace(/\s+/g, ' '));
    const para = paras.find((p) => /dpds\.sh install/.test(p) && /\bbuilds?\b/i.test(p) && /\b(Node|npm)\b/.test(p));
    expect(para, 'no paragraph about dpds.sh install building the frontend with Node or npm').toBeDefined();
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

  it('the README says a plain cargo build embeds whatever frontend/dist holds, which may be stale', () => {
    const readme = readFileSync(resolve(repoDir, 'README.md'), 'utf8');
    const paragraphs = readme.split(/\n\s*\n/).map((p) => p.replace(/\s+/g, ' '));
    const para = paragraphs.find((p) => /cargo build/.test(p) && /frontend\/dist/.test(p) && /stale|outdated|whatever/i.test(p));
    expect(para, 'no paragraph about cargo build, frontend/dist and stale content').toBeDefined();
    expect(para).toMatch(/dpds\.sh[^.]*\b(explicit|always)\b|\b(explicit|always)\b[^.]*dpds\.sh/i);
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
