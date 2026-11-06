import type { ReactNode } from 'react';
import { Accordion, Form } from 'react-bootstrap';
import ConfigModal from './ConfigModal';
import { fieldKit } from './ConfigFields';
import { RequiredNote } from './FieldLabel';
import {
  DISTRIBUTIONS,
  frameRequired,
  halfLifeRequired,
  INIT_RATE_UNITS,
  LIMITER_MODES,
  LIMITER_RATE_UNITS,
  accuracyOf,
  reorderingActive,
  stageProblem,
  stdRequired,
} from './linkForm';
import type { ConfigForm } from './linkForm';
import { derivedLimitText } from './linkSummary';
import { PACKET_UNITS, TIME_UNITS, shiftDecimal } from '../common/units';

/** One always-open accordion item; its fieldset is named by a visually hidden legend, so the section is a group. */
function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <Accordion.Item eventKey={title}>
      <Accordion.Header>{title}</Accordion.Header>
      <Accordion.Body>
        <fieldset>
          <legend className="visually-hidden">{title}</legend>
          {children}
        </fieldset>
      </Accordion.Body>
    </Accordion.Item>
  );
}

type StageKey = 'delayer_added' | 'limiter_added' | 'dropper_added';

function LinkSettingsBody({ value, onChange }: { value: ConfigForm; onChange: (next: ConfigForm) => void }) {
  const { text, unitField, accuracyToggle, select } = fieldKit(value, onChange);
  const packets = shiftDecimal(value.buffer_capacity.text, PACKET_UNITS[value.buffer_capacity.unit].exp) ?? '';

  /** The on/off switch of a stage; off, its fields are not rendered (their typed values stay in the form). */
  const stage = (key: StageKey, name: string, fields: ReactNode) => (
    <>
      <Form.Check
        type="switch"
        id={`cfg-${key}`}
        className="mb-3"
        label={`Enable ${name}`}
        checked={value[key]}
        onChange={(e) => onChange({ ...value, [key]: e.target.checked })}
      />
      {value[key] && fields}
    </>
  );

  return (
    <>
      <RequiredNote />
      <Accordion alwaysOpen defaultActiveKey={['Buffer', 'Delayer', 'Limiter', 'Dropper']}>
        <Section title="Buffer">
          {unitField('buffer_capacity', 'Buffer capacity', PACKET_UNITS, { required: true })}
          {text('seed', 'Seed', { hint: 'Empty: random seed.' })}
          {text('avg_frame_size', 'Average frame size', { addon: 'B', required: frameRequired(value) })}
        </Section>
        <Section title="Delayer">
          {stage(
            'delayer_added',
            'delayer',
            <>
              {select('distribution', 'Delay distribution', DISTRIBUTIONS)}
              {unitField('mean_ns', 'Mean delay', TIME_UNITS, { required: true })}
              {unitField('std_ns', 'Delay std', TIME_UNITS, { required: stdRequired(value) })}
              {accuracyToggle()}
              {unitField('half_life_period_ns', 'Half-life period', TIME_UNITS, {
                required: halfLifeRequired(value),
                disabled: accuracyOf(value) !== 'Correlation',
              })}
              {unitField('initialization_rate_pps', 'Initialization rate', INIT_RATE_UNITS, {
                required: true,
                hint: 'Expected packet rate of this link, e.g. about 8.1 Mpps for 100 Gb/s of 1518 B frames (rate / ((frame + 20 B) x 8))',
              })}
            </>,
          )}
        </Section>
        <Section title="Limiter">
          {stage(
            'limiter_added',
            'limiter',
            <>
              {select('limiter_mode', 'Limiter mode', LIMITER_MODES, {
                disabled: reorderingActive(value) ? ['Spacer'] : [],
                hint: reorderingActive(value) ? 'Reordering works only with the policer (token bucket).' : undefined,
              })}
              {unitField('rate_bps', 'Rate', LIMITER_RATE_UNITS, { required: true })}
              <Form.Text className="d-block text-muted">{derivedLimitText(packets)}</Form.Text>
            </>,
          )}
        </Section>
        <Section title="Dropper">
          {stage(
            'dropper_added',
            'dropper',
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
        </Section>
      </Accordion>
    </>
  );
}

/** The "Link settings" modal: Buffer, Delayer, Limiter and Dropper of one link. */
export default function LinkSettingsModal({
  title,
  initial,
  onConfirm,
  onClose,
}: {
  title: string;
  initial: ConfigForm;
  onConfirm: (f: ConfigForm) => void;
  onClose: () => void;
}) {
  return (
    <ConfigModal title={title} initial={initial} problem={stageProblem} onConfirm={onConfirm} onClose={onClose}>
      {(value, onChange) => <LinkSettingsBody value={value} onChange={onChange} />}
    </ConfigModal>
  );
}
