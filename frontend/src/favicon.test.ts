// The favicon is the DPDS logo (src/assets/logo.png, 256x97) centred on a transparent square, so a browser tab does not squash it;
// Vite copies public/ into dist, which the controller serves as is.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const root = resolve(__dirname, '..');

/** Width and height from a PNG's IHDR chunk. */
function pngSize(path: string): [number, number] {
  const b = readFileSync(path);
  expect(b.subarray(1, 4).toString('ascii')).toBe('PNG');
  return [b.readUInt32BE(16), b.readUInt32BE(20)];
}

describe('favicon', () => {
  it('index.html links /favicon.png as a PNG icon', () => {
    const html = readFileSync(resolve(root, 'index.html'), 'utf8');
    expect(html).toMatch(/<link rel="icon" type="image\/png" href="\/favicon\.png" \/>/);
  });

  it('public/favicon.png is a square PNG at least as wide as the logo', () => {
    const [w, h] = pngSize(resolve(root, 'public/favicon.png'));
    const [logoW] = pngSize(resolve(root, 'src/assets/logo.png'));
    expect(w).toBe(h);
    expect(w).toBeGreaterThanOrEqual(logoW);
  });
});
