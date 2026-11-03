import type { ReactNode } from 'react';
import { Offcanvas } from 'react-bootstrap';
import { Link as RouterLink } from 'react-router';
import type { PortLabeler } from '../common/portLabel';
import { distributionName, pps, rate, time } from './linkSummary';
import type { Link } from '../api/types';

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section role="group" aria-label={title} className="mb-3">
      <h3 className="h6 text-uppercase">{title}</h3>
      <dl className="inspect-fields mb-0">{children}</dl>
    </section>
  );
}

function Item({ name, children }: { name: string; children: ReactNode }) {
  return (
    <>
      <dt>{name}</dt>
      <dd>{children}</dd>
    </>
  );
}

const onOff = (b: boolean) => (b ? 'on' : 'off');

/** "enp101s0f0 (0000:65:00.0)", or the device name alone when it has no netdev. */
function port(p: string, label: PortLabeler): string {
  const l = label(p) ?? p;
  return l === p ? p : `${l} (${p})`;
}

/** Side panel with the full configuration of one link, grouped like the editor's subsections. */
export default function LinkInspect({ link, label, onClose }: { link: Link | null; label: PortLabeler; onClose: () => void }) {
  const c = link?.config;
  const titleId = 'link-inspect-title';
  return (
    <Offcanvas show={link !== null} onHide={onClose} placement="end" animation={false} aria-labelledby={titleId}>
      {link && c && (
        <>
          <Offcanvas.Header closeButton>
            <Offcanvas.Title as="h2" id={titleId}>{link.name}</Offcanvas.Title>
          </Offcanvas.Header>
          <Offcanvas.Body>
            <RouterLink to={`/links/${link.id}`} className="btn btn-sm btn-outline-secondary mb-3" aria-label={`Edit ${link.name}`}>
              <i className="bi bi-pencil" aria-hidden="true" /> Edit
            </RouterLink>
            <dl className="inspect-fields">
              <Item name="Ingress">{port(link.ingress, label)}</Item>
              <Item name="Egress">{port(link.egress, label)}</Item>
              <Item name="Worker lcore">{link.worker == null ? 'auto' : link.worker}</Item>
            </dl>
            <Section title="Buffer">
              <Item name="Buffer capacity">{c.buffer_capacity}</Item>
              <Item name="Reordering">{onOff(c.reordering)}</Item>
              <Item name="Seed">{c.seed == null ? 'random' : c.seed}</Item>
            </Section>
            <Section title="Delayer">
              <Item name="Delay distribution">{distributionName(c.delayer.distribution)}</Item>
              <Item name="Mean delay">{time(c.delayer.mean_ns)}</Item>
              <Item name="Delay std">{time(c.delayer.std_ns)}</Item>
              <Item name="Correlation">{onOff(c.delayer.activate_correlation)}</Item>
              {c.delayer.activate_correlation && <Item name="Half-life period">{time(c.delayer.half_life_period_ns)}</Item>}
              <Item name="Initialization rate">{pps(c.delayer.initialization_rate_pps)}</Item>
            </Section>
            <Section title="Limiter">
              <Item name="Limiter">{onOff(c.limiter.activate)}</Item>
              {c.limiter.activate && (
                <>
                  <Item name="Limiter mode">{c.limiter.mode}</Item>
                  <Item name="Rate">{rate(c.limiter.rate_bps)}</Item>
                </>
              )}
            </Section>
            <Section title="Dropper">
              <Item name="Loss model">{c.loss_model.type}</Item>
              {c.loss_model.type === 'Random' ? (
                <Item name="Random loss probability">{c.loss_model.p}</Item>
              ) : (
                <>
                  <Item name="Gilbert-Elliot p">{c.loss_model.p}</Item>
                  <Item name="Gilbert-Elliot r">{c.loss_model.r}</Item>
                  <Item name="Gilbert-Elliot k">{c.loss_model.k}</Item>
                  <Item name="Gilbert-Elliot h">{c.loss_model.h}</Item>
                  <Item name="Gilbert-Elliot starts in good state">{c.loss_model.state_is_good === false ? 'no' : 'yes'}</Item>
                </>
              )}
            </Section>
            <Section title="Statistics">
              <Item name="Delay statistics">{onOff(c.delay_stats ?? true)}</Item>
              {(c.delay_stats ?? true) && (
                <>
                  <Item name="Histogram bins">{c.histogram?.bins ?? 'automatic'}</Item>
                  <Item name="Histogram lo">{c.histogram?.lo_ns == null ? 'automatic' : time(c.histogram.lo_ns)}</Item>
                  <Item name="Histogram hi">{c.histogram?.hi_ns == null ? 'automatic' : time(c.histogram.hi_ns)}</Item>
                </>
              )}
            </Section>
          </Offcanvas.Body>
        </>
      )}
    </Offcanvas>
  );
}
