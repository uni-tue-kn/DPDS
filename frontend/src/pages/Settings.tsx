import { useRef, useState } from 'react';
import type { ChangeEvent, FormEvent } from 'react';
import { Button, Card, Form, InputGroup, Modal } from 'react-bootstrap';
import ErrorAlert from '../components/ErrorAlert';
import { errorMessage } from '../common/format';
import { FieldLabel, InfoTip } from './FieldLabel';
import {
  isLinksFile,
  linksProblem,
  parseStored,
  readRaw,
  replaceLinks,
  updateConfig,
  useLocalConfig,
} from '../common/localConfig';
import type { Link, Settings as SettingsDto } from '../api/types';

interface FormState {
  main_lcore: string;
  /** One list row per argument; the id keeps a row's input when rows before it are removed. */
  eal_extra_args: { id: number; value: string }[];
  poll_interval_ms: string;
}

let nextRowId = 0;
const newRow = (value: string) => ({ id: nextRowId++, value });

function toForm(s: SettingsDto): FormState {
  return {
    main_lcore: s.main_lcore == null ? '' : String(s.main_lcore),
    eal_extra_args: s.eal_extra_args.map(newRow),
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
    eal_extra_args: f.eal_extra_args.map((a) => a.value.trim()).filter(Boolean),
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

const pad = (n: number) => String(n).padStart(2, '0');
const localDate = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;

/**
 * Downloads `{version: 1, links}` as `dpds-links-<local date>.json`: the links as stored, ids included, no settings.
 * An unreadable store is the rescue case: its stored text is downloaded unchanged.
 */
function downloadLinks() {
  const raw = readRaw();
  const stored = parseStored(raw);
  const text = stored.corrupt ? (raw ?? '') : JSON.stringify({ version: 1, links: stored.doc.links }, null, 2);
  const url = URL.createObjectURL(new Blob([text], { type: 'application/json' }));
  const a = document.createElement('a');
  a.href = url;
  a.download = `dpds-links-${localDate(new Date())}.json`;
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
  const [pendingImport, setPendingImport] = useState<Link[]>();
  const fileInput = useRef<HTMLInputElement>(null);
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

  const exportLinks = () => {
    setError(undefined);
    try {
      downloadLinks();
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
      setError(`"${file.name}" is not a valid JSON links file: ${errorMessage(e)}`);
      return;
    }
    if (!isLinksFile(parsed)) {
      setError(`"${file.name}" is not a DPDS links file (a version 1 document with a list of links)`);
      return;
    }
    const problem = linksProblem(parsed.links);
    if (problem !== null) setError(`"${file.name}" cannot be imported. ${problem}`);
    else setPendingImport(parsed.links as Link[]);
  };

  const confirmImport = () => {
    if (!pendingImport) return;
    try {
      replaceLinks(pendingImport);
      setImported(`Imported ${pendingImport.length} links`);
    } catch (e) {
      setError(errorMessage(e));
    }
    setPendingImport(undefined);
  };

  return (
    <>
      <div className="d-flex justify-content-between align-items-center mb-3">
        <h1 className="mb-0">Settings</h1>
        <div className="text-end">
          <input
            ref={fileInput}
            type="file"
            accept=".json"
            aria-label="Import links file"
            className="d-none"
            onChange={chooseFile}
          />
          <Button variant="success" className="me-2" onClick={() => fileInput.current?.click()}>
            <i className="bi bi-cloud-arrow-down-fill" aria-hidden="true" /> Import
          </Button>
          <Button variant="danger" onClick={exportLinks}>
            <i className="bi bi-cloud-arrow-up-fill" aria-hidden="true" /> Export
          </Button>
        </div>
      </div>
      <ErrorAlert message={error} />
      {imported && (
        <div role="status" className="alert alert-success">
          {imported}
        </div>
      )}
      {saved && (
        <div role="status" className="alert alert-success">
          Settings saved. They apply at the next start.
        </div>
      )}
      <Card style={{ maxWidth: '40rem' }}>
        <Card.Body>
          <Form onSubmit={submit} noValidate>
            <Form.Group className="mb-3" controlId="main-lcore">
              <FieldLabel help="Main lcore" />
              <Form.Control
                type="number"
                value={form.main_lcore}
                onChange={(e) => edit({ main_lcore: e.target.value })}
              />
              <Form.Text>Empty: the lowest non-isolated online core.</Form.Text>
            </Form.Group>
            <fieldset className="mb-3">
              <legend className="fs-6">EAL extra arguments</legend>
              {form.eal_extra_args.map((arg, i) => (
                <InputGroup key={arg.id} className="mb-2">
                  <Form.Control
                    type="text"
                    aria-label={`EAL argument ${i + 1}`}
                    value={arg.value}
                    onChange={(e) =>
                      edit({
                        eal_extra_args: form.eal_extra_args.map((a) =>
                          a.id === arg.id ? { ...a, value: e.target.value } : a,
                        ),
                      })
                    }
                  />
                  <Button
                    variant="outline-secondary"
                    aria-label={`Remove EAL argument ${i + 1}`}
                    onClick={() => edit({ eal_extra_args: form.eal_extra_args.filter((a) => a.id !== arg.id) })}
                  >
                    <i className="bi bi-trash" aria-hidden="true" />
                  </Button>
                </InputGroup>
              ))}
              <Button
                variant="outline-secondary"
                size="sm"
                onClick={() => edit({ eal_extra_args: [...form.eal_extra_args, newRow('')] })}
              >
                Add EAL argument
              </Button>
              <Form.Text className="d-block">
                One argument per row, e.g. <code>--no-huge</code>, <code>-m</code>, <code>256</code>.
                {/* Not in the <legend>: a button there would become part of the group's accessible name. */}
                <InfoTip help="EAL extra arguments" />
              </Form.Text>
            </fieldset>
            <Form.Group className="mb-3" controlId="poll-interval">
              <FieldLabel help="Poll interval (ms)" />
              <Form.Control
                type="number"
                value={form.poll_interval_ms}
                onChange={(e) => edit({ poll_interval_ms: e.target.value })}
              />
            </Form.Group>
            <Button type="submit" variant="primary">
              Save
            </Button>
          </Form>
        </Card.Body>
      </Card>
      <Modal show={pendingImport !== undefined} onHide={() => setPendingImport(undefined)} animation={false} centered>
        <Modal.Header closeButton>
          <Modal.Title>Import links</Modal.Title>
        </Modal.Header>
        <Modal.Body>
          Importing will replace all existing links with the links of the file. The settings stay. This cannot be
          undone.
        </Modal.Body>
        <Modal.Footer>
          <Button variant="secondary" onClick={() => setPendingImport(undefined)}>
            Cancel
          </Button>
          <Button variant="danger" onClick={confirmImport}>
            Confirm import
          </Button>
        </Modal.Footer>
      </Modal>
    </>
  );
}
