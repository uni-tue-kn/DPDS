import { useState } from 'react';
import type { FormEvent } from 'react';
import { Button, Card, Col, Form, Row } from 'react-bootstrap';
import { Link as RouterLink, useLocation, useNavigate, useParams } from 'react-router';
import * as api from '../api/client';
import ErrorAlert from '../components/ErrorAlert';
import { errorMessage } from '../common/format';
import { useLoad } from '../common/useLoad';
import { saveLink, useLocalConfig } from '../common/localConfig';
import ConfigFieldset from './ConfigFieldset';
import { FieldLabel, RequiredNote } from './FieldLabel';
import { configToForm, defaultConfigForm, formToDto, parseWorker, portError } from './linkForm';
import type { ConfigForm } from './linkForm';
import type { Interface, Link, LinkInput } from '../api/types';

/** Port -> name of the link that uses it, per role. */
type Users = Map<string, string>;

function optionLabel(i: Interface, usedBy?: string, role?: string): string {
  const base = i.netdev ?? i.pci;
  const reasons: string[] = [];
  if (!i.dpds_usable || i.uplink) reasons.push(i.reason ?? (i.uplink ? 'uplink' : 'not usable'));
  if (usedBy !== undefined) reasons.push(`already used as ${role} by ${usedBy}`);
  return reasons.length === 0 ? base : `${base} - ${reasons.join('; ')}`;
}

/** A picker over the discovered interfaces, shown by netdev (device name without one). */
function PortField({
  name,
  id,
  value,
  interfaces,
  users,
  onChange,
}: {
  name: 'Ingress' | 'Egress';
  id: string;
  value: string;
  interfaces: Interface[];
  users: Users;
  onChange: (v: string) => void;
}) {
  const role = name.toLowerCase();
  // A stored port that is not a discovered interface (e.g. a vdev) stays selectable as its own option.
  const unknown = value !== '' && !interfaces.some((i) => i.pci === value);
  return (
    <Col md={6} className="mb-3">
      <Form.Group controlId={id}>
        <FieldLabel help={name} required />
        <Form.Select value={value} onChange={(e) => onChange(e.target.value)} required aria-required="true">
          <option value="">Select a port...</option>
          {unknown && <option value={value}>{value}</option>}
          {interfaces.map((i) => (
            <option key={i.pci} value={i.pci} disabled={!i.dpds_usable || i.uplink || users.has(i.pci)}>
              {optionLabel(i, users.get(i.pci), role)}
            </option>
          ))}
        </Form.Select>
      </Form.Group>
    </Col>
  );
}

function Editor({
  id,
  link,
  interfaces,
  others,
}: {
  id?: number;
  link?: Link;
  interfaces: Interface[];
  others: Link[];
}) {
  const navigate = useNavigate();
  const [name, setName] = useState(link?.name ?? '');
  const [ingress, setIngress] = useState(link?.ingress ?? '');
  const [egress, setEgress] = useState(link?.egress ?? '');
  const [config, setConfig] = useState<ConfigForm>(() =>
    link ? configToForm(link.config, link.avg_frame_size_byte) : defaultConfigForm(),
  );
  const [worker, setWorker] = useState(link?.worker == null ? '' : String(link.worker));
  const [error, setError] = useState<string>();

  // Every port is ingress of at most one link and egress of at most one link (the controller enforces it, 409).
  const ingressUsers: Users = new Map();
  const egressUsers: Users = new Map();
  for (const l of others) {
    ingressUsers.set(l.ingress, l.name);
    egressUsers.set(l.egress, l.name);
  }

  const submit = (ev: FormEvent) => {
    ev.preventDefault();
    setError(undefined);
    let input: LinkInput;
    try {
      if (name.trim() === '') throw new Error('Name is required');
      if (!LINK_NAME.test(name.trim())) {
        throw new Error('Name must be 1 to 32 characters: letters A-Z a-z, digits, underscore, dot or dash');
      }
      if (others.some((l) => l.name === name.trim())) throw new Error(`A link named "${name.trim()}" already exists`);
      const ports = [
        ['Ingress', ingress],
        ['Egress', egress],
      ];
      for (const [role, port] of ports) {
        if (port === '') throw new Error(`Select an ${role.toLowerCase()} port`);
        const bad = portError(port);
        if (bad) throw new Error(`${role}: ${bad}`);
      }
      input = {
        name: name.trim(),
        ingress: ports[0][1],
        egress: ports[1][1],
        ...formToDto(config),
        worker: parseWorker(worker),
      };
    } catch (e) {
      setError(errorMessage(e));
      return;
    }
    try {
      saveLink(input, id);
      navigate('/links');
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
          <Row>
            <Form.Group as={Col} md={6} className="mb-3" controlId="link-name">
              <FieldLabel label="Name" help="Link name" required />
              <Form.Control
                type="text"
                value={name}
                onChange={(e) => setName(e.target.value)}
                required
                aria-required="true"
              />
            </Form.Group>
            <Form.Group as={Col} md={6} className="mb-3" controlId="link-worker">
              <FieldLabel help="Worker lcore" />
              <Form.Control type="number" min={0} step={1} value={worker} onChange={(e) => setWorker(e.target.value)} />
              <Form.Text>Empty: automatic.</Form.Text>
            </Form.Group>
          </Row>
          <Row>
            <PortField
              name="Ingress"
              id="link-ingress"
              value={ingress}
              interfaces={interfaces}
              users={ingressUsers}
              onChange={setIngress}
            />
            <PortField
              name="Egress"
              id="link-egress"
              value={egress}
              interfaces={interfaces}
              users={egressUsers}
              onChange={setEgress}
            />
          </Row>
        </Card.Body>
      </Card>
      <ConfigFieldset title="Link configuration" value={config} onChange={setConfig} />
      <Button type="submit" variant="primary" className="me-2">
        Save
      </Button>
      <RouterLink to="/links" className="btn btn-outline-secondary">
        Cancel
      </RouterLink>
    </Form>
  );
}

/** The controller's link-name rule (`valid_name` in `api.rs`). */
const LINK_NAME = /^[A-Za-z0-9_.-]{1,32}$/;

/** The first free `<name>_copy`, `<name>_copy2`, ...; the base is shortened so the result has at most 32 characters. */
function copyName(name: string, taken: Link[]): string {
  const used = new Set(taken.map((l) => l.name));
  for (let n = 1; ; n += 1) {
    const suffix = n === 1 ? '_copy' : `_copy${n}`;
    const candidate = name.slice(0, 32 - suffix.length) + suffix;
    if (!used.has(candidate)) return candidate;
  }
}

export default function LinkEditor() {
  const { id } = useParams();
  // "Duplicate" in the Links list opens the new-link editor with `{ copyOf: <link id> }` as router state.
  const copyOf = (useLocation().state as { copyOf?: number } | null)?.copyOf;
  const linkId = id === undefined ? undefined : Number(id);
  const interfaces = useLoad(api.getInterfaces);
  const { doc, corrupt } = useLocalConfig();
  const link = linkId === undefined ? undefined : doc.links.find((l) => l.id === linkId);
  const source = linkId === undefined ? doc.links.find((l) => l.id === copyOf) : undefined;
  const copy = source && { ...source, name: copyName(source.name, doc.links), ingress: '', egress: '' };
  const missing = linkId !== undefined && link === undefined && !corrupt;
  const ready = interfaces.data !== undefined && !missing && (linkId === undefined || link !== undefined);

  return (
    <>
      <h1 className="mb-3">{linkId === undefined ? 'New link' : 'Edit link'}</h1>
      <ErrorAlert message={interfaces.error ?? (missing ? `Link ${id} not found` : undefined)} />
      {ready && (
        <Editor
          key={linkId ?? 'new'}
          id={linkId}
          link={link ?? copy}
          interfaces={interfaces.data ?? []}
          others={doc.links.filter((l) => l.id !== linkId)}
        />
      )}
    </>
  );
}
