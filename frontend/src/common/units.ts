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
  /**
   * Set by a cross-family unit change: the text and unit the shown number was converted from. Further unit changes and
   * saving use it, so switching back and forth does not drift. Typing into the number drops it. Form state only.
   */
  from?: { text: string; unit: number };
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

/** Layer-1 overhead per frame (preamble + SFD + inter-frame gap) and the smallest Ethernet frame, FCS included. */
const L1_OVERHEAD_BYTES = 20;
export const MIN_FRAME_BYTES = 64;

const isPackets = (u: Unit) => u.label.endsWith('pps');

/** True when both units measure the same thing (packets/s, or bit/s). */
export const sameFamily = (a: Unit, b: Unit) => isPackets(a) === isPackets(b);

/** The average frame size in bytes (L2 frame incl. FCS) a text holds, or null when it is empty, no number or below 64. */
export function parseFrame(text: string): number | null {
  const n = text.trim() === '' ? NaN : Number(text);
  return Number.isFinite(n) && n >= MIN_FRAME_BYTES ? n : null;
}

/** A base value of one family in the base unit of the other: pps = bit/s / ((frame + 20) x 8). */
export function convertBase(n: number, from: Unit, to: Unit, frame: number): number {
  if (sameFamily(from, to)) return n;
  const bitsPerFrame = (frame + L1_OVERHEAD_BYTES) * 8;
  return isPackets(from) ? n * bitsPerFrame : n / bitsPerFrame;
}

/**
 * Switches the unit and converts the shown number; an empty or unparsable text stays as it is. Within a family the
 * conversion is decimal-exact; across families it needs the frame size and shows the number rounded to 3 decimals, and
 * the value it was converted from is remembered (`from`) so that the next change converts from that, not from the
 * rounded number. Without a frame size a cross-family number cannot be converted, so it is cleared.
 */
export function changeUnit(v: UnitValue, units: Unit[], unit: number, frame: number | null = null): UnitValue {
  const src = sourceOf(v);
  const from = units[src.unit];
  const to = units[unit];
  if (!sameFamily(from, to)) {
    const n = toBase(src, units);
    if (frame === null) return { text: '', unit };
    if (n === null) return { text: src.text, unit };
    const shown = convertBase(n, from, to, frame) / 10 ** to.exp;
    return { text: String(Number(shown.toFixed(3))), unit, from: src };
  }
  const t = shiftDecimal(src.text, from.exp - to.exp);
  return v.from ? { text: t ?? src.text, unit, from: v.from } : { text: t ?? src.text, unit };
}

/** What the field holds exactly: the remembered value if there is one, else the shown number. */
export const sourceOf = (v: UnitValue): { text: string; unit: number } => v.from ?? { text: v.text, unit: v.unit };

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
