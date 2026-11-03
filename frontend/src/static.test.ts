// Static checks of how the frontend is delivered (spec v13) and of leftovers in the source tree (F6).
// The controller embeds `dist/` and serves it, CI builds it as a release asset, and the Vite dev server
// proxies /api to a running controller (src/viteConfig.test.ts). These tests read the files as text from the repository
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

describe('.github/workflows/ci.yml: frontend-asset-build and frontend-asset-publish (spec v13, v13b item 1)', () => {
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
  /** Shell lines of a job with `\`-continued lines joined and comment-only lines dropped. */
  const logical = (text: string) =>
    text
      .replace(/\\\n\s*/g, ' ')
      .split('\n')
      .filter((l) => !/^\s*#/.test(l))
      .join('\n');
  /** Every `gh release <sub> ...` command up to the next `|`, `&`, `;`, `>` or end of line. */
  const ghReleaseCommands = (text: string, sub?: string) =>
    Array.from(logical(text).matchAll(/\bgh release (\w+)[^\n|&;>]*/g))
      .filter((m) => !sub || m[1] === sub)
      .map((m) => m[0]);

  const build = job('frontend-asset-build');
  const publish = job('frontend-asset-publish');

  it('the old single frontend-asset job is split in two', () => {
    expect(build).not.toBe('');
    expect(publish).not.toBe('');
    expect(job('frontend-asset')).toBe('');
  });

  it('build runs after the frontend job, publish after build; both on pushes to nightly and main only', () => {
    expect(build).toMatch(/^\s+needs:.*\bfrontend\b|^\s+needs:\s*\n(\s+-\s*[\w-]+\s*\n)*\s+-\s*frontend\b/m);
    expect(publish).toMatch(
      /^\s+needs:\s*\[?\s*frontend-asset-build\b|^\s+needs:\s*\n(\s+-\s*[\w-]+\s*\n)*\s+-\s*frontend-asset-build\b/m,
    );
    for (const [name, text] of [['build', build], ['publish', publish]]) {
      const cond = (text.match(/^\s+if:\s*(.+)$/m) ?? [])[1] ?? '';
      expect(cond, `${name} needs an if`).toMatch(/push/);
      expect(cond).toMatch(/nightly/);
      expect(cond).toMatch(/\bmain\b/);
      expect(cond).not.toMatch(/master/);
    }
  });

  it('only publish may write: contents: write once in the workflow, in publish; build and the workflow default read', () => {
    expect(ci.match(/contents:\s*write/g)).toHaveLength(1);
    expect(publish).toMatch(/permissions:\s*\n\s+contents:\s*write/);
    expect(build).not.toMatch(/contents:\s*write/);
    expect(ci).toMatch(/^permissions:\s*\n\s+contents:\s*read/m);
  });

  it('build checks out, builds with npm ci and npm run build, packages tar + sha256 and uploads an artifact', () => {
    expect(build).toMatch(/actions\/checkout@/);
    expect(build).toMatch(/npm ci/);
    expect(build).toMatch(/npm run build/);
    expect(build).toMatch(/git rev-parse HEAD:frontend/);
    expect(build).toMatch(/dpds-frontend-/);
    expect(build).toMatch(/\.tar\.gz/);
    expect(build).toMatch(/\btar\b[^\n]*dist/);
    expect(build).toMatch(/sha256sum/);
    expect(build).toMatch(/\.sha256/);
    expect(build).toMatch(/actions\/upload-artifact@v\d+\b/);
  });

  it('build holds no token and never talks to the release', () => {
    expect(build).not.toMatch(/GH_TOKEN|GITHUB_TOKEN/);
    expect(build).not.toMatch(/\bgh\s/);
    expect(build).not.toMatch(/--clobber/);
  });

  it('publish has no checkout and no npm: it only moves the artifact to the release', () => {
    expect(publish).not.toMatch(/actions\/checkout/);
    expect(publish).not.toMatch(/\bnpm\b/);
    expect(publish).not.toMatch(/setup-node/);
    expect(publish).toMatch(/actions\/download-artifact@v\d+\b/);
  });

  it('upload-artifact and download-artifact are pinned to a major version', () => {
    expect(ci).toMatch(/uses:\s*actions\/upload-artifact@v\d+\s*$/m);
    expect(ci).toMatch(/uses:\s*actions\/download-artifact@v\d+\s*$/m);
  });

  it('publish uses gh with the GITHUB_TOKEN as GH_TOKEN and names the repository on every release command', () => {
    expect(publish).toMatch(/GH_TOKEN:\s*\$\{\{\s*secrets\.GITHUB_TOKEN\s*\}\}/);
    const cmds = ghReleaseCommands(publish);
    expect(cmds.length).toBeGreaterThanOrEqual(3);
    for (const c of cmds) expect(c, c).toMatch(/--repo\s+"?\$\{?GITHUB_REPOSITORY\}?"?/);
  });

  it('publish uses the frontend-builds prerelease, created as view || create --prerelease || view, never a tag named nightly', () => {
    expect(publish).toMatch(/frontend-builds/);
    expect(publish).toMatch(/--prerelease/);
    const text = logical(publish);
    expect(text).toMatch(/gh release view[^\n]*\|\|\s*gh release create[^\n]*--prerelease[^\n]*\|\|\s*gh release view/);
    expect(ci).not.toMatch(/gh release \w+\s+['"]?nightly\b/);
    expect(ci).not.toMatch(/tag_name:\s*nightly/);
  });

  it('asks the public download URL whether each file exists (curl -fsI), not the asset list of gh release view', () => {
    const text = logical(publish);
    expect(text).toMatch(/curl\s+-(?=[a-zA-Z]*f)(?=[a-zA-Z]*I)[a-zA-Z]+/);
    expect(text).toMatch(/github\.com\/\$\{?GITHUB_REPOSITORY\}?\/releases\/download\/frontend-builds\b/);
    // `gh release view --json assets` lists at most about 100 assets
    expect(publish).not.toMatch(/--json\s+assets/);
    expect(publish).not.toMatch(/\.assets\[\]/);
  });

  it('skips what is already there', () => {
    expect(publish).toMatch(/already|exist/i);
  });

  it('--clobber is used in publish only, on every upload', () => {
    expect(ci.match(/--clobber/g)?.length ?? 0).toBeGreaterThan(0);
    expect(ci.match(/--clobber/g)?.length).toBe(publish.match(/--clobber/g)?.length);
    for (const c of ghReleaseCommands(publish, 'upload')) expect(c, c).toMatch(/--clobber/);
  });

  /** The files one `gh release upload` command uploads: its arguments minus the tag and options. */
  function uploadedFiles(cmd: string): string[] {
    const toks = cmd.split(/\s+/).slice(3); // after: gh release upload
    const files: string[] = [];
    for (let i = 0; i < toks.length; i++) {
      if (toks[i] === '--repo') i++;
      else if (!toks[i].startsWith('-')) files.push(toks[i]);
    }
    return files.slice(1); // the first operand is the release tag
  }

  it('the tarball and its .sha256 are uploaded by separate gh release upload commands, the tarball first', () => {
    const uploads = ghReleaseCommands(publish, 'upload');
    expect(uploads.length).toBeGreaterThanOrEqual(2);
    for (const c of uploads) expect(uploadedFiles(c), `one file per upload: ${c}`).toHaveLength(1);
    const isSha = (c: string) => /\.sha256/.test(c);
    // somewhere a tarball upload is directly followed by the .sha256 upload; one upload call
    // would send both at once, so the checksum file is no proof that the tarball is complete
    const pairs = uploads.filter((c, i) => !isSha(c) && uploads[i + 1] !== undefined && isSha(uploads[i + 1]));
    expect(pairs.length).toBeGreaterThanOrEqual(1);
    // a .sha256-only upload (repair of a half-uploaded asset) is a third command
    expect(uploads.filter(isSha).length).toBeGreaterThanOrEqual(2);
  });

  it('the workflow has no Docker left', () => {
    expect(ci).not.toMatch(/docker/i);
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
