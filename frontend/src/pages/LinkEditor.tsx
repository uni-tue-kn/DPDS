import { useState } from 'react';
import type { FormEvent } from 'react';
import { Button, Card, Col, Form, Row } from 'react-bootstrap';
import { Link as RouterLink, useNavigate, useParams } from 'react-router-dom';
import * as api from '../api/client';
import ErrorAlert from '../components/ErrorAlert';
import { directionLabel, errorMessage } from '../common/format';
import { useLoad } from '../common/useLoad';
import DirectionForm from './DirectionForm';
import { configToDto, configToForm, defaultConfigForm, parseWorkers, portError } from './linkForm';
import type { ConfigForm } from './linkForm';
import type { Interface, Link } from '../api/types';

function optionLabel(i: Interface): string {
  const base = i.netdev ? `${i.pci} (${i.netdev})` : i.pci;
  if (i.dpds_usable && !i.kernel_route) return base;
  return `${base} - ${i.reason ?? (i.kernel_route ? 'management NIC' : 'not usable')}`;
}

/** A picker over the discovered interfaces plus a free-text field; a typed value wins (see `Editor`). */
function PortField({ name, id, value, typed, interfaces, onChange, onType }: {
  name: string; id: string; value: string; typed: string; interfaces: Interface[];
  onChange: (v: string) => void; onType: (v: string) => void;
}) {
  // A stored port that is not a discovered interface (e.g. a vdev) stays selectable as its own option.
  const unknown = value !== '' && !interfaces.some((i) => i.pci === value);
  return (
    <Col md={4} className="mb-3">
      <Form.Group controlId={id}>
        <Form.Label>{name}</Form.Label>
        <Form.Select value={value} onChange={(e) => onChange(e.target.value)}>
          <option value="">Select a port...</option>
          {unknown && <option value={value}>{value}</option>}
          {interfaces.map((i) => (
            <option key={i.pci} value={i.pci} disabled={!i.dpds_usable || i.kernel_route}>{optionLabel(i)}</option>
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

function Editor({ id, link, interfaces }: { id?: number; link?: Link; interfaces: Interface[] }) {
  const navigate = useNavigate();
  const [name, setName] = useState(link?.name ?? '');
  const [portCount, setPortCount] = useState(link ? Math.min(link.ports.length, 2) : 2);
  const [ports, setPorts] = useState<string[]>([link?.ports[0] ?? '', link?.ports[1] ?? '']);
  const [typed, setTyped] = useState<string[]>(['', '']);
  const [configs, setConfigs] = useState<ConfigForm[]>(() => [
    link?.configs[0] ? configToForm(link.configs[0]) : defaultConfigForm(),
    link?.configs[1] ? configToForm(link.configs[1]) : defaultConfigForm(),
  ]);
  const [workers, setWorkers] = useState(link?.workers?.join(', ') ?? '');
  const [error, setError] = useState<string>();
  const [saving, setSaving] = useState(false);

  // A typed value wins over the picker.
  const effective = ports.map((p, i) => typed[i].trim() || p);

  const submit = async (ev: FormEvent) => {
    ev.preventDefault();
    if (saving) return;
    setError(undefined);
    let input;
    try {
      if (name.trim() === '') throw new Error('Name is required');
      const chosen = effective.slice(0, portCount);
      if (chosen.some((p) => p === '')) throw new Error('Select a port for every port of the link');
      chosen.forEach((p, i) => {
        const bad = portError(p);
        if (bad) throw new Error(`Port ${i === 0 ? 'A' : 'B'}: ${bad}`);
      });
      input = {
        name: name.trim(),
        ports: chosen,
        configs: configs.slice(0, portCount).map((c, d) => configToDto(c, d)),
        workers: parseWorkers(workers),
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

  const setPort = (i: number, v: string) => setPorts((p) => p.map((x, k) => (k === i ? v : x)));
  const setType = (i: number, v: string) => setTyped((p) => p.map((x, k) => (k === i ? v : x)));
  const setConfig = (i: number, c: ConfigForm) => setConfigs((cs) => cs.map((x, k) => (k === i ? c : x)));
  // Direction 0 is A -> B, direction 1 is B -> A; a single port loops back onto itself.
  const dirTitle = (d: number) => (portCount === 1
    ? directionLabel(0, effective[0], effective[0])
    : directionLabel(d, effective[d], effective[1 - d]));

  return (
    <Form onSubmit={submit} noValidate>
      <ErrorAlert message={error} />
      <Card className="mb-3">
        <Card.Body>
          <Row>
            <Form.Group as={Col} md={4} className="mb-3" controlId="link-name">
              <Form.Label>Name</Form.Label>
              <Form.Control type="text" value={name} onChange={(e) => setName(e.target.value)} />
            </Form.Group>
            <Form.Group as={Col} md={4} className="mb-3" controlId="port-count">
              <Form.Label>Number of ports</Form.Label>
              <Form.Select value={String(portCount)} onChange={(e) => setPortCount(Number(e.target.value))}>
                <option value="1">1 (loopback)</option>
                <option value="2">2</option>
              </Form.Select>
            </Form.Group>
            <Form.Group as={Col} md={4} className="mb-3" controlId="link-workers">
              <Form.Label>Worker lcores</Form.Label>
              <Form.Control type="text" value={workers} onChange={(e) => setWorkers(e.target.value)} />
              <Form.Text>Comma separated, one per direction. Empty: automatic.</Form.Text>
            </Form.Group>
          </Row>
          <Row>
            <PortField
              name="Port A" id="port-a" value={ports[0]} typed={typed[0]} interfaces={interfaces}
              onChange={(v) => setPort(0, v)} onType={(v) => setType(0, v)}
            />
            {portCount === 2 && (
              <PortField
                name="Port B" id="port-b" value={ports[1]} typed={typed[1]} interfaces={interfaces}
                onChange={(v) => setPort(1, v)} onType={(v) => setType(1, v)}
              />
            )}
          </Row>
        </Card.Body>
      </Card>
      {configs.slice(0, portCount).map((c, d) => (
        <DirectionForm key={d} index={d} title={dirTitle(d)} value={c} onChange={(next) => setConfig(d, next)} />
      ))}
      <Button type="submit" variant="primary" className="me-2" disabled={saving}>Save</Button>
      <RouterLink to="/links" className="btn btn-outline-secondary">Cancel</RouterLink>
    </Form>
  );
}

export default function LinkEditor() {
  const { id } = useParams();
  const linkId = id === undefined ? undefined : Number(id);
  const interfaces = useLoad(api.getInterfaces);
  const link = useLoad(() => (linkId === undefined ? Promise.resolve(undefined) : api.getLink(linkId)));
  const ready = interfaces.data !== undefined && (linkId === undefined || link.data !== undefined);

  return (
    <>
      <h1 className="mb-3">{linkId === undefined ? 'New link' : 'Edit link'}</h1>
      <ErrorAlert message={interfaces.error ?? link.error} />
      {ready && <Editor key={linkId ?? 'new'} id={linkId} link={link.data} interfaces={interfaces.data ?? []} />}
    </>
  );
}
