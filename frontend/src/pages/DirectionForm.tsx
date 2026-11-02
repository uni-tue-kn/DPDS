import { Col, Form, Row } from 'react-bootstrap';
import { DISTRIBUTIONS, LIMITER_MODES } from './linkForm';
import type { ConfigForm } from './linkForm';

interface Props {
  index: number;
  title: string;
  value: ConfigForm;
  onChange: (next: ConfigForm) => void;
}

/** One `<fieldset>` per direction; ids are prefixed with the direction so labels stay unique on the page. */
export default function DirectionForm({ index, title, value, onChange }: Props) {
  const id = (name: string) => `d${index}-${name}`;
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
      <Row>
        {select('distribution', 'Delay distribution', DISTRIBUTIONS)}
        {text('mean_ns', 'Mean delay (ns)')}
        {text('std_ns', 'Delay std (ns)')}
        {check('activate_correlation', 'Activate correlation')}
        {text('half_life_period_ns', 'Half-life period (ns)')}
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
