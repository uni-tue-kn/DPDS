import type { ReactNode } from 'react';
import { Col, Form, InputGroup, Row, ToggleButton, ToggleButtonGroup } from 'react-bootstrap';
import { ACCURACIES, accuracyOf, withAccuracy } from './linkForm';
import type { Accuracy, ConfigForm } from './linkForm';
import { FieldLabel } from './FieldLabel';
import type { HelpKey } from './fieldHelp';
import { changeUnit, parseFrame, sameFamily } from '../common/units';
import type { Unit, UnitValue } from '../common/units';

const forget = ({ text, unit }: UnitValue): UnitValue => ({ text, unit });

type UnitKey = {
  [K in keyof ConfigForm]: ConfigForm[K] extends UnitValue ? K : never;
}[keyof ConfigForm];

type PlainKey = {
  [K in keyof ConfigForm]: ConfigForm[K] extends string ? K : never;
}[keyof ConfigForm];

const fieldId = (name: string) => `cfg-${name}`;

/** One row of a modal form (P4TG layout): the label with its marker and info button left, the control and its hint right. */
function Field({
  id,
  help,
  required,
  hint,
  children,
}: {
  id: string;
  help: HelpKey;
  required?: boolean;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <Form.Group as={Row} className="mb-3 align-items-center" controlId={id}>
      <Col sm={3}>
        <FieldLabel help={help} required={required} />
      </Col>
      <Col sm={9}>
        {children}
        {hint && <Form.Text className="text-muted">{hint}</Form.Text>}
      </Col>
    </Form.Group>
  );
}

/** The field builders of the two modal bodies, bound to the form `value` they edit. */
export function fieldKit(value: ConfigForm, onChange: (next: ConfigForm) => void) {
  const set = <K extends keyof ConfigForm>(key: K, v: ConfigForm[K]) => onChange({ ...value, [key]: v });

  const text = (key: PlainKey, label: HelpKey, opts: { required?: boolean; hint?: string; addon?: string } = {}) => {
    const input = (
      <Form.Control
        type="number"
        step="any"
        value={value[key]}
        onChange={(e) =>
          key === 'avg_frame_size'
            ? // Any frame-size edit makes the shown rate text count: the old frame does not apply to the remembered values.
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
      <Field key={key} id={fieldId(key)} help={label} required={opts.required} hint={opts.hint}>
        {opts.addon ? (
          <InputGroup>
            {input}
            <InputGroup.Text>{opts.addon}</InputGroup.Text>
          </InputGroup>
        ) : (
          input
        )}
      </Field>
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
    // converts it; without one, only the unit that is already selected stays (it is not convertible).
    const offered = units.flatMap((u, i) =>
      frame !== null || sameFamily(u, units[0]) || i === v.unit ? [{ u, i }] : [],
    );
    return (
      <Field key={key} id={fieldId(key)} help={label} required={opts.required} hint={opts.hint}>
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
          <Form.Select
            id={`${fieldId(key)}-unit`}
            className="unit-select"
            aria-label={`${label} unit`}
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
      </Field>
    );
  };

  /** The three-way "Delay accuracy" toggle: it sets the two booleans of the config together. */
  const accuracyToggle = () => (
    <Field key="accuracy" id={fieldId('accuracy')} help="Delay accuracy">
      <ToggleButtonGroup
        type="radio"
        name={fieldId('accuracy')}
        id={fieldId('accuracy')}
        aria-label="Delay accuracy"
        size="sm"
        value={accuracyOf(value)}
        onChange={(a: Accuracy) => onChange(withAccuracy(value, a))}
      >
        {ACCURACIES.map((a) => (
          <ToggleButton key={a} id={fieldId(`accuracy-${a}`)} value={a} variant="outline-primary">
            {a}
          </ToggleButton>
        ))}
      </ToggleButtonGroup>
    </Field>
  );

  const select = (
    key: 'distribution' | 'limiter_mode' | 'loss_type',
    label: HelpKey,
    options: readonly string[],
    opts: { disabled?: readonly string[]; hint?: string } = {},
  ) => (
    <Field key={key} id={fieldId(key)} help={label} hint={opts.hint}>
      <Form.Select value={value[key]} onChange={(e) => set(key, e.target.value as never)}>
        {options.map((o) => (
          <option key={o} value={o} disabled={opts.disabled?.includes(o)}>
            {o}
          </option>
        ))}
      </Form.Select>
    </Field>
  );

  return { text, unitField, accuracyToggle, select };
}
