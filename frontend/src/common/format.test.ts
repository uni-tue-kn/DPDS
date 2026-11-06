// The four formatters of the dashboard's top row, ported exactly from P4TG (`formatFrameCount`, `formatBits`, `formatPacketRate`,
// `secondsToTime`). The expected strings are literals computed with P4TG's own code; they do not call the functions under test.
// Not pinned, because P4TG's code gives nonsense there: values between 0 and 1 and the rounding step at 999.999 -> "1000 ...".
import { formatBits, formatFrameCount, formatPacketRate, secondsToTime } from './format';

describe('formatBits (P4TG): 1000-based, unit chosen before rounding, trailing zeros dropped', () => {
  it.each([
    [0, '0 Bit/s'],
    [-5, '0 Bit/s'],
    [1, '1 Bit/s'],
    [5, '5 Bit/s'],
    [999, '999 Bit/s'],
    [1000, '1 Kbit/s'],
    [1500, '1.5 Kbit/s'],
    [12_345_678, '12.35 Mbit/s'],
    [1e6, '1 Mbit/s'],
    [1.5e9, '1.5 Gbit/s'],
    [9.25e9, '9.25 Gbit/s'],
    [16.5e9, '16.5 Gbit/s'],
    [19e9, '19 Gbit/s'],
    [1e12, '1 Tbit/s'],
  ])('%s -> "%s"', (bits, text) => {
    expect(formatBits(bits)).toBe(text);
  });

  it('the second argument is the number of decimals (default 2)', () => {
    expect(formatBits(1234, 0)).toBe('1 Kbit/s');
    expect(formatBits(1234, 3)).toBe('1.234 Kbit/s');
    expect(formatBits(1234)).toBe('1.23 Kbit/s');
  });
});

describe('formatPacketRate (P4TG): the same rule with Pps, Kpps, Mpps, Gpps, Tpps', () => {
  it.each([
    [0, '0 Pps'],
    [-1, '0 Pps'],
    [1, '1 Pps'],
    [500, '500 Pps'],
    [1000, '1 Kpps'],
    [1500, '1.5 Kpps'],
    [200_000, '200 Kpps'],
    [1_234_567, '1.23 Mpps'],
    [7.31e6, '7.31 Mpps'],
    [2e9, '2 Gpps'],
    [1e12, '1 Tpps'],
  ])('%s -> "%s"', (packets, text) => {
    expect(formatPacketRate(packets)).toBe(text);
  });

  it('the second argument is the number of decimals', () => {
    expect(formatPacketRate(1_234_567, 0)).toBe('1 Mpps');
    expect(formatPacketRate(1_234_567, 3)).toBe('1.235 Mpps');
  });
});

describe('formatFrameCount (P4TG): always the decimals, a unit of "", K, M, B, T', () => {
  it.each([
    [0, '0'],
    [5, '5.00 '], // the unit is empty, so the text ends with a space
    [100, '100.00 '],
    [999, '999.00 '],
    [1000, '1.00 K'],
    [1500, '1.50 K'],
    [7770, '7.77 K'],
    [1_234_567, '1.23 M'],
    [12_345_678, '12.35 M'],
    [1e9, '1.00 B'],
    [2.5e9, '2.50 B'],
    [1e12, '1.00 T'],
  ])('%s -> "%s"', (packets, text) => {
    expect(formatFrameCount(packets)).toBe(text);
  });

  it('the second argument is the number of decimals', () => {
    expect(formatFrameCount(7770, 0)).toBe('8 K');
    expect(formatFrameCount(7770, 3)).toBe('7.770 K');
  });
});

describe('secondsToTime (P4TG): "<h>h <m>m <s>s", hours do not wrap at a day', () => {
  it.each([
    [0, '0h 0m 0s'],
    [5, '0h 0m 5s'],
    [59, '0h 0m 59s'],
    [60, '0h 1m 0s'],
    [65, '0h 1m 5s'],
    [192, '0h 3m 12s'],
    [3599, '0h 59m 59s'],
    [3600, '1h 0m 0s'],
    [3661, '1h 1m 1s'],
    [86_399, '23h 59m 59s'],
    [90_000, '25h 0m 0s'],
  ])('%s s -> "%s"', (seconds, text) => {
    expect(secondsToTime(seconds)).toBe(text);
  });

  it('fractions of a second are cut off, not rounded', () => {
    expect(secondsToTime(59.9)).toBe('0h 0m 59s');
    expect(secondsToTime(65.999)).toBe('0h 1m 5s');
  });
});
