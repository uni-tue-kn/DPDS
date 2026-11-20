/** Recursively drops null / undefined object members ("null or absent" are the same optional value). */
export function dropNulls<T>(v: T): T {
  if (Array.isArray(v)) return v.map((x) => dropNulls(x)) as T;
  if (v !== null && typeof v === 'object') {
    return Object.fromEntries(
      Object.entries(v as Record<string, unknown>)
        .filter(([, x]) => x !== null && x !== undefined)
        .map(([k, x]) => [k, dropNulls(x)]),
    ) as T;
  }
  return v;
}
