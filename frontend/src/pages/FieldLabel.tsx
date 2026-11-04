import { useId } from 'react';
import { Form, OverlayTrigger, Tooltip } from 'react-bootstrap';
import { HELP } from './fieldHelp';
import type { HelpKey } from './fieldHelp';

/** The marker after the label of a required field. aria-hidden: the accessible name stays the plain label. */
export function RequiredMarker() {
  return (
    <span className="required-marker" aria-hidden="true">
      *
    </span>
  );
}

/** The one note of a form that explains the marker. */
export function RequiredNote() {
  return <Form.Text className="d-block mb-3">* required</Form.Text>;
}

/**
 * The info icon after a label: hover or keyboard focus shows what the field means, its unit and its range (`HELP[help]`).
 * A button of its own (outside the `<label>`), so the field keeps its plain accessible name. `label` defaults to `help`.
 */
export function InfoTip({ help, label = help }: { help: HelpKey; label?: string }) {
  const id = useId();
  return (
    <OverlayTrigger
      placement="top"
      trigger={['hover', 'focus']}
      overlay={
        <Tooltip id={id} className="info-tip-text">
          {HELP[help]}
        </Tooltip>
      }
    >
      <button type="button" className="info-tip" aria-label={`About ${label}`}>
        <i className="bi bi-info-circle" aria-hidden="true" />
      </button>
    </OverlayTrigger>
  );
}

/** A form label, its required marker if any, then its info tooltip. `label` defaults to the help key. */
export function FieldLabel({ help, label = help, required }: { help: HelpKey; label?: string; required?: boolean }) {
  return (
    <>
      <Form.Label>{label}</Form.Label>
      {required && <RequiredMarker />}
      <InfoTip help={help} label={label} />
    </>
  );
}
