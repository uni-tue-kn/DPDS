import { useState } from 'react';
import type { FormEvent } from 'react';
import { Button, Form, Modal } from 'react-bootstrap';
import * as api from '../api/client';
import ErrorAlert from '../components/ErrorAlert';
import { errorMessage } from '../common/format';
import type { ConfigDto, Preset } from '../api/types';

/** Name prompt for "Save as preset". `getConfig` validates the form and throws a readable Error when it is invalid. */
export default function SavePresetModal({ getConfig, onSaved, onClose }: {
  getConfig: () => ConfigDto;
  onSaved: (p: Preset) => void;
  onClose: () => void;
}) {
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [error, setError] = useState<string>();
  const [saving, setSaving] = useState(false);

  const submit = async (ev: FormEvent) => {
    ev.preventDefault();
    if (saving) return;
    setError(undefined);
    if (name.trim() === '') {
      setError('Name is required');
      return;
    }
    setSaving(true);
    try {
      const saved = await api.createPreset({
        name: name.trim(),
        description: description.trim() === '' ? null : description.trim(),
        config: getConfig(),
      });
      onSaved(saved);
    } catch (e) {
      setError(errorMessage(e));
      setSaving(false);
    }
  };

  return (
    <Modal show onHide={onClose} animation={false} centered>
      <Form onSubmit={submit} noValidate>
        <Modal.Header closeButton><Modal.Title>Save as preset</Modal.Title></Modal.Header>
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
          <Button variant="secondary" onClick={onClose}>Cancel</Button>
          <Button type="submit" variant="primary" disabled={saving}>Save preset</Button>
        </Modal.Footer>
      </Form>
    </Modal>
  );
}
