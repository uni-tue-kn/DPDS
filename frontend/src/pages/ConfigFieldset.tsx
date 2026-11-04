import type { ReactNode } from 'react';
import { Button, Col, Form, InputGroup, Row } from 'react-bootstrap';
import {
  DISTRIBUTIONS,
  frameRequired,
  halfLifeRequired,
  INIT_RATE_UNITS,
  LIMITER_MODES,
  LIMITER_RATE_UNITS,
  stdRequired,
} from './linkForm';
import type { ConfigForm } from './linkForm';
import { FieldLabel, InfoTip } from './FieldLabel';
import type { HelpKey } from './fieldHelp';
import { TIME_UNITS, changeUnit, parseFrame, sameFamily, shiftDecimal, toBase } from '../common/units';
import type { Unit, UnitValue } from '../common/units';
import { derivedLimitText } from './linkSummary';

interface Props {
  title: string;
  value: ConfigForm;
  onChange: (next: ConfigForm) => void;
}

const forget = ({ text, unit }: UnitValue): UnitValue => ({ text, unit });

type UnitKey = {
  [K in keyof ConfigForm]: ConfigForm[K] extends UnitValue ? K : never;
}[keyof ConfigForm];

function Section({ title, onRemove, children }: { title: string; onRemove?: () => void; children: ReactNode }) {
  return (
    <fieldset className="config-section">
      <legend>{title}</legend>
      <Row>{children}</Row>
      {onRemove && (
        <Button type="button" variant="outline-secondary" size="sm" className="mb-3" onClick={onRemove}>
          {`Remove ${title.toLowerCase()}`}
        </Button>
      )}
    </fieldset>
  );
}

const isPowerOfTwo = (n: number) => Number.isSafeInteger(n) && n > 0 && Number.isInteger(Math.log2(n));

/** `ns` as the text of the time unit `unit`, exact ("0.064"), or null when it cannot be shown. */
const inUnit = (ns: number, unit: number) => shiftDecimal(String(ns), -TIME_UNITS[unit].exp);

/** P4TG-style advice on the histogram range: advisory only, nothing here refuses a value. */
function rangeAdvice(
  value: ConfigForm,
): { aligned: boolean; width: string; suggested: string; exponent: number } | null {
  const hi = value.hist_hi.text.trim() === '' ? null : toBase(value.hist_hi, TIME_UNITS);
  if (hi === null || hi <= 0) return null;
  const lo = value.hist_lo.text.trim() === '' ? null : toBase(value.hist_lo, TIME_UNITS);
  const bins = value.hist_bins.trim() === '' ? NaN : Number(value.hist_bins);
  const width = lo === null ? NaN : (hi - lo) / bins;
  if (lo !== null && Number.isSafeInteger(lo) && isPowerOfTwo(width) && lo % width === 0) {
    const shown = inUnit(width, value.hist_hi.unit);
    if (shown !== null) return { aligned: true, width: shown, suggested: '', exponent: Math.log2(width) };
  }
  const next = 2 ** Math.ceil(Math.log2(Math.ceil(hi)));
  const suggested = inUnit(next, value.hist_hi.unit);
  if (!Number.isSafeInteger(next) || suggested === null) return null;
  return { aligned: false, width: '', suggested, exponent: Math.log2(next) };
}

/** The config form of one link: a `<fieldset>` named by `title` with one subsection per stage. */
export default function ConfigFieldset({ title, value, onChange }: Props) {
  const id = (name: string) => `cfg-${name}`;
  const set = <K extends keyof ConfigForm>(key: K, v: ConfigForm[K]) => onChange({ ...value, [key]: v });

  type PlainKey = {
    [K in keyof ConfigForm]: ConfigForm[K] extends string ? K : never;
  }[keyof ConfigForm];

  const text = (key: PlainKey, label: HelpKey, opts: { required?: boolean; hint?: string; addon?: string } = {}) => {
    const input = (
      <Form.Control
        type="number"
        step="any"
        value={value[key]}
        onChange={(e) =>
          key === 'avg_frame_size'
            ? // Any frame-size edit makes the shown rate text count: the old frame no longer applies to the remembered values.
              onChange({
                ...value,
                avg_frame_size: e.target.value,
                initialization_rate_pps: forget(value.initialization_rate_pps),
                rate_bps: forget(value.rate_bps),
              })
            : set(key, e.target.value as never)
        }
        required={opts.required}
        aria-required={opts.required ? 'true' : undefined}
      />
    );
    return (
      <Form.Group as={Col} md={4} className="mb-3" controlId={id(key)}>
        <FieldLabel help={label} required={opts.required} />
        {opts.addon ? (
          <InputGroup>
            {input}
            <InputGroup.Text>{opts.addon}</InputGroup.Text>
          </InputGroup>
        ) : (
          input
        )}
        {opts.hint && <Form.Text>{opts.hint}</Form.Text>}
      </Form.Group>
    );
  };

  const frame = parseFrame(value.avg_frame_size);
  const advice = rangeAdvice(value);
  const binsText = value.hist_bins.trim();
  const binsHint =
    binsText === ''
      ? ''
      : isPowerOfTwo(Number(binsText))
        ? ' Power-of-two bin count.'
        : ' Tip: use a power-of-two bin count (1, 2, 4, …, 4096).';

  /** A number with its own unit select right of it; changing the unit converts the shown number. */
  const unitField = (
    key: UnitKey,
    label: HelpKey,
    units: Unit[],
    opts: { required?: boolean; disabled?: boolean; hint?: string } = {},
  ) => {
    const v = value[key];
    // The first unit is the field's own family. The other family is offered only with a valid frame size, which
    // converts it; without one, only the unit that is already selected stays (it is no longer convertible).
    const offered = units.flatMap((u, i) =>
      frame !== null || sameFamily(u, units[0]) || i === v.unit ? [{ u, i }] : [],
    );
    return (
      <Form.Group as={Col} md={4} className="mb-3" controlId={id(key)}>
        <FieldLabel help={label} required={opts.required} />
        <InputGroup>
          <Form.Control
            type="number"
            step="any"
            value={v.text}
            disabled={opts.disabled}
            required={opts.required}
            aria-required={opts.required ? 'true' : undefined}
            onChange={(e) => set(key, { text: e.target.value, unit: v.unit })}
          />
          <Form.Label htmlFor={`${id(key)}-unit`} visuallyHidden>{`${label} unit`}</Form.Label>
          <Form.Select
            id={`${id(key)}-unit`}
            className="unit-select"
            value={v.unit}
            disabled={opts.disabled}
            onChange={(e) => set(key, changeUnit(v, units, Number(e.target.value), frame))}
          >
            {offered.map(({ u, i }) => (
              <option key={u.label} value={i}>
                {u.label}
              </option>
            ))}
          </Form.Select>
        </InputGroup>
        {opts.hint && <Form.Text>{opts.hint}</Form.Text>}
      </Form.Group>
    );
  };

  const check = (key: keyof ConfigForm, label: HelpKey) => (
    <Form.Group as={Col} md={4} className="mb-3">
      <Form.Check id={id(key)}>
        <Form.Check.Input
          type="checkbox"
          checked={Boolean(value[key])}
          onChange={(e) => set(key, e.target.checked as never)}
        />
        <Form.Check.Label>{label}</Form.Check.Label>
        <InfoTip help={label} />
      </Form.Check>
    </Form.Group>
  );

  const select = (key: 'distribution' | 'limiter_mode' | 'loss_type', label: HelpKey, options: readonly string[]) => (
    <Form.Group as={Col} md={4} className="mb-3" controlId={id(key)}>
      <FieldLabel help={label} />
      <Form.Select value={value[key]} onChange={(e) => set(key, e.target.value as never)}>
        {options.map((o) => (
          <option key={o} value={o}>
            {o}
          </option>
        ))}
      </Form.Select>
    </Form.Group>
  );

  /** A stage that is not added is one button; added, it is its subsection with a "Remove" control. */
  const stage = (key: 'delayer_added' | 'limiter_added' | 'dropper_added', title: string, body: ReactNode) => {
    const name = title.toLowerCase();
    return value[key] ? (
      <Section title={title} onRemove={() => set(key, false)}>
        {body}
      </Section>
    ) : (
      <div className="mb-3">
        <Button
          type="button"
          variant="outline-primary"
          size="sm"
          onClick={() => set(key, true)}
        >{`Add ${name}`}</Button>
      </div>
    );
  };

  return (
    <fieldset className="direction" aria-labelledby={id('title')}>
      {/* Not a <legend>: the subsections are the only fieldsets with one, so "fieldset > legend" lists exactly them. */}
      <div id={id('title')} className="config-title">
        {title}
      </div>
      <Section title="Buffer">
        {text('buffer_capacity', 'Buffer capacity', { required: true })}
        {check('reordering', 'Reordering')}
        {text('seed', 'Seed', { hint: 'Empty: random seed.' })}
        {text('avg_frame_size', 'Average frame size', { addon: 'B', required: frameRequired(value) })}
      </Section>
      {stage(
        'delayer_added',
        'Delayer',
        <>
          {select('distribution', 'Delay distribution', DISTRIBUTIONS)}
          {unitField('mean_ns', 'Mean delay', TIME_UNITS, { required: true })}
          {unitField('std_ns', 'Delay std', TIME_UNITS, { required: stdRequired(value) })}
          {check('activate_correlation', 'Activate correlation')}
          {unitField('half_life_period_ns', 'Half-life period', TIME_UNITS, {
            required: halfLifeRequired(value),
            disabled: !value.activate_correlation,
          })}
          {unitField('initialization_rate_pps', 'Initialization rate', INIT_RATE_UNITS, {
            required: true,
            hint: 'Expected packet rate of this link, e.g. about 8.1 Mpps for 100 Gb/s of 1518 B frames (rate / ((frame + 20 B) x 8))',
          })}
        </>,
      )}
      {stage(
        'limiter_added',
        'Limiter',
        <>
          {select('limiter_mode', 'Limiter mode', LIMITER_MODES)}
          {unitField('rate_bps', 'Rate', LIMITER_RATE_UNITS, { required: true })}
          <Col md={12} className="mb-3">
            <Form.Text>{derivedLimitText(value.buffer_capacity)}</Form.Text>
          </Col>
        </>,
      )}
      {stage(
        'dropper_added',
        'Dropper',
        <>
          {select('loss_type', 'Loss model', ['Random', 'GilbertElliot'])}
          {value.loss_type === 'Random' ? (
            text('random_p', 'Random loss probability', { required: true })
          ) : (
            <>
              {text('ge_p', 'Gilbert-Elliot p', { required: true })}
              {text('ge_r', 'Gilbert-Elliot r', { required: true })}
              {text('ge_k', 'Gilbert-Elliot k', { required: true })}
              {text('ge_h', 'Gilbert-Elliot h', { required: true })}
            </>
          )}
        </>,
      )}
      <Section title="Statistics">
        {check('delay_stats', 'Delay statistics')}
        {text('hist_bins', 'Histogram bins', { hint: `Empty: 256 bins, mean +- 6 std. Max 4096.${binsHint}` })}
        {unitField('hist_lo', 'Histogram lo', TIME_UNITS)}
        {unitField('hist_hi', 'Histogram hi', TIME_UNITS)}
        {advice && (
          <Col md={12} className="histogram-optimization-hint">
            <span>
              {advice.aligned ? (
                <>
                  <i
                    className="bi bi-check-circle-fill histogram-optimization-icon histogram-optimization-icon-aligned"
                    aria-hidden="true"
                  />
                  Binary-aligned bins: <strong>{`${advice.width} ${TIME_UNITS[value.hist_hi.unit].label}`}</strong> each{' '}
                  {`(2^${advice.exponent} ns).`}
                </>
              ) : (
                <>
                  <i
                    className="bi bi-exclamation-circle-fill histogram-optimization-icon histogram-optimization-icon-suggestion"
                    aria-hidden="true"
                  />
                  Suggested range: <strong>{`0 – ${advice.suggested} ${TIME_UNITS[value.hist_hi.unit].label}`}</strong>{' '}
                  {`(2^${advice.exponent} ns).`}
                </>
              )}
            </span>
            {!advice.aligned && (
              <button
                type="button"
                className="histogram-optimization-action"
                onClick={() =>
                  onChange({
                    ...value,
                    hist_lo: { text: '0', unit: value.hist_lo.unit },
                    hist_hi: { text: advice.suggested, unit: value.hist_hi.unit },
                  })
                }
              >
                Use suggestion
              </button>
            )}
          </Col>
        )}
      </Section>
    </fieldset>
  );
}
