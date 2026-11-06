import { expect } from 'vitest';
import type { HistogramView } from '../api/types';

/** A histogram bar is bin count / total recorded (bins + under + overflow) * 100. */
export function percent(h: Pick<HistogramView, 'bins' | 'underflow' | 'overflow'>): number[] {
  const total = h.bins.reduce((a, b) => a + b, 0) + h.underflow + h.overflow;
  return h.bins.map((b) => (total === 0 ? 0 : (b / total) * 100));
}

/** The chart's bar values are the probabilities of `h` (compared to 9 digits: the order of the float operations is free). */
export function expectBars(actual: number[], h: Pick<HistogramView, 'bins' | 'underflow' | 'overflow'>): void {
  const want = percent(h);
  expect(actual).toHaveLength(want.length);
  want.forEach((w, i) => {
    expect(actual[i], `bar ${i}`).toBeCloseTo(w, 9);
  });
}
