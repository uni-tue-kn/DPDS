/**
 * At most `limit` items, evenly spaced by position with the first and the last kept, like P4TG's `select_time_keys`: all of
 * them when there are at most `limit`, the first when `limit` is 1, else the positions floor(j * (n - 1) / (limit - 1)).
 */
export function selectTimeKeys<T>(items: readonly T[], limit: number): T[] {
  const n = items.length;
  if (n <= limit) return [...items];
  if (limit <= 1) return items.slice(0, 1);
  return Array.from({ length: limit }, (_, j) => items[Math.floor((j * (n - 1)) / (limit - 1))] as T);
}
