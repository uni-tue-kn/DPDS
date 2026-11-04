import { useEffect, useRef, useState } from 'react';
import { Button, Modal, OverlayTrigger, Tooltip } from 'react-bootstrap';
import { Navigate, NavLink, Route, Routes } from 'react-router';
import * as api from './api/client';
import Dashboard from './pages/Dashboard';
import Hints from './pages/Hints';
import Interfaces from './pages/Interfaces';
import LinkEditor from './pages/LinkEditor';
import Links from './pages/Links';
import Settings from './pages/Settings';
import { resetConfig, useLocalConfig } from './common/localConfig';
import { startPolling } from './common/polling';
import { isUpdateAvailable, RELEASES_URL } from './common/updateCheck';
import type { OnlineView } from './api/types';

const ONLINE_POLL_MS = 5000;

type Theme = 'dark' | 'light';

function initialTheme(): Theme {
  const stored = localStorage.getItem('theme');
  if (stored === 'dark' || stored === 'light') return stored;
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

const NAV = [
  { to: '/', label: 'Dashboard', icon: 'bi-speedometer2', end: true },
  { to: '/links', label: 'Links', icon: 'bi-diagram-2', end: false },
  { to: '/interfaces', label: 'Interfaces', icon: 'bi-ethernet', end: false },
  { to: '/hints', label: 'Host hints', icon: 'bi-lightbulb', end: false },
  { to: '/settings', label: 'Settings', icon: 'bi-gear-wide-connected', end: false },
];

function Sidebar({ theme, onToggle }: { theme: Theme; onToggle: () => void }) {
  const [info, setInfo] = useState<OnlineView>();
  const [updateAvailable, setUpdateAvailable] = useState(false);
  const [showUpdateModal, setShowUpdateModal] = useState(false);
  // The update check runs once per page load, after the first successful /api/online, not once per poll.
  const checked = useRef(false);
  // Re-polled so "Offline" recovers (and turns back into "Offline" when the controller goes away).
  useEffect(() => {
    let alive = true;
    const stop = startPolling(
      async () => {
        try {
          const o = await api.getOnline();
          if (alive) setInfo(o);
          if (!checked.current) {
            checked.current = true;
            // Not awaited: a slow GitHub must not hold up the /api/online polling.
            void isUpdateAvailable(o.version).then((update) => {
              if (alive && update) {
                setUpdateAvailable(true);
                setShowUpdateModal(true);
              }
            });
          }
        } catch {
          if (alive) setInfo(undefined);
        }
      },
      ONLINE_POLL_MS,
      true,
    );
    return () => {
      alive = false;
      stop();
    };
  }, []);
  return (
    <div className="sidebar">
      <div className="sidebar-brand">DPDS</div>
      <nav aria-label="Main navigation" className="nav flex-column">
        {NAV.map((n) => (
          <NavLink key={n.to} to={n.to} end={n.end} className="nav-link">
            <i className={`bi ${n.icon}`} aria-hidden="true" />
            <span className="nav-label">{n.label}</span>
          </NavLink>
        ))}
      </nav>
      <div className="sidebar-footer">
        <button
          type="button"
          className="btn btn-link theme-toggle mb-2"
          aria-label="Toggle dark mode"
          aria-pressed={theme === 'dark'}
          onClick={onToggle}
        >
          <i className={`bi ${theme === 'dark' ? 'bi-sun' : 'bi-moon-stars'}`} aria-hidden="true" />
        </button>
        <a
          href="/api/docs"
          target="_blank"
          rel="noopener noreferrer"
          className="btn btn-link theme-toggle mb-2"
          title="API documentation"
        >
          <i className="bi bi-book" aria-hidden="true" />
          <span className="visually-hidden">API documentation</span>
        </a>
        <div className="nav-label" title={info?.dpds_bin}>
          {info ? `v${info.version}` : 'Offline'}
          {info && updateAvailable && (
            <OverlayTrigger
              placement="top"
              overlay={<Tooltip id="update-available-tooltip">New release available</Tooltip>}
            >
              <a href={RELEASES_URL} target="_blank" rel="noopener noreferrer" className="text-danger ms-2">
                <i className="bi bi-arrow-up-circle-fill" aria-hidden="true" />
                <span className="visually-hidden">New DPDS release available</span>
              </a>
            </OverlayTrigger>
          )}
          {info && !info.telemetry && <span className="badge rounded-pill text-bg-warning ms-2">no telemetry</span>}
        </div>
      </div>
      <Modal show={showUpdateModal} onHide={() => setShowUpdateModal(false)} centered>
        <Modal.Header closeButton>
          <Modal.Title>Update available</Modal.Title>
        </Modal.Header>
        <Modal.Body>
          A new stable DPDS release is available. Update with ./dpds.sh update --stable (see the releases page).
        </Modal.Body>
        <Modal.Footer>
          <Button variant="secondary" onClick={() => setShowUpdateModal(false)}>
            Close
          </Button>
          <Button variant="primary" href={RELEASES_URL} target="_blank" rel="noopener noreferrer">
            Open releases
          </Button>
        </Modal.Footer>
      </Modal>
    </div>
  );
}

/** The stored configuration cannot be read: say so on every page and let the user reset it (it is never dropped silently). */
function CorruptConfigAlert() {
  const { corrupt } = useLocalConfig();
  if (!corrupt) return null;
  return (
    <div role="alert" className="alert alert-danger d-flex align-items-center gap-3">
      <span>
        The local configuration of this browser cannot be read (damaged, or written by a newer version), so nothing is
        shown and nothing can be started. It stays untouched until you reset it.
      </span>
      <Button variant="danger" size="sm" className="text-nowrap" onClick={resetConfig}>
        Reset local configuration
      </Button>
    </div>
  );
}

export default function App() {
  const [theme, setTheme] = useState<Theme>(initialTheme);

  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme);
    document.documentElement.setAttribute('data-bs-theme', theme);
  }, [theme]);

  const toggle = () => {
    const next: Theme = theme === 'dark' ? 'light' : 'dark';
    localStorage.setItem('theme', next);
    setTheme(next);
  };

  return (
    <div className="app-shell">
      <Sidebar theme={theme} onToggle={toggle} />
      <main className="main-content">
        <CorruptConfigAlert />
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/links" element={<Links />} />
          <Route path="/links/new" element={<LinkEditor />} />
          <Route path="/links/:id" element={<LinkEditor />} />
          <Route path="/interfaces" element={<Interfaces />} />
          <Route path="/hints" element={<Hints />} />
          <Route path="/settings" element={<Settings />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </main>
    </div>
  );
}
