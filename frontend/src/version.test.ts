// The release version lives in VERSION.md at the repository root (first line, X.Y.Z); the frontend's
// package.json and package-lock.json must carry the same one. The Rust side checks the crates, Cargo.lock and the
// README badge (crates/dpds-controller/tests/integration/release.rs). Plain file reads: frontend/ is the directory
// vitest runs in, its parent is the repository root.
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const frontendDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const repoDir = resolve(frontendDir, '..');
const versionMd = () => readFileSync(resolve(repoDir, 'VERSION.md'), 'utf8');
const firstLine = () => versionMd().split('\n')[0];
const json = (...p: string[]) => JSON.parse(readFileSync(resolve(frontendDir, ...p), 'utf8'));

describe('the release version (spec 18)', () => {
  it('VERSION.md starts with 1.0.0 on its own LF-terminated first line', () => {
    expect(firstLine()).toMatch(/^\d+\.\d+\.\d+$/);
    expect(firstLine()).toBe('1.0.0');
  });

  it('package.json has the version of VERSION.md', () => {
    expect(json('package.json').version).toBe(firstLine());
  });

  it('package-lock.json has it at the top and for the root package', () => {
    const lock = json('package-lock.json');
    expect(lock.version).toBe(firstLine());
    expect(lock.packages[''].version).toBe(firstLine());
  });
});
