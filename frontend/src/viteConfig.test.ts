// The Vite dev server proxies /api to a running dpds-controller (spec v13: the controller serves the built
// frontend itself, so there is no nginx proxy any more), and the build output is what the controller embeds
// and caches. The config is loaded in a separate node process with Vite's own loader, like `vite` does at
// start-up (it bundles vite.config.ts with esbuild, which does not work inside the jsdom test environment).
import { execFileSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const frontendDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');

type Proxy = string | { target?: string };
type Loose = {
  base?: string;
  build?: { outDir?: string; assetsDir?: string };
  server?: { proxy?: Record<string, Proxy> };
};

const LOADER = `
import { loadConfigFromFile } from 'vite';
const r = await loadConfigFromFile({ command: 'serve', mode: 'development' }, 'vite.config.ts', process.cwd());
const c = r.config;
process.stdout.write(JSON.stringify({ base: c.base, build: c.build, server: c.server }));
`;

/** The config as `vite` sees it, with DPDS_CONTROLLER set to `env` (unset for undefined). */
function loadConfig(env?: string): Loose {
  const childEnv: NodeJS.ProcessEnv = { ...process.env };
  delete childEnv.DPDS_CONTROLLER;
  if (env !== undefined) childEnv.DPDS_CONTROLLER = env;
  const out = execFileSync(process.execPath, ['--input-type=module', '-e', LOADER], {
    cwd: frontendDir,
    env: childEnv,
    encoding: 'utf8',
  });
  return JSON.parse(out) as Loose;
}

const target = (cfg: Loose) => {
  const p = cfg.server?.proxy?.['/api'];
  return typeof p === 'string' ? p : p?.target;
};

describe('vite.config.ts (dev server and build output)', () => {
  it('proxies /api to the controller on its default port 8100 on this host', () => {
    expect(target(loadConfig())).toBe('http://127.0.0.1:8100');
  });

  it('DPDS_CONTROLLER (scheme://host:port) overrides the proxy target', () => {
    expect(target(loadConfig('http://10.1.2.3:9000'))).toBe('http://10.1.2.3:9000');
  });

  it('proxies only /api', () => {
    expect(Object.keys(loadConfig().server?.proxy ?? {})).toEqual(['/api']);
  });

  it('builds into dist/ with hashed files under /assets/ at the root, which is what the controller caches', () => {
    const cfg = loadConfig();
    expect(cfg.build?.outDir ?? 'dist').toBe('dist');
    expect(cfg.build?.assetsDir ?? 'assets').toBe('assets');
    expect(cfg.base ?? '/').toBe('/');
  });
});
