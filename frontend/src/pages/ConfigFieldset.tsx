import { Button, Col, Form, Row } from 'react-bootstrap';
import { configToForm, DISTRIBUTIONS, LIMITER_MODES } from './linkForm';
import type { ConfigForm } from './linkForm';
import type { Preset } from '../api/types';

interface Props {
  title: string;
  value: ConfigForm;
  onChange: (next: ConfigForm) => void;
  /** Given: an "Apply preset" select copies the chosen preset's config into this form. */
  presets?: Preset[];
  /** Given: a "Save as preset" button. */
  onSaveAsPreset?: () => void;
}

/** The config form of one link: a `<fieldset>` named by `title`. */
export default function ConfigFieldset({ title, value, onChange, presets, onSaveAsPreset }: Props) {
  const id = (name: string) => `cfg-${name}`;
  const set = <K extends keyof ConfigForm>(key: K, v: ConfigForm[K]) => onChange({ ...value, [key]: v });

  const text = (key: keyof ConfigForm, label: string, type: 'number' | 'text' = 'number', hint?: string) => (
    <Form.Group as={Col} md={4} className="mb-3" controlId={id(key)}>
      <Form.Label>{label}</Form.Label>
      <Form.Control type={type} step="any" value={String(value[key])} onChange={(e) => set(key, e.target.value as never)} />
      {hint && <Form.Text>{hint}</Form.Text>}
    </Form.Group>
  );

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

  const select = (key: 'distribution' | 'limiter_mode' | 'loss_type', label: string, options: readonly string[]) => (
    <Form.Group as={Col} md={4} className="mb-3" controlId={id(key)}>
      <Form.Label>{label}</Form.Label>
      <Form.Select value={value[key]} onChange={(e) => set(key, e.target.value as never)}>
        {options.map((o) => <option key={o} value={o}>{o}</option>)}
      </Form.Select>
    </Form.Group>
  );

  return (
    <fieldset className="direction">
      <legend>{title}</legend>
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
      <Row>
        {select('distribution', 'Delay distribution', DISTRIBUTIONS)}
        {text('mean_ns', 'Mean delay (ns)')}
        {text('std_ns', 'Delay std (ns)')}
        {check('activate_correlation', 'Activate correlation')}
        {text('half_life_period_ns', 'Half-life period (ns)')}
        {text('initialization_rate_pps', 'Initialization rate (pps)', 'number', 'Expected packet rate of this link, e.g. about 8100000 for 100 Gb/s of 1518 B frames (rate / ((frame + 20 B) × 8))')}
      </Row>
      <Row>
        {check('limiter_activate', 'Limiter active')}
        {select('limiter_mode', 'Limiter mode', LIMITER_MODES)}
        {text('rate_bps', 'Rate (bit/s)')}
        {text('limit_bit', 'Burst limit (bit)')}
      </Row>
      <Row>
        {select('loss_type', 'Loss model', ['Random', 'GilbertElliot'])}
        {value.loss_type === 'Random' ? (
          text('random_p', 'Random loss probability')
        ) : (
          <>
            {text('ge_p', 'Gilbert-Elliot p')}
            {text('ge_r', 'Gilbert-Elliot r')}
            {text('ge_k', 'Gilbert-Elliot k')}
            {text('ge_h', 'Gilbert-Elliot h')}
            {check('ge_good', 'Gilbert-Elliot starts in good state')}
          </>
        )}
      </Row>
      <Row>
        {text('buffer_capacity', 'Buffer capacity')}
        {check('reordering', 'Reordering')}
        {text('seed', 'Seed', 'number', 'Empty: random seed.')}
      </Row>
      <Row>
        {text('hist_bins', 'Histogram bins', 'number', 'Empty: 256 bins, mean +- 6 std. Max 500.')}
        {text('hist_lo', 'Histogram lo (ns)')}
        {text('hist_hi', 'Histogram hi (ns)')}
        {check('delay_stats', 'Delay statistics')}
      </Row>
    </fieldset>
  );
}
