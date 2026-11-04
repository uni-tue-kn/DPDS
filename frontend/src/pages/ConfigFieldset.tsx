import type { ReactNode } from 'react';
import { Button, Col, Form, InputGroup, Row } from 'react-bootstrap';
import {
  configToForm,
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
import { TIME_UNITS, changeUnit, parseFrame, sameFamily } from '../common/units';
import type { Unit, UnitValue } from '../common/units';
import type { Preset } from '../api/types';
import { derivedLimitText } from './linkSummary';

interface Props {
  title: string;
  value: ConfigForm;
  onChange: (next: ConfigForm) => void;
  /** Given: an "Apply preset" select copies the chosen preset's config into this form. */
  presets?: Preset[];
  /** Given: a "Save as preset" button. */
  onSaveAsPreset?: () => void;
}

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

/** The config form of one link or preset: a `<fieldset>` named by `title` with one subsection per stage. */
export default function ConfigFieldset({ title, value, onChange, presets, onSaveAsPreset }: Props) {
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
        onChange={(e) => set(key, e.target.value as never)}
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
            onChange={(e) => set(key, { ...v, text: e.target.value })}
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
      {(presets || onSaveAsPreset) && (
        <Row className="align-items-end">
          {presets && (
            <Form.Group as={Col} md={4} className="mb-3" controlId={id('apply-preset')}>
              <FieldLabel help="Apply preset" />
              {/* Always shows the placeholder: choosing copies the config, it does not link to the preset. */}
              <Form.Select
                value=""
                onChange={(e) => {
                  const p = presets.find((x) => String(x.id) === e.target.value);
                  if (p) onChange(configToForm(p.config, p.avg_frame_size_byte));
                }}
              >
                <option value="">Select a preset...</option>
                {presets.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))}
              </Form.Select>
            </Form.Group>
          )}
          {onSaveAsPreset && (
            <Col md={4} className="mb-3">
              <Button type="button" variant="outline-secondary" onClick={onSaveAsPreset}>
                Save as preset
              </Button>
            </Col>
          )}
        </Row>
      )}
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
              {check('ge_good', 'Gilbert-Elliot starts in good state')}
            </>
          )}
        </>,
      )}
      <Section title="Statistics">
        {check('delay_stats', 'Delay statistics')}
        {text('hist_bins', 'Histogram bins', { hint: 'Empty: 256 bins, mean +- 6 std. Max 500.' })}
        {unitField('hist_lo', 'Histogram lo', TIME_UNITS)}
        {unitField('hist_hi', 'Histogram hi', TIME_UNITS)}
      </Section>
    </fieldset>
  );
}
