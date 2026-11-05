import { selectTimeKeys } from './selectTimeKeys';

// Spec 50 G (user: the DPDS diagrams look more crowded than P4TG's): at most 100 points per time chart, chosen like P4TG's
// `select_time_keys` (Controller/src/api/statistics.rs): evenly spaced BY POSITION, first and last kept, samples selected and
// never averaged. With n items and limit L: n <= L gives all of them; L == 1 the first; else the positions
// { floor(j * (n - 1) / (L - 1)) | j = 0 .. L - 1 }.
// CHOICE (the spec names no module): `selectTimeKeys<T>(items: readonly T[], limit: number): T[]` in src/common/selectTimeKeys.ts,
// a pure function that returns a new array. The positions below were worked out by hand from the formula, not with the helper.

const range = (n: number) => Array.from({ length: n }, (_, i) => i);

describe('selectTimeKeys (spec 50 G)', () => {
  it('keeps everything when there are at most `limit` items', () => {
    expect(selectTimeKeys(range(5), 100)).toEqual([0, 1, 2, 3, 4]);
    expect(selectTimeKeys(range(100), 100)).toEqual(range(100));
    expect(selectTimeKeys(range(2), 2)).toEqual([0, 1]);
  });

  it('an empty list stays empty, and a single item stays', () => {
    expect(selectTimeKeys([], 100)).toEqual([]);
    expect(selectTimeKeys(['only'], 100)).toEqual(['only']);
  });

  it('limit 1 keeps the first item', () => {
    expect(selectTimeKeys(range(10), 1)).toEqual([0]);
    expect(selectTimeKeys(range(250), 1)).toEqual([0]);
  });

  it('limit 1 with a single item keeps it', () => {
    expect(selectTimeKeys([7], 1)).toEqual([7]);
  });

  it('limit 2 keeps the first and the last', () => {
    expect(selectTimeKeys(range(10), 2)).toEqual([0, 9]);
  });

  it('n = 5, limit 3: positions 0, 2, 4', () => {
    expect(selectTimeKeys(range(5), 3)).toEqual([0, 2, 4]);
  });

  it('n = 10, limit 4: positions 0, 3, 6, 9 (integer division)', () => {
    // j * 9 / 3 = 0, 3, 6, 9
    expect(selectTimeKeys(range(10), 4)).toEqual([0, 3, 6, 9]);
  });

  it('n = 8, limit 3: positions 0, 3, 7 (3.5 rounds down)', () => {
    expect(selectTimeKeys(range(8), 3)).toEqual([0, 3, 7]);
  });

  it('n = 101, limit 100: 100 items, only position 99 is left out', () => {
    const got = selectTimeKeys(range(101), 100);
    expect(got).toHaveLength(100);
    expect(got.slice(0, 4)).toEqual([0, 1, 2, 3]);
    expect(got.slice(-3)).toEqual([97, 98, 100]);
    expect(got).toEqual(range(101).filter((i) => i !== 99));
  });

  it('n = 250, limit 100: floor(j * 249 / 99)', () => {
    const got = selectTimeKeys(range(250), 100);
    expect(got).toHaveLength(100);
    expect(got.slice(0, 6)).toEqual([0, 2, 5, 7, 10, 12]);
    expect([got[49], got[50]]).toEqual([123, 125]);
    expect(got.slice(-2)).toEqual([246, 249]);
  });

  it('the first and the last item are always kept', () => {
    for (const n of [101, 150, 250, 1000, 86400]) {
      const got = selectTimeKeys(range(n), 100);
      expect(got, `n = ${n}`).toHaveLength(100);
      expect(got[0]).toBe(0);
      expect(got[99]).toBe(n - 1);
    }
  });

  it('selects, in order and without repeats; it does not average or copy values', () => {
    const items = range(250).map((i) => ({ i }));
    const got = selectTimeKeys(items, 100);
    expect(got.every((g) => items.includes(g))).toBe(true);
    expect(got.map((g) => g.i)).toEqual([...new Set(got.map((g) => g.i))].sort((a, b) => a - b));
  });

  it('does not change its input', () => {
    const items = range(250);
    selectTimeKeys(items, 100);
    expect(items).toEqual(range(250));
  });
});
