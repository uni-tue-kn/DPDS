import { useState } from 'react';
import type { FormEvent } from 'react';
import { Button, Card, Form } from 'react-bootstrap';
import { Link as RouterLink, useNavigate, useParams } from 'react-router';
import * as api from '../api/client';
import ErrorAlert from '../components/ErrorAlert';
import { errorMessage } from '../common/format';
import { useLoad } from '../common/useLoad';
import ConfigFieldset, { RequiredMarker, RequiredNote } from './ConfigFieldset';
import { configToDto, configToForm, defaultConfigForm } from './linkForm';
import type { ConfigForm } from './linkForm';
import type { Preset } from '../api/types';

function Editor({ id, preset }: { id?: number; preset?: Preset }) {
  const navigate = useNavigate();
  const [name, setName] = useState(preset?.name ?? '');
  const [description, setDescription] = useState(preset?.description ?? '');
  const [config, setConfig] = useState<ConfigForm>(() => (preset ? configToForm(preset.config) : defaultConfigForm()));
  const [error, setError] = useState<string>();
  const [saving, setSaving] = useState(false);

  const submit = async (ev: FormEvent) => {
    ev.preventDefault();
    if (saving) return;
    setError(undefined);
    let input;
    try {
      if (name.trim() === '') throw new Error('Name is required');
      input = {
        name: name.trim(),
        description: description.trim() === '' ? null : description.trim(),
        config: configToDto(config),
      };
    } catch (e) {
      setError(errorMessage(e));
      return;
    }
    setSaving(true);
    try {
      if (id === undefined) await api.createPreset(input);
      else await api.updatePreset(id, input);
      navigate('/presets');
    } catch (e) {
      setError(errorMessage(e));
      setSaving(false);
    }
  };

  return (
    <Form onSubmit={submit} noValidate>
      <ErrorAlert message={error} />
      <RequiredNote />
      <Card className="mb-3">
        <Card.Body>
          <Form.Group className="mb-3" controlId="preset-name">
            <Form.Label>Name</Form.Label>
            <RequiredMarker />
            <Form.Control
              type="text" maxLength={64} value={name} onChange={(e) => setName(e.target.value)} required aria-required="true"
            />
          </Form.Group>
          <Form.Group controlId="preset-description">
            <Form.Label>Description</Form.Label>
            <Form.Control type="text" value={description} onChange={(e) => setDescription(e.target.value)} />
          </Form.Group>
        </Card.Body>
      </Card>
      <ConfigFieldset title="Preset configuration" value={config} onChange={setConfig} />
      <Button type="submit" variant="primary" className="me-2" disabled={saving}>Save</Button>
      <RouterLink to="/presets" className="btn btn-outline-secondary">Cancel</RouterLink>
    </Form>
  );
}

export default function PresetEditor() {
  const { id } = useParams();
  const presetId = id === undefined ? undefined : Number(id);
  const preset = useLoad(() => (presetId === undefined ? Promise.resolve(undefined) : api.getPreset(presetId)));
  const ready = presetId === undefined || preset.data !== undefined;

  return (
    <>
      <h1 className="mb-3">{presetId === undefined ? 'New preset' : 'Edit preset'}</h1>
      <ErrorAlert message={preset.error} />
      {ready && <Editor key={presetId ?? 'new'} id={presetId} preset={preset.data} />}
    </>
  );
}
