// The nine loss causes (donut segments and Packet loss chart lines) use clearly different colours: every pair is at least 60 apart
// in RGB (Euclidean, 0-255 per channel).
import { CAUSES } from './LossDonut';

const rgb = (hex: string) => [1, 3, 5].map((i) => Number.parseInt(hex.slice(i, i + 2), 16));
const distance = (a: string, b: string) => Math.hypot(...rgb(a).map((v, i) => v - (rgb(b)[i] ?? 0)));

describe('loss cause colours', () => {
  it('are nine #rrggbb colours', () => {
    expect(CAUSES).toHaveLength(9);
    for (const c of CAUSES) expect(c.color).toMatch(/^#[0-9a-f]{6}$/i);
  });

  it('keep the soft palette the user liked: the first three as before, one red only', () => {
    expect(CAUSES.map((c) => c.color)).toEqual([
      '#6c8ebf',
      '#f6b73c',
      '#e67e22',
      '#c0392b',
      '#a29bfe',
      '#fd79a8',
      '#00cec9',
      '#55efc4',
      '#636e72',
    ]);
  });

  it('every pair differs clearly (RGB distance >= 60)', () => {
    for (let i = 0; i < CAUSES.length; i++) {
      for (let j = i + 1; j < CAUSES.length; j++) {
        const [a, b] = [CAUSES[i], CAUSES[j]];
        if (!a || !b) throw new Error('cause missing');
        expect(distance(a.color, b.color), `${a.label} vs ${b.label}`).toBeGreaterThanOrEqual(60);
      }
    }
  });
});
