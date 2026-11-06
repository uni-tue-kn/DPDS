import { Col, Form, Row } from 'react-bootstrap';
import ConfigModal from './ConfigModal';
import { fieldKit } from './ConfigFields';
import { InfoHelp } from './FieldLabel';
import { histogramProblem, isPowerOfTwo, rangeAdvice } from './linkForm';
import type { ConfigForm } from './linkForm';
import { TIME_UNITS } from '../common/units';

function HistogramBody({ value, onChange }: { value: ConfigForm; onChange: (next: ConfigForm) => void }) {
  const { text, unitField } = fieldKit(value, onChange);
  const advice = rangeAdvice(value);
  const binsText = value.hist_bins.trim();
  const binsHint =
    binsText === ''
      ? ''
      : isPowerOfTwo(Number(binsText))
        ? ' Power-of-two bin count.'
        : ' Tip: use a power-of-two bin count (1, 2, 4, …, 4096).';

  return (
    <>
      <div className="d-flex align-items-center gap-2 mb-3">
        <Form.Check
          type="switch"
          id="cfg-delay_stats"
          label="Delay statistics"
          checked={value.delay_stats}
          onChange={(e) => onChange({ ...value, delay_stats: e.target.checked })}
        />
        <InfoHelp help="Delay statistics" />
      </div>
      <fieldset>
        <legend className="visually-hidden">Delay histogram</legend>
        <h5 className="mb-2">Delay histogram</h5>
        <p className="mb-3">
          How long packets stay in the link, from arrival until they are enqueued for transmission. Empty fields are
          chosen automatically.
        </p>
        {text('hist_bins', 'Histogram bins', { hint: `Empty: 256 bins, mean +- 6 std. Max 4096.${binsHint}` })}
        {unitField('hist_lo', 'Histogram lo', TIME_UNITS)}
        {unitField('hist_hi', 'Histogram hi', TIME_UNITS)}
        {advice && (
          <Row>
            <Col className="histogram-optimization-hint">
              <span>
                {advice.aligned ? (
                  <>
                    <i
                      className="bi bi-check-circle-fill histogram-optimization-icon histogram-optimization-icon-aligned"
                      aria-hidden="true"
                    />
                    Binary-aligned bins: <strong>{`${advice.width} ${TIME_UNITS[value.hist_hi.unit].label}`}</strong>{' '}
                    each {`(2^${advice.exponent} ns).`}
                  </>
                ) : (
                  <>
                    <i
                      className="bi bi-exclamation-circle-fill histogram-optimization-icon histogram-optimization-icon-suggestion"
                      aria-hidden="true"
                    />
                    Suggested range:{' '}
                    <strong>{`0 – ${advice.suggested} ${TIME_UNITS[value.hist_hi.unit].label}`}</strong>{' '}
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
          </Row>
        )}
      </fieldset>
      <hr />
      <fieldset>
        <legend className="visually-hidden">IAT histogram</legend>
        <h5 className="mb-2">IAT histogram</h5>
        <p className="mb-3">The gaps between consecutive departures of the link. Empty fields use the defaults.</p>
        {text('iat_bins', 'IAT histogram bins')}
        {unitField('iat_lo', 'IAT histogram lo', TIME_UNITS)}
        {unitField('iat_hi', 'IAT histogram hi', TIME_UNITS)}
      </fieldset>
    </>
  );
}

/** The "Histogram" modal: delay statistics, the delay histogram and the IAT histogram of one link. */
export default function HistogramModal({
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
    <ConfigModal title={title} initial={initial} problem={histogramProblem} onConfirm={onConfirm} onClose={onClose}>
      {(value, onChange) => <HistogramBody value={value} onChange={onChange} />}
    </ConfigModal>
  );
}
