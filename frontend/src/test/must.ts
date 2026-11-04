/** Narrows `T | null | undefined` to `T` in tests, failing with a readable message instead of a non-null assertion. */
export function must<T>(value: T | null | undefined, what = 'value'): T {
  if (value === null || value === undefined) throw new Error(`expected ${what} to be present, got ${String(value)}`);
  return value;
}
