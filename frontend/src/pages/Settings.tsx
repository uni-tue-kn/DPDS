import { useState } from 'react';
import type { ChangeEvent, FormEvent } from 'react';
import { Button, Card, Form, InputGroup, Modal } from 'react-bootstrap';
import ErrorAlert from '../components/ErrorAlert';
import { errorMessage } from '../common/format';
import {
  emptyConfig, isLocalConfig, parseStored, readRaw, updateConfig, useLocalConfig, writeConfig,
} from '../common/localConfig';
import type { LocalConfig, Settings as SettingsDto } from '../api/types';

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
  if (f.poll_interval_ms.trim() === '' || !Number.isInteger(poll) || poll < 1) {
    throw new Error('Poll interval (ms) must be a whole number of at least 1');
  }
  if (main !== '' && (!Number.isInteger(Number(main)) || Number(main) < 0)) {
    throw new Error('Main lcore must be a whole number of at least 0, or empty');
  }
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

/** Downloads the local document as `dpds-setup-<date>.json`: as stored, ids included (an unreadable value as it is). */
function downloadSetup() {
  const raw = readRaw();
  const stored = parseStored(raw);
  const text = raw !== null && stored.corrupt ? raw : JSON.stringify(raw === null ? emptyConfig() : stored.doc, null, 2);
  const url = URL.createObjectURL(new Blob([text], { type: 'application/json' }));
  const a = document.createElement('a');
  a.href = url;
  a.download = `dpds-setup-${new Date().toISOString().slice(0, 10)}.json`;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

export default function Settings() {
  const { doc } = useLocalConfig();
  const [form, setForm] = useState<FormState>(() => toForm(doc.settings));
  const [error, setError] = useState<string>();
  const [saved, setSaved] = useState(false);
  const [pendingImport, setPendingImport] = useState<LocalConfig>();
  const [imported, setImported] = useState<string>();

  const edit = (patch: Partial<FormState>) => {
    setForm((f) => (f ? { ...f, ...patch } : f));
    setSaved(false);
  };

  const submit = (ev: FormEvent) => {
    ev.preventDefault();
    setError(undefined);
    setSaved(false);
    try {
      const settings = toDto(form);
      updateConfig((d) => ({ ...d, settings }));
      setSaved(true);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const exportSetup = () => {
    setError(undefined);
    try {
      downloadSetup();
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
    let parsed: unknown;
    try {
      parsed = JSON.parse(await readText(file));
    } catch (e) {
      setError(`"${file.name}" is not a valid JSON setup file: ${errorMessage(e)}`);
      return;
    }
    if (isLocalConfig(parsed)) setPendingImport(parsed);
    else setError(`"${file.name}" is not a DPDS setup file (a version 1 document with links, presets and settings)`);
  };

  const confirmImport = () => {
    if (!pendingImport) return;
    writeConfig(pendingImport);
    setForm(toForm(pendingImport.settings));
    setSaved(false);
    setImported(`Setup imported: ${pendingImport.links.length} links and ${pendingImport.presets.length} presets. Settings were replaced as well.`);
    setPendingImport(undefined);
  };

  return (
    <>
      <h1 className="mb-3">Settings</h1>
      <ErrorAlert message={error} />
      {imported && <div role="status" className="alert alert-success">{imported}</div>}
      {saved && <div role="status" className="alert alert-success">Settings saved. They apply at the next start.</div>}
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
            <Button type="submit" variant="primary">Save</Button>
          </Form>
        </Card.Body>
      </Card>
      <Card className="mt-3" style={{ maxWidth: '40rem' }}>
        <Card.Body>
          <h2 className="h5">Backup</h2>
          <p className="mb-2">Export settings, links and presets as a JSON file, or restore them from one. They are kept in this browser only.</p>
          <Button variant="outline-secondary" className="mb-3" onClick={exportSetup}>Export setup</Button>
          <Form.Group controlId="import-file">
            <Form.Label>Import setup file</Form.Label>
            <Form.Control type="file" accept=".json,application/json" onChange={chooseFile} />
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
          <Button variant="danger" onClick={confirmImport}>Confirm import</Button>
        </Modal.Footer>
      </Modal>
    </>
  );
}
