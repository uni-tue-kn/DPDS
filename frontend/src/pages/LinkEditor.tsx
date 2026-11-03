import { useState } from 'react';
import type { FormEvent } from 'react';
import { Button, Card, Col, Form, Row } from 'react-bootstrap';
import { Link as RouterLink, useNavigate, useParams } from 'react-router-dom';
import * as api from '../api/client';
import ErrorAlert from '../components/ErrorAlert';
import { errorMessage } from '../common/format';
import { useLoad } from '../common/useLoad';
import ConfigFieldset from './ConfigFieldset';
import SavePresetModal from './SavePresetModal';
import { configToDto, configToForm, defaultConfigForm, parseWorker, portError } from './linkForm';
import type { ConfigForm } from './linkForm';
import type { Interface, Link, Preset } from '../api/types';

/** Port -> name of the link that uses it, per role. */
type Users = Map<string, string>;

function optionLabel(i: Interface, usedBy?: string, role?: string): string {
  const base = i.netdev ? `${i.pci} (${i.netdev})` : i.pci;
  const reasons: string[] = [];
  if (!i.dpds_usable || i.kernel_route) reasons.push(i.reason ?? (i.kernel_route ? 'management NIC' : 'not usable'));
  if (usedBy !== undefined) reasons.push(`already used as ${role} by ${usedBy}`);
  return reasons.length === 0 ? base : `${base} - ${reasons.join('; ')}`;
}

/** A picker over the discovered interfaces plus a free-text field; a typed value wins (see `Editor`). */
function PortField({ name, id, value, typed, interfaces, users, onChange, onType }: {
  name: string; id: string; value: string; typed: string; interfaces: Interface[]; users: Users;
  onChange: (v: string) => void; onType: (v: string) => void;
}) {
  const role = name.toLowerCase();
  // A stored port that is not a discovered interface (e.g. a vdev) stays selectable as its own option.
  const unknown = value !== '' && !interfaces.some((i) => i.pci === value);
  return (
    <Col md={6} className="mb-3">
      <Form.Group controlId={id}>
        <Form.Label>{name}</Form.Label>
        <Form.Select value={value} onChange={(e) => onChange(e.target.value)}>
          <option value="">Select a port...</option>
          {unknown && <option value={value}>{value}</option>}
          {interfaces.map((i) => (
            <option key={i.pci} value={i.pci} disabled={!i.dpds_usable || i.kernel_route || users.has(i.pci)}>
              {optionLabel(i, users.get(i.pci), role)}
            </option>
          ))}
        </Form.Select>
      </Form.Group>
      <Form.Group controlId={`${id}-typed`} className="mt-2">
        <Form.Label>{`${name} (typed)`}</Form.Label>
        <Form.Control type="text" value={typed} onChange={(e) => onType(e.target.value)} placeholder="0000:65:00.0 or net_null0" />
        <Form.Text>Overrides the picker when not empty.</Form.Text>
      </Form.Group>
    </Col>
  );
}

function Editor({ id, link, interfaces, others, presets, onPresetSaved }: {
  id?: number; link?: Link; interfaces: Interface[]; others: Link[]; presets: Preset[]; onPresetSaved: () => void;
}) {
  const navigate = useNavigate();
  const [name, setName] = useState(link?.name ?? '');
  const [ingress, setIngress] = useState(link?.ingress ?? '');
  const [egress, setEgress] = useState(link?.egress ?? '');
  const [typedIngress, setTypedIngress] = useState('');
  const [typedEgress, setTypedEgress] = useState('');
  const [config, setConfig] = useState<ConfigForm>(() => (link ? configToForm(link.config) : defaultConfigForm()));
  const [worker, setWorker] = useState(link?.worker == null ? '' : String(link.worker));
  const [error, setError] = useState<string>();
  const [saving, setSaving] = useState(false);
  const [presetOpen, setPresetOpen] = useState(false);
  const [presetSaved, setPresetSaved] = useState<string>();

  // Every port is ingress of at most one link and egress of at most one link (the controller enforces it, 409).
  const ingressUsers: Users = new Map();
  const egressUsers: Users = new Map();
  for (const l of others) {
    ingressUsers.set(l.ingress, l.name);
    egressUsers.set(l.egress, l.name);
  }

  const submit = async (ev: FormEvent) => {
    ev.preventDefault();
    if (saving) return;
    setError(undefined);
    let input;
    try {
      if (name.trim() === '') throw new Error('Name is required');
      // A typed value wins over the picker.
      const ports = [['Ingress', typedIngress.trim() || ingress], ['Egress', typedEgress.trim() || egress]];
      for (const [role, port] of ports) {
        if (port === '') throw new Error(`Select an ${role.toLowerCase()} port`);
        const bad = portError(port);
        if (bad) throw new Error(`${role}: ${bad}`);
      }
      input = {
        name: name.trim(), ingress: ports[0][1], egress: ports[1][1], config: configToDto(config), worker: parseWorker(worker),
      };
    } catch (e) {
      setError(errorMessage(e));
      return;
    }
    setSaving(true);
    try {
      if (id === undefined) await api.createLink(input);
      else await api.updateLink(id, input);
      navigate('/links');
    } catch (e) {
      setError(errorMessage(e));
      setSaving(false);
    }
  };

  return (
    <>
      <Form onSubmit={submit} noValidate>
        <ErrorAlert message={error} />
        {presetSaved && <div role="status" className="alert alert-success">{presetSaved}</div>}
        <Card className="mb-3">
          <Card.Body>
            <Row>
              <Form.Group as={Col} md={6} className="mb-3" controlId="link-name">
                <Form.Label>Name</Form.Label>
                <Form.Control type="text" value={name} onChange={(e) => setName(e.target.value)} />
              </Form.Group>
              <Form.Group as={Col} md={6} className="mb-3" controlId="link-worker">
                <Form.Label>Worker lcore</Form.Label>
                <Form.Control type="number" min={0} step={1} value={worker} onChange={(e) => setWorker(e.target.value)} />
                <Form.Text>Empty: automatic.</Form.Text>
              </Form.Group>
            </Row>
            <Row>
              <PortField
                name="Ingress" id="link-ingress" value={ingress} typed={typedIngress} interfaces={interfaces}
                users={ingressUsers} onChange={setIngress} onType={setTypedIngress}
              />
              <PortField
                name="Egress" id="link-egress" value={egress} typed={typedEgress} interfaces={interfaces}
                users={egressUsers} onChange={setEgress} onType={setTypedEgress}
              />
            </Row>
          </Card.Body>
        </Card>
        <ConfigFieldset
          title="Link configuration" value={config} onChange={setConfig} presets={presets}
          onSaveAsPreset={() => { setPresetSaved(undefined); setPresetOpen(true); }}
        />
        <Button type="submit" variant="primary" className="me-2" disabled={saving}>Save</Button>
        <RouterLink to="/links" className="btn btn-outline-secondary">Cancel</RouterLink>
      </Form>
      {/* Outside the <Form>: React submit events bubble through portals and would submit the link. */}
      {presetOpen && (
        <SavePresetModal
          getConfig={() => configToDto(config)}
          onClose={() => setPresetOpen(false)}
          onSaved={(p) => {
            setPresetOpen(false);
            setPresetSaved(`Preset "${p.name}" saved.`);
            onPresetSaved();
          }}
        />
      )}
    </>
  );
}

export default function LinkEditor() {
  const { id } = useParams();
  const linkId = id === undefined ? undefined : Number(id);
  const interfaces = useLoad(api.getInterfaces);
  // The editor works without presets: a failed load just leaves the "Apply preset" selects empty.
  const presets = useLoad(api.getPresets);
  // Only used to disable ports other links hold: the editor works without it, the controller answers 409 anyway.
  const links = useLoad(api.getLinks);
  const link = useLoad(() => (linkId === undefined ? Promise.resolve(undefined) : api.getLink(linkId)));
  const ready = interfaces.data !== undefined && (links.data !== undefined || links.error !== undefined) && (linkId === undefined || link.data !== undefined);

  return (
    <>
      <h1 className="mb-3">{linkId === undefined ? 'New link' : 'Edit link'}</h1>
      <ErrorAlert message={interfaces.error ?? link.error} />
      {ready && (
        <Editor
          key={linkId ?? 'new'} id={linkId} link={link.data} interfaces={interfaces.data ?? []}
          others={(links.data ?? []).filter((l) => l.id !== linkId)}
          presets={presets.data ?? []} onPresetSaved={presets.reload}
        />
      )}
    </>
  );
}
