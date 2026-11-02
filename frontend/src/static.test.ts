// Static checks of the container packaging (F7) and of leftovers in the source tree (F6). They read the files as
// text from the repository (frontend/ = the directory vitest runs in, its parent holds docker-compose.yml);
// nothing is built or started.
import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const frontendDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const read = (...p: string[]) => readFileSync(resolve(frontendDir, ...p), 'utf8');
/** Comment-free text (`#` comments in Dockerfile/compose, `#` comments in nginx.conf) so prose cannot satisfy a check. */
const code = (t: string) => t.split('\n').filter((l) => !l.trim().startsWith('#')).join('\n');

/** Bodies of every `location ... { ... }` block as [header, body] (no nested braces in our config). */
function locations(conf: string): [string, string][] {
  // `${VAR}` template placeholders contain braces; hide them while splitting into blocks.
  const hidden = code(conf).replace(/\$\{(\w+)\}/g, '<<$1>>');
  return Array.from(hidden.matchAll(/location\s+([^{]+?)\s*\{([^}]*)\}/g)).map((m) => [
    m[1], m[2].replace(/<<(\w+)>>/g, '${$1}'),
  ]);
}

describe('frontend/Dockerfile (F7)', () => {
  const dockerfile = code(read('Dockerfile'));
  const froms = Array.from(dockerfile.matchAll(/^\s*FROM\s+(\S+)(?:\s+AS\s+(\S+))?/gim)).map((m) => m[1]);

  it('has a build stage and a runtime stage', () => {
    expect(froms).toHaveLength(2);
  });

  it.each([0, 1])('base image #%i is pinned to a version (no bare major, no latest/stable)', (i) => {
    const [name, tag] = froms[i].split(':');
    expect(tag, `${froms[i]} has no tag`).toBeDefined();
    expect(tag).toMatch(/^\d+\.\d+/); // at least major.minor, e.g. 22.12-alpine or 1.27-alpine
    expect(tag).not.toMatch(/latest|stable|mainline/);
    expect(name).toBeTruthy();
  });

  it('builds with node 22 on alpine, pinned', () => {
    expect(froms[0]).toMatch(/^node:22\.\d+(\.\d+)?-alpine/);
    expect(froms[0]).not.toBe('node:22');
  });

  it('serves with the unprivileged nginx image (alpine, pinned), not nginx:stable-alpine', () => {
    expect(froms[1]).toMatch(/^nginxinc\/nginx-unprivileged:\d+\.\d+(\.\d+)?-alpine/);
    expect(dockerfile).not.toMatch(/nginx:stable/);
  });

  it('does not switch to root in the runtime stage', () => {
    const runtime = dockerfile.slice(dockerfile.lastIndexOf('FROM'));
    expect(runtime).not.toMatch(/^\s*USER\s+(root|0)\b/im);
  });

  it('defaults DPDS_FRONTEND_BIND to 0.0.0.0 and still sets the port and controller defaults', () => {
    expect(dockerfile).toMatch(/DPDS_FRONTEND_BIND\s*=\s*0\.0\.0\.0/);
    expect(dockerfile).toMatch(/DPDS_FRONTEND_PORT\s*=\s*\d+/);
    expect(dockerfile).toMatch(/DPDS_CONTROLLER\s*=\s*http:\/\/[^\s/]+:\d+(\s|$)/);
  });

  it('installs the nginx config as a template (envsubst renders it at start)', () => {
    expect(dockerfile).toMatch(/COPY\s+nginx\.conf\s+\/etc\/nginx\/templates\/default\.conf\.template/);
  });
});

describe('frontend/nginx.conf (F7)', () => {
  const raw = read('nginx.conf');
  const conf = code(raw);

  it('listens on ${DPDS_FRONTEND_BIND}:${DPDS_FRONTEND_PORT}', () => {
    expect(conf).toMatch(/^\s*listen\s+\$\{DPDS_FRONTEND_BIND\}:\$\{DPDS_FRONTEND_PORT\}\s*;/m);
  });

  it('no other listen directive (no bare port that ignores the bind address)', () => {
    const listens = Array.from(conf.matchAll(/^\s*listen\s+([^;]+);/gm)).map((m) => m[1].trim());
    expect(listens).toEqual(['${DPDS_FRONTEND_BIND}:${DPDS_FRONTEND_PORT}']);
  });

  it('forwards the browser Host to the controller (DNS-rebinding guard) in the /api/ location', () => {
    const api = locations(conf).find(([h]) => /\/api\//.test(h));
    expect(api, 'location /api/').toBeDefined();
    expect(api?.[1]).toMatch(/proxy_set_header\s+Host\s+\$host\s*;/);
    expect(api?.[1]).toMatch(/proxy_pass\s+\$\{DPDS_CONTROLLER\}\s*;/);
  });

  it('serves index.html with Cache-Control: no-cache', () => {
    const index = locations(conf).find(([h]) => /index\.html/.test(h));
    expect(index, 'a location matching index.html').toBeDefined();
    expect(index?.[1]).toMatch(/Cache-Control\s+"?[^;"]*no-cache/i);
  });

  it('caches the hashed assets for a long time', () => {
    const assets = locations(conf).find(([h]) => /assets/.test(h));
    expect(assets, 'a location for /assets/').toBeDefined();
    expect(assets?.[1]).toMatch(/expires\s+(\d+[hdwMy]|max)\s*;|max-age=\d{6,}|immutable/);
    expect(assets?.[1]).not.toMatch(/no-cache|no-store/i);
  });

  it('compresses js, css and json with gzip', () => {
    expect(conf).toMatch(/^\s*gzip\s+on\s*;/m);
    const types = (conf.match(/^\s*gzip_types\s+([^;]+);/m) ?? [])[1] ?? '';
    expect(types).toMatch(/javascript/);
    expect(types).toMatch(/text\/css/);
    expect(types).toMatch(/application\/json/);
  });

  it('keeps the SPA fallback to /index.html', () => {
    const root = locations(conf).find(([h]) => h.trim() === '/');
    expect(root?.[1]).toMatch(/try_files\s+\$uri\s+\$uri\/\s+\/index\.html\s*;/);
  });

  it('documents that DPDS_CONTROLLER is http://host:port without a path', () => {
    const docs = `${raw}\n${read('Dockerfile')}\n${read('README.md')}`;
    expect(docs).toMatch(/without a path/i);
  });
});

describe('docker-compose.yml (F7)', () => {
  const compose = code(read('..', 'docker-compose.yml'));

  it('passes DPDS_FRONTEND_BIND (default 0.0.0.0) into the frontend container', () => {
    expect(compose).toMatch(/DPDS_FRONTEND_BIND\s*[:=]\s*"?\$\{DPDS_FRONTEND_BIND:-0\.0\.0\.0\}"?/);
  });

  it('still passes the port and controller URL', () => {
    expect(compose).toMatch(/DPDS_FRONTEND_PORT\s*[:=]\s*"?\$\{DPDS_FRONTEND_PORT:-\d+\}/);
    expect(compose).toMatch(/DPDS_CONTROLLER\s*[:=]\s*"?\$\{DPDS_CONTROLLER:-http:\/\/127\.0\.0\.1:\d+\}/);
  });

  it('keeps host networking (the controller listens on the host)', () => {
    expect(compose).toMatch(/network_mode:\s*host/);
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
