import { useState } from 'react';
import type { FormEvent } from 'react';
import { Button, Card, Form } from 'react-bootstrap';
import { Link as RouterLink, useNavigate, useParams } from 'react-router';
import ErrorAlert from '../components/ErrorAlert';
import { errorMessage } from '../common/format';
import { savePreset, useLocalConfig } from '../common/localConfig';
import ConfigFieldset from './ConfigFieldset';
import { FieldLabel, RequiredNote } from './FieldLabel';
import { configToForm, defaultConfigForm, formToDto } from './linkForm';
import type { ConfigForm } from './linkForm';
import type { Preset, PresetInput } from '../api/types';

function Editor({ id, preset }: { id?: number; preset?: Preset }) {
  const navigate = useNavigate();
  const [name, setName] = useState(preset?.name ?? '');
  const [description, setDescription] = useState(preset?.description ?? '');
  const [config, setConfig] = useState<ConfigForm>(() =>
    preset ? configToForm(preset.config, preset.avg_frame_size_byte) : defaultConfigForm(),
  );
  const [error, setError] = useState<string>();

  const submit = (ev: FormEvent) => {
    ev.preventDefault();
    setError(undefined);
    let input: PresetInput;
    try {
      if (name.trim() === '') throw new Error('Name is required');
      input = {
        name: name.trim(),
        description: description.trim() === '' ? null : description.trim(),
        ...formToDto(config),
      };
    } catch (e) {
      setError(errorMessage(e));
      return;
    }
    try {
      savePreset(input, id);
      navigate('/presets');
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  return (
    <Form onSubmit={submit} noValidate>
      <ErrorAlert message={error} />
      <RequiredNote />
      <Card className="mb-3">
        <Card.Body>
          <Form.Group className="mb-3" controlId="preset-name">
            <FieldLabel label="Name" help="Preset name" required />
            <Form.Control
              type="text"
              maxLength={64}
              value={name}
              onChange={(e) => setName(e.target.value)}
              required
              aria-required="true"
            />
          </Form.Group>
          <Form.Group controlId="preset-description">
            <FieldLabel help="Description" />
            <Form.Control type="text" value={description} onChange={(e) => setDescription(e.target.value)} />
          </Form.Group>
        </Card.Body>
      </Card>
      <ConfigFieldset title="Preset configuration" value={config} onChange={setConfig} />
      <Button type="submit" variant="primary" className="me-2">
        Save
      </Button>
      <RouterLink to="/presets" className="btn btn-outline-secondary">
        Cancel
      </RouterLink>
    </Form>
  );
}

export default function PresetEditor() {
  const { id } = useParams();
  const presetId = id === undefined ? undefined : Number(id);
  const { doc, corrupt } = useLocalConfig();
  const preset = presetId === undefined ? undefined : doc.presets.find((p) => p.id === presetId);
  const missing = presetId !== undefined && preset === undefined && !corrupt;
  const ready = presetId === undefined || preset !== undefined;

  return (
    <>
      <h1 className="mb-3">{presetId === undefined ? 'New preset' : 'Edit preset'}</h1>
      <ErrorAlert message={missing ? `Preset ${id} not found` : undefined} />
      {ready && <Editor key={presetId ?? 'new'} id={presetId} preset={preset} />}
    </>
  );
}
