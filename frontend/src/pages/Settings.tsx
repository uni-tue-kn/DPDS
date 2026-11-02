import { useEffect, useState } from 'react';
import type { FormEvent } from 'react';
import { Button, Card, Form, InputGroup } from 'react-bootstrap';
import * as api from '../api/client';
import ErrorAlert from '../components/ErrorAlert';
import { errorMessage } from '../common/format';
import type { Settings as SettingsDto } from '../api/types';

interface FormState {
  main_lcore: string;
  mlx5_devargs: string;
  /** One list row per argument. */
  eal_extra_args: string[];
  poll_interval_ms: string;
}

function toForm(s: SettingsDto): FormState {
  return {
    main_lcore: s.main_lcore == null ? '' : String(s.main_lcore),
    mlx5_devargs: s.mlx5_devargs,
    eal_extra_args: s.eal_extra_args,
    poll_interval_ms: String(s.poll_interval_ms),
  };
}

function toDto(f: FormState): SettingsDto {
  const main = f.main_lcore.trim();
  const poll = Number(f.poll_interval_ms);
  if (f.poll_interval_ms.trim() === '' || !Number.isFinite(poll)) throw new Error('Poll interval (ms) must be a number');
  if (main !== '' && !Number.isInteger(Number(main))) throw new Error('Main lcore must be an integer or empty');
  return {
    main_lcore: main === '' ? null : Number(main),
    mlx5_devargs: f.mlx5_devargs,
    eal_extra_args: f.eal_extra_args.map((a) => a.trim()).filter(Boolean),
    poll_interval_ms: poll,
  };
}

export default function Settings() {
  const [form, setForm] = useState<FormState | null>(null);
  const [error, setError] = useState<string>();
  const [saved, setSaved] = useState(false);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let alive = true;
    api.getSettings().then(
      (s) => alive && setForm(toForm(s)),
      (e: unknown) => alive && setError(errorMessage(e)),
    );
    return () => {
      alive = false;
    };
  }, []);

  const edit = (patch: Partial<FormState>) => {
    setForm((f) => (f ? { ...f, ...patch } : f));
    setSaved(false);
  };

  const submit = async (ev: FormEvent) => {
    ev.preventDefault();
    if (!form || saving) return;
    setError(undefined);
    setSaved(false);
    setSaving(true);
    try {
      await api.updateSettings(toDto(form));
      setSaved(true);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      <h1 className="mb-3">Settings</h1>
      <ErrorAlert message={error} />
      {saved && <div role="status" className="alert alert-success">Settings saved. They apply at the next start.</div>}
      {form && (
        <Card style={{ maxWidth: '40rem' }}>
          <Card.Body>
            <Form onSubmit={submit} noValidate>
              <Form.Group className="mb-3" controlId="main-lcore">
                <Form.Label>Main lcore</Form.Label>
                <Form.Control type="number" value={form.main_lcore} onChange={(e) => edit({ main_lcore: e.target.value })} />
                <Form.Text>Empty: the lowest non-isolated online core.</Form.Text>
              </Form.Group>
              <Form.Group className="mb-3" controlId="mlx5-devargs">
                <Form.Label>mlx5 devargs</Form.Label>
                <Form.Control type="text" value={form.mlx5_devargs} onChange={(e) => edit({ mlx5_devargs: e.target.value })} />
              </Form.Group>
              <fieldset className="mb-3">
                <legend className="fs-6">EAL extra arguments</legend>
                {form.eal_extra_args.map((arg, i) => (
                  // Rows have no identity beyond their position; they are controlled inputs, so index keys are safe.
                  <InputGroup key={i} className="mb-2">
                    <Form.Control
                      type="text"
                      aria-label={`EAL argument ${i + 1}`}
                      value={arg}
                      onChange={(e) => edit({ eal_extra_args: form.eal_extra_args.map((a, k) => (k === i ? e.target.value : a)) })}
                    />
                    <Button
                      variant="outline-secondary"
                      aria-label={`Remove EAL argument ${i + 1}`}
                      onClick={() => edit({ eal_extra_args: form.eal_extra_args.filter((_, k) => k !== i) })}
                    >
                      <i className="bi bi-trash" aria-hidden="true" />
                    </Button>
                  </InputGroup>
                ))}
                <Button variant="outline-secondary" size="sm" onClick={() => edit({ eal_extra_args: [...form.eal_extra_args, ''] })}>
                  Add EAL argument
                </Button>
                <Form.Text className="d-block">One argument per row, e.g. <code>--no-huge</code>, <code>-m</code>, <code>256</code>.</Form.Text>
              </fieldset>
              <Form.Group className="mb-3" controlId="poll-interval">
                <Form.Label>Poll interval (ms)</Form.Label>
                <Form.Control type="number" value={form.poll_interval_ms} onChange={(e) => edit({ poll_interval_ms: e.target.value })} />
              </Form.Group>
              <Button type="submit" variant="primary" disabled={saving}>Save</Button>
            </Form>
          </Card.Body>
        </Card>
      )}
    </>
  );
}
