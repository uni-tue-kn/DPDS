// Units of the config form, modelled on P4TG's histogram dialog. The API and the stored values stay in the base unit
// (ns, bit/s, bit, pps); the form shows a number plus a unit. All conversions shift the decimal point of the text, so
// 2.5 µs is exactly 2500 ns and never 2499.9999999999995.

export interface Unit {
  label: string;
  /** Power of ten of the base unit: 1 unit = 10^exp base units. */
  exp: number;
}

export const TIME_UNITS: Unit[] = [
  { label: 'ns', exp: 0 },
  { label: 'µs', exp: 3 },
  { label: 'ms', exp: 6 },
  { label: 's', exp: 9 },
];
export const RATE_UNITS: Unit[] = [
  { label: 'bit/s', exp: 0 },
  { label: 'kbit/s', exp: 3 },
  { label: 'Mbit/s', exp: 6 },
  { label: 'Gbit/s', exp: 9 },
];
export const PPS_UNITS: Unit[] = [
  { label: 'pps', exp: 0 },
  { label: 'kpps', exp: 3 },
  { label: 'Mpps', exp: 6 },
];
export const SIZE_UNITS: Unit[] = [
  { label: 'bit', exp: 0 },
  { label: 'kbit', exp: 3 },
  { label: 'Mbit', exp: 6 },
  { label: 'Gbit', exp: 9 },
];

/** What a unit field of the form holds: the typed text and the index of the selected unit. */
export interface UnitValue {
  text: string;
  unit: number;
}

const DECIMAL = /^([+-]?)(\d*)\.?(\d*)(?:e([+-]?\d+))?$/i;

/** `text` x 10^by as a plain decimal string, or null when `text` is not a decimal number. */
export function shiftDecimal(text: string, by: number): string | null {
  const m = DECIMAL.exec(text.trim());
  if (!m || (m[2] === '' && m[3] === '')) return null;
  const [, sign, int, frac, exp] = m;
  let digits = int + frac;
  let point = int.length + by + (exp ? Number(exp) : 0);
  if (Math.abs(point) > 400) return null;
  if (point < 0) {
    digits = '0'.repeat(-point) + digits;
    point = 0;
  } else if (point > digits.length) {
    digits += '0'.repeat(point - digits.length);
  }
  const whole = digits.slice(0, point).replace(/^0+/, '') || '0';
  const fraction = digits.slice(point).replace(/0+$/, '');
  const out = fraction === '' ? whole : `${whole}.${fraction}`;
  return sign === '-' && out !== '0' ? `-${out}` : out;
}

const decimals = (s: string) => (s.split('.')[1] ?? '').length;

/**
 * The display of a stored base value: the largest unit in which it is >= 1 with at most 3 decimals. Zero shows the
 * field's default unit; a nonzero value that fits no unit that way shows the smallest one.
 */
export function fromBase(base: number, units: Unit[], defaultUnit: number): UnitValue {
  const plain = shiftDecimal(String(base), 0);
  if (plain === null) return { text: String(base), unit: defaultUnit };
  if (Number(plain) === 0) return { text: '0', unit: defaultUnit };
  for (let i = units.length - 1; i > 0; i -= 1) {
    const t = shiftDecimal(plain, -units[i].exp);
    if (t !== null && Number(t) >= 1 && decimals(t) <= 3) return { text: t, unit: i };
  }
  return { text: plain, unit: 0 };
}

/** An empty field for a new form (or an unset optional value). */
export const emptyUnit = (unit: number): UnitValue => ({ text: '', unit });

/** Switches the unit and converts the shown number; an empty or unparsable text stays as it is. */
export function changeUnit(v: UnitValue, units: Unit[], unit: number): UnitValue {
  const t = shiftDecimal(v.text, units[v.unit].exp - units[unit].exp);
  return { text: t ?? v.text, unit };
}

/** The base value of the shown number, or null when it is empty or not a finite number. */
export function toBase(v: UnitValue, units: Unit[]): number | null {
  const t = shiftDecimal(v.text, units[v.unit].exp);
  const n = t === null ? NaN : Number(t);
  return Number.isFinite(n) ? n : null;
}

/** "2.5 µs": a stored base value in the unit the editor would show. */
export function formatUnit(base: number, units: Unit[], defaultUnit: number): string {
  const v = fromBase(base, units, defaultUnit);
  return `${v.text} ${units[v.unit].label}`;
}
