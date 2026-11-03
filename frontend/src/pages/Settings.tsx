import { useEffect, useState } from 'react';
import type { ChangeEvent, FormEvent } from 'react';
import { Button, Card, Form, InputGroup, Modal } from 'react-bootstrap';
import * as api from '../api/client';
import ErrorAlert from '../components/ErrorAlert';
import { errorMessage } from '../common/format';
import { useLoad } from '../common/useLoad';
import type { ExportDocument, Settings as SettingsDto } from '../api/types';

interface FormState {
  main_lcore: string;
  /** One list row per argument. */
  eal_extra_args: string[];
  poll_interval_ms: string;
}

function toForm(s: SettingsDto): FormState {
  return {
    main_lcore: s.main_lcore == null ? '' : String(s.main_lcore),
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
    eal_extra_args: f.eal_extra_args.map((a) => a.trim()).filter(Boolean),
    poll_interval_ms: poll,
  };
}

function readText(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result));
    r.onerror = () => reject(r.error ?? new Error('Cannot read the file'));
    r.readAsText(file);
  });
}

/** Downloads the controller's export document as `dpds-setup-<date>.json`. */
async function downloadSetup() {
  const doc = await api.exportSetup();
  const url = URL.createObjectURL(new Blob([JSON.stringify(doc, null, 2)], { type: 'application/json' }));
  const a = document.createElement('a');
  a.href = url;
  a.download = `dpds-setup-${new Date().toISOString().slice(0, 10)}.json`;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

export default function Settings() {
  const [form, setForm] = useState<FormState | null>(null);
  const [error, setError] = useState<string>();
  const [saved, setSaved] = useState(false);
  const [saving, setSaving] = useState(false);
  const emulation = useLoad(api.getEmulation);
  /** Bumped after an import so the form shows the imported settings. */
  const [round, setRound] = useState(0);
  const [pendingImport, setPendingImport] = useState<ExportDocument>();
  const [importing, setImporting] = useState(false);
  const [imported, setImported] = useState<string>();
  const stopped = emulation.data?.state === 'stopped';

  useEffect(() => {
    let alive = true;
    api.getSettings().then(
      (s) => alive && setForm(toForm(s)),
      (e: unknown) => alive && setError(errorMessage(e)),
    );
    return () => {
      alive = false;
    };
  }, [round]);

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

  const exportSetup = async () => {
    setError(undefined);
    try {
      await downloadSetup();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const chooseFile = async (ev: ChangeEvent<HTMLInputElement>) => {
    const file = ev.target.files?.[0];
    ev.target.value = ''; // the same file can be chosen again
    if (!file) return;
    setError(undefined);
    setImported(undefined);
    try {
      const doc: unknown = JSON.parse(await readText(file));
      if (typeof doc !== 'object' || doc === null || Array.isArray(doc)) throw new Error('the top level must be an object');
      setPendingImport(doc as ExportDocument);
    } catch (e) {
      setError(`"${file.name}" is not a valid JSON setup file: ${errorMessage(e)}`);
    }
  };

  const confirmImport = async () => {
    if (!pendingImport || importing) return;
    setImporting(true);
    try {
      const n = await api.importSetup(pendingImport);
      setSaved(false);
      setImported(`Setup imported: ${n.links} links and ${n.presets} presets. Settings were replaced as well.`);
      setRound((r) => r + 1);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setImporting(false);
      setPendingImport(undefined);
      emulation.reload();
    }
  };

  return (
    <>
      <h1 className="mb-3">Settings</h1>
      <ErrorAlert message={error} />
      {imported && <div role="status" className="alert alert-success">{imported}</div>}
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
      <Card className="mt-3" style={{ maxWidth: '40rem' }}>
        <Card.Body>
          <h2 className="h5">Backup</h2>
          <p className="mb-2">Export settings, links and presets as a JSON file, or restore them from one.</p>
          <Button variant="outline-secondary" className="mb-3" onClick={exportSetup}>Export setup</Button>
          <Form.Group controlId="import-file">
            <Form.Label>Import setup file</Form.Label>
            <Form.Control type="file" accept=".json,application/json" disabled={!stopped} onChange={chooseFile} />
            {emulation.data && !stopped && (
              <Form.Text data-testid="import-hint" className="d-block">
                Stop the emulation to import a setup.
              </Form.Text>
            )}
          </Form.Group>
        </Card.Body>
      </Card>
      <Modal show={pendingImport !== undefined} onHide={() => setPendingImport(undefined)} animation={false} centered>
        <Modal.Header closeButton><Modal.Title>Import setup</Modal.Title></Modal.Header>
        <Modal.Body>
          Importing will replace all existing links, settings and presets with the contents of the file. This cannot be undone.
        </Modal.Body>
        <Modal.Footer>
          <Button variant="secondary" onClick={() => setPendingImport(undefined)}>Cancel</Button>
          <Button variant="danger" disabled={importing} onClick={confirmImport}>Confirm import</Button>
        </Modal.Footer>
      </Modal>
    </>
  );
}
