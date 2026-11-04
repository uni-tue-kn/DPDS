// `npm run build` must not warn "Some chunks are larger than 500 kB after minification". The warning is fixed
// by splitting the bundle (the controller embeds frontend/dist and serves it, so the result stays a static
// index.html plus hashed assets), never by raising or hiding the limit. The production build runs in a child
// node process, like viteConfig.test.ts does (vite bundles its config with esbuild/rolldown, which does not
// work inside the jsdom test environment), into a temporary outDir so the repo's dist/ is untouched.
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readdirSync, readFileSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { must } from './test/must';

const frontendDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const viteBin = resolve(frontendDir, 'node_modules/vite/bin/vite.js');

/** Vite's own threshold: 500 kB = 500 * 1000 bytes (not KiB). */
const LIMIT_BYTES = 500 * 1000;

/** Every file below `dir`, as paths relative to it, with '/' separators. */
function listFiles(dir: string, prefix = ''): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
    e.isDirectory() ? listFiles(join(dir, e.name), `${prefix}${e.name}/`) : [`${prefix}${e.name}`],
  );
}

describe('production build size', () => {
  let outDir = '';
  let log = '';
  let status: number | null = null;

  beforeAll(() => {
    outDir = mkdtempSync(join(tmpdir(), 'dpds-bundle-size-'));
    const env: NodeJS.ProcessEnv = { ...process.env, NO_COLOR: '1', FORCE_COLOR: '0' };
    // vitest sets NODE_ENV=test; the production build must see what `npm run build` sees.
    delete env.NODE_ENV;
    delete env.VITEST;
    const r = spawnSync(process.execPath, [viteBin, 'build', '--outDir', outDir, '--emptyOutDir'], {
      cwd: frontendDir,
      env,
      encoding: 'utf8',
      maxBuffer: 64 * 1024 * 1024,
    });
    // The size warning goes to stderr; keep both streams.
    log = `${r.stdout ?? ''}\n${r.stderr ?? ''}${r.error ? `\n${String(r.error)}` : ''}`;
    status = r.status;
  }, 180_000);

  afterAll(() => {
    if (outDir) rmSync(outDir, { recursive: true, force: true });
  });

  it('builds successfully', () => {
    expect(log).toContain('built in');
    expect(status).toBe(0);
  });

  it('prints no "larger than 500 kB" chunk warning', () => {
    expect(log).not.toMatch(/larger than 500\s*kB/i);
    expect(log).not.toMatch(/chunks are larger than/i);
  });

  it('emits no .js file above 500 kB (500 * 1000 bytes, as Vite counts)', () => {
    const js = listFiles(outDir).filter((f) => f.endsWith('.js'));
    expect(js.length).toBeGreaterThan(0);
    const sizes = js.map((f) => ({ file: f, bytes: statSync(join(outDir, f)).size }));
    const tooBig = sizes.filter((s) => s.bytes > LIMIT_BYTES);
    expect(tooBig, `files above ${LIMIT_BYTES} bytes: ${JSON.stringify(tooBig)}`).toEqual([]);
  });

  it('index.html references an existing entry script among the emitted files', () => {
    const html = readFileSync(join(outDir, 'index.html'), 'utf8');
    const tag = must(/<script\b[^>]*\btype="module"[^>]*>/.exec(html), 'a module <script> tag in index.html')[0];
    const src = must(/\bsrc="([^"]+)"/.exec(tag), 'the src of the entry script')[1];
    // Absolute from the root, as the controller serves it (base '/').
    expect(src).toMatch(/^\/assets\/[^/]+\.js$/);
    expect(listFiles(outDir)).toContain(src.slice(1));
  });
});

describe('vite.config.ts does not hide the warning', () => {
  // Comments may mention the option; only code counts.
  const code = readFileSync(resolve(frontendDir, 'vite.config.ts'), 'utf8')
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/^\s*\/\/.*$/gm, '')
    .replace(/\s\/\/.*$/gm, '');

  it('does not set chunkSizeWarningLimit', () => {
    expect(code).not.toMatch(/chunkSizeWarningLimit/);
  });

  it.each(['logLevel', 'customLogger', 'onwarn', 'onLog'])('does not silence build output through %s', (key) => {
    expect(code).not.toMatch(new RegExp(`\\b${key}\\b`));
  });
});
