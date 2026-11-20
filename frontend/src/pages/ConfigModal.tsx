import { useState } from 'react';
import type { FormEvent, ReactNode } from 'react';
import { Alert, Button, Modal } from 'react-bootstrap';
import type { ConfigForm } from './linkForm';

/**
 * The shell of the Link settings and Histogram modals (P4TG's SettingsModal): it edits a copy of `initial` (the modal is
 * mounted per opening, so every opening starts from the row). Close, the header X and Escape discard the copy; Confirm
 * (also Enter in a field) hands it to `onConfirm` unless `problem` names one, which shows as an alert and keeps the modal.
 */
export default function ConfigModal({
  title,
  initial,
  problem,
  onConfirm,
  onClose,
  children,
}: {
  title: string;
  initial: ConfigForm;
  problem: (f: ConfigForm) => string | null;
  onConfirm: (f: ConfigForm) => void;
  onClose: () => void;
  children: (value: ConfigForm, onChange: (next: ConfigForm) => void) => ReactNode;
}) {
  const [value, setValue] = useState(initial);
  const [error, setError] = useState<string | null>(null);

  const submit = (ev: FormEvent) => {
    ev.preventDefault();
    const found = problem(value);
    if (found === null) onConfirm(value);
    else setError(found);
  };

  return (
    <Modal show size="lg" onHide={onClose} animation={false}>
      <Modal.Header closeButton>
        <Modal.Title>{title}</Modal.Title>
      </Modal.Header>
      <form onSubmit={submit} noValidate>
        <Modal.Body>
          {error && (
            <Alert variant="danger" dismissible onClose={() => setError(null)}>
              {error}
            </Alert>
          )}
          {children(value, setValue)}
        </Modal.Body>
        <Modal.Footer>
          <Button variant="secondary" onClick={onClose}>
            Close
          </Button>
          <Button variant="primary" type="submit">
            Confirm
          </Button>
        </Modal.Footer>
      </form>
    </Modal>
  );
}
