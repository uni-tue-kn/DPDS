import { useState } from 'react';
import type { FormEvent } from 'react';
import { Button, Form, Modal } from 'react-bootstrap';
import { savePreset } from '../common/localConfig';
import ErrorAlert from '../components/ErrorAlert';
import { errorMessage } from '../common/format';
import type { Preset } from '../api/types';
import type { FormDto } from './linkForm';

/**
 * Name prompt for "Save as preset". `getConfig` validates the form and returns its config and frame size; it throws a
 * readable Error when the form is invalid.
 */
export default function SavePresetModal({
  getConfig,
  onSaved,
  onClose,
}: {
  getConfig: () => FormDto;
  onSaved: (p: Preset) => void;
  onClose: () => void;
}) {
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [error, setError] = useState<string>();

  const submit = (ev: FormEvent) => {
    ev.preventDefault();
    setError(undefined);
    if (name.trim() === '') {
      setError('Name is required');
      return;
    }
    try {
      onSaved(
        savePreset({
          name: name.trim(),
          description: description.trim() === '' ? null : description.trim(),
          ...getConfig(),
        }),
      );
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  return (
    <Modal show onHide={onClose} animation={false} centered>
      <Form onSubmit={submit} noValidate>
        <Modal.Header closeButton>
          <Modal.Title>Save as preset</Modal.Title>
        </Modal.Header>
        <Modal.Body>
          <ErrorAlert message={error} />
          <Form.Group className="mb-3" controlId="save-preset-name">
            <Form.Label>Preset name</Form.Label>
            <Form.Control type="text" maxLength={64} autoFocus value={name} onChange={(e) => setName(e.target.value)} />
          </Form.Group>
          <Form.Group controlId="save-preset-description">
            <Form.Label>Description</Form.Label>
            <Form.Control type="text" value={description} onChange={(e) => setDescription(e.target.value)} />
          </Form.Group>
        </Modal.Body>
        <Modal.Footer>
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" variant="primary">
            Save preset
          </Button>
        </Modal.Footer>
      </Form>
    </Modal>
  );
}
