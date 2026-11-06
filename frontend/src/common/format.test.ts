// The formatters match P4TG's SendReceiveMonitor output (1000-based, unit chosen before rounding); expected strings are literals.
import {
  addNullable,
  formatBits,
  formatCount,
  formatDuration,
  formatFrameCount,
  formatPacketRate,
  l1Bps,
  l1Gbps,
  linkLabel,
  secondsToTime,
} from './format';

describe('P4TG formatters', () => {
  it.each([
    [0, '0 Bit/s'],
    [-5, '0 Bit/s'],
    [999, '999 Bit/s'],
    [1500, '1.5 Kbit/s'],
    [12_345_678, '12.35 Mbit/s'],
    [9.25e9, '9.25 Gbit/s'],
    [19e9, '19 Gbit/s'],
    [1e12, '1 Tbit/s'],
  ])('formatBits(%s) = "%s"', (bits, text) => {
    expect(formatBits(bits)).toBe(text);
  });

  it.each([
    [0, '0 Pps'],
    [500, '500 Pps'],
    [200_000, '200 Kpps'],
    [7.31e6, '7.31 Mpps'],
    [2e9, '2 Gpps'],
  ])('formatPacketRate(%s) = "%s"', (packets, text) => {
    expect(formatPacketRate(packets)).toBe(text);
  });

  it.each([
    [0, '0'],
    [5, '5.00 '], // the unit is empty
    [7770, '7.77 K'],
    [12_345_678, '12.35 M'],
    [2.5e9, '2.50 B'],
  ])('formatFrameCount(%s) = "%s"', (packets, text) => {
    expect(formatFrameCount(packets)).toBe(text);
  });

  it('the second argument is the number of decimals', () => {
    expect(formatBits(1234, 0)).toBe('1 Kbit/s');
    expect(formatBits(1234, 3)).toBe('1.234 Kbit/s');
    expect(formatFrameCount(7770, 0)).toBe('8 K');
  });

  it.each([
    [0, '0h 0m 0s'],
    [65, '0h 1m 5s'],
    [3661, '1h 1m 1s'],
    [90_000, '25h 0m 0s'], // hours do not wrap at a day
    [59.9, '0h 0m 59s'], // fractions are cut off
  ])('secondsToTime(%s) = "%s"', (seconds, text) => {
    expect(secondsToTime(seconds)).toBe(text);
  });
});

describe('rates and labels', () => {
  it('L1 is L2 plus 20 bytes per frame', () => {
    expect(l1Bps(8e9, 1e6)).toBe(8e9 + 1e6 * 160);
    expect(l1Gbps(8, 1)).toBeCloseTo(8.16, 12);
  });

  it('durations pick their own unit, counts have thousands separators, null is n/a', () => {
    expect(formatDuration(500)).toBe('500.00 ns');
    expect(formatDuration(2500)).toBe('2.50 µs');
    expect(formatDuration(1_050_000)).toBe('1.05 ms');
    expect(formatDuration(3e9)).toBe('3.00 s');
    expect(formatDuration(null)).toBe('n/a');
    expect(formatCount(98_765_432_100)).toBe('98,765,432,100');
    expect(formatCount(undefined)).toBe('n/a');
  });

  it('a link is "ingress → egress" with ports through the labeler and "?" for what is missing', () => {
    expect(linkLabel('0000:65:00.0', '0000:65:00.1', (p) => `net-${p?.slice(-1)}`)).toBe('net-0 → net-1');
    expect(linkLabel(null, undefined)).toBe('? → ?');
  });

  it('a count that is null is the sum of the others, null only when both are', () => {
    expect(addNullable(null, null)).toBeNull();
    expect(addNullable(null, 3)).toBe(3);
    expect(addNullable(2, null)).toBe(2);
    expect(addNullable(2, 3)).toBe(5);
  });
});
