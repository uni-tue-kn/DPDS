import { selectTimeKeys } from './selectTimeKeys';

// Evenly spaced by position, first and last kept, like P4TG's `select_time_keys`: the positions floor(j * (n - 1) / (limit - 1)).
const range = (n: number) => Array.from({ length: n }, (_, i) => i);

describe('selectTimeKeys', () => {
  it('keeps everything up to `limit` items, also none or one', () => {
    expect(selectTimeKeys(range(5), 100)).toEqual(range(5));
    expect(selectTimeKeys(range(100), 100)).toEqual(range(100));
    expect(selectTimeKeys([], 100)).toEqual([]);
    expect(selectTimeKeys(['only'], 1)).toEqual(['only']);
  });

  it.each([
    [10, 1, [0]],
    [10, 2, [0, 9]],
    [5, 3, [0, 2, 4]],
    [10, 4, [0, 3, 6, 9]],
    [8, 3, [0, 3, 7]], // 3.5 rounds down
  ])('n = %i, limit %i: positions %j', (n, limit, positions) => {
    expect(selectTimeKeys(range(n), limit)).toEqual(positions);
  });

  it('at 100 points of a long series it keeps the first and the last and selects without averaging', () => {
    for (const n of [101, 250, 86400]) {
      const got = selectTimeKeys(range(n), 100);
      expect(got).toHaveLength(100);
      expect([got[0], got[99]]).toEqual([0, n - 1]);
      expect(got).toEqual([...got].sort((a, b) => a - b));
    }
    expect(selectTimeKeys(range(250), 100).slice(0, 6)).toEqual([0, 2, 5, 7, 10, 12]);
  });

  it('does not change its input', () => {
    const items = range(250);
    selectTimeKeys(items, 100);
    expect(items).toEqual(range(250));
  });
});
