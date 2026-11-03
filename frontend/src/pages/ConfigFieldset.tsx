import type { ReactNode } from 'react';
import { Button, Col, Form, InputGroup, Row } from 'react-bootstrap';
import {
  configToForm, DISTRIBUTIONS, halfLifeRequired, LIMITER_MODES, limiterRequired, stdRequired,
} from './linkForm';
import type { ConfigForm } from './linkForm';
import { PPS_UNITS, RATE_UNITS, SIZE_UNITS, TIME_UNITS, changeUnit } from '../common/units';
import type { Unit, UnitValue } from '../common/units';
import type { Preset } from '../api/types';

/** The marker after the label of a required field. aria-hidden: the accessible name stays the plain label. */
export function RequiredMarker() {
  return <span className="required-marker" aria-hidden="true">*</span>;
}

/** The one note of a form that explains the marker. */
export function RequiredNote() {
  return <Form.Text className="d-block mb-3">* required</Form.Text>;
}

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

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <fieldset className="config-section">
      <legend>{title}</legend>
      <Row>{children}</Row>
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

  const text = (key: PlainKey, label: string, opts: { required?: boolean; hint?: string } = {}) => (
    <Form.Group as={Col} md={4} className="mb-3" controlId={id(key)}>
      <Form.Label>{label}</Form.Label>
      {opts.required && <RequiredMarker />}
      <Form.Control
        type="number" step="any" value={value[key]} onChange={(e) => set(key, e.target.value as never)}
        required={opts.required} aria-required={opts.required ? 'true' : undefined}
      />
      {opts.hint && <Form.Text>{opts.hint}</Form.Text>}
    </Form.Group>
  );

  /** A number with its own unit select right of it; changing the unit converts the shown number. */
  const unitField = (
    key: UnitKey, label: string, units: Unit[],
    opts: { required?: boolean; disabled?: boolean; hint?: string } = {},
  ) => {
    const v = value[key];
    return (
      <Form.Group as={Col} md={4} className="mb-3" controlId={id(key)}>
        <Form.Label>{label}</Form.Label>
        {opts.required && <RequiredMarker />}
        <InputGroup>
          <Form.Control
            type="number" step="any" value={v.text} disabled={opts.disabled}
            required={opts.required} aria-required={opts.required ? 'true' : undefined}
            onChange={(e) => set(key, { ...v, text: e.target.value })}
          />
          <Form.Label htmlFor={`${id(key)}-unit`} visuallyHidden>{`${label} unit`}</Form.Label>
          <Form.Select
            id={`${id(key)}-unit`} className="unit-select" value={v.unit} disabled={opts.disabled}
            onChange={(e) => set(key, changeUnit(v, units, Number(e.target.value)))}
          >
            {units.map((u, i) => <option key={u.label} value={i}>{u.label}</option>)}
          </Form.Select>
        </InputGroup>
        {opts.hint && <Form.Text>{opts.hint}</Form.Text>}
      </Form.Group>
    );
  };

  const check = (key: keyof ConfigForm, label: string) => (
    <Form.Group as={Col} md={4} className="mb-3">
      <Form.Check
        type="checkbox"
        id={id(key)}
        label={label}
        checked={Boolean(value[key])}
        onChange={(e) => set(key, e.target.checked as never)}
      />
    </Form.Group>
  );

  const select = (
    key: 'distribution' | 'limiter_mode' | 'loss_type', label: string, options: readonly string[], disabled = false,
  ) => (
    <Form.Group as={Col} md={4} className="mb-3" controlId={id(key)}>
      <Form.Label>{label}</Form.Label>
      <Form.Select value={value[key]} disabled={disabled} onChange={(e) => set(key, e.target.value as never)}>
        {options.map((o) => <option key={o} value={o}>{o}</option>)}
      </Form.Select>
    </Form.Group>
  );

  const limiterOff = !value.limiter_activate;
  const limiterReq = limiterRequired(value);

  return (
    <fieldset className="direction" aria-labelledby={id('title')}>
      {/* Not a <legend>: the subsections are the only fieldsets with one, so "fieldset > legend" lists exactly them. */}
      <div id={id('title')} className="config-title">{title}</div>
      {(presets || onSaveAsPreset) && (
        <Row className="align-items-end">
          {presets && (
            <Form.Group as={Col} md={4} className="mb-3" controlId={id('apply-preset')}>
              <Form.Label>Apply preset</Form.Label>
              {/* Always shows the placeholder: choosing copies the config, it does not link to the preset. */}
              <Form.Select
                value=""
                onChange={(e) => {
                  const p = presets.find((x) => String(x.id) === e.target.value);
                  if (p) onChange(configToForm(p.config));
                }}
              >
                <option value="">Select a preset...</option>
                {presets.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
              </Form.Select>
            </Form.Group>
          )}
          {onSaveAsPreset && (
            <Col md={4} className="mb-3">
              <Button type="button" variant="outline-secondary" onClick={onSaveAsPreset}>Save as preset</Button>
            </Col>
          )}
        </Row>
      )}
      <Section title="Buffer">
        {text('buffer_capacity', 'Buffer capacity', { required: true })}
        {check('reordering', 'Reordering')}
        {text('seed', 'Seed', { hint: 'Empty: random seed.' })}
      </Section>
      <Section title="Delayer">
        {select('distribution', 'Delay distribution', DISTRIBUTIONS)}
        {unitField('mean_ns', 'Mean delay', TIME_UNITS, { required: true })}
        {unitField('std_ns', 'Delay std', TIME_UNITS, { required: stdRequired(value) })}
        {check('activate_correlation', 'Activate correlation')}
        {unitField('half_life_period_ns', 'Half-life period', TIME_UNITS, {
          required: halfLifeRequired(value), disabled: !value.activate_correlation,
        })}
        {unitField('initialization_rate_pps', 'Initialization rate', PPS_UNITS, {
          required: true,
          hint: 'Expected packet rate of this link, e.g. about 8.1 Mpps for 100 Gb/s of 1518 B frames (rate / ((frame + 20 B) x 8))',
        })}
      </Section>
      <Section title="Limiter">
        {check('limiter_activate', 'Limiter active')}
        {select('limiter_mode', 'Limiter mode', LIMITER_MODES, limiterOff)}
        {unitField('rate_bps', 'Rate', RATE_UNITS, { required: limiterReq, disabled: limiterOff })}
        {unitField('limit_bit', 'Burst limit', SIZE_UNITS, { required: limiterReq, disabled: limiterOff })}
      </Section>
      <Section title="Dropper">
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
      </Section>
      <Section title="Statistics">
        {check('delay_stats', 'Delay statistics')}
        {text('hist_bins', 'Histogram bins', { hint: 'Empty: 256 bins, mean +- 6 std. Max 500.' })}
        {unitField('hist_lo', 'Histogram lo', TIME_UNITS)}
        {unitField('hist_hi', 'Histogram hi', TIME_UNITS)}
      </Section>
    </fieldset>
  );
}
