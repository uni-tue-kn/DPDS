// The derived limit line of the Link settings form.
import { SIZE_UNITS, formatUnit } from '../common/units';
import { MBIT } from './linkForm';

const size = (bit: number) => formatUnit(bit, SIZE_UNITS, MBIT);

/** dpds-core's frame size for the derived limiter limit: 1518 B frame + 4 B FEC + 20 B L1 overhead. */
const MAX_FRAME_WIRE_BYTES = 1542;

/** The read-only limiter limit line of the form; the number is left out while the capacity is no valid count. */
export function derivedLimitText(capacity: string): string {
  const text = 'Limit: derived from buffer capacity × 1542 B';
  const n = Number(capacity);
  if (capacity.trim() === '' || !Number.isInteger(n) || n < 0) return text;
  return `${text} = ${size(n * MAX_FRAME_WIRE_BYTES * 8)}`;
}
