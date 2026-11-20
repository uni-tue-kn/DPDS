import { Form } from 'react-bootstrap';
import InfoBox from '../components/InfoBox';
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

/** The info button of a field: it shows what the field means, its unit and its range (`HELP[help]`). `label` defaults to `help`. */
export function InfoHelp({ help, label = help }: { help: HelpKey; label?: string }) {
  return <InfoBox label={label}>{HELP[help]}</InfoBox>;
}

/** A form label, its required marker if any, then its info button. `label` defaults to the help key. */
export function FieldLabel({ help, label = help, required }: { help: HelpKey; label?: string; required?: boolean }) {
  return (
    <>
      <Form.Label className="mb-0">{label}</Form.Label>
      {required && <RequiredMarker />}
      <InfoHelp help={help} label={label} />
    </>
  );
}
