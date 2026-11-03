import { useEffect, useState } from 'react';
import { Navigate, NavLink, Route, Routes } from 'react-router';
import * as api from './api/client';
import Dashboard from './pages/Dashboard';
import Hints from './pages/Hints';
import Interfaces from './pages/Interfaces';
import PresetEditor from './pages/PresetEditor';
import Presets from './pages/Presets';
import LinkEditor from './pages/LinkEditor';
import Links from './pages/Links';
import Settings from './pages/Settings';
import { startPolling } from './common/polling';
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
  { to: '/presets', label: 'Presets', icon: 'bi-sliders', end: false },
];

function Sidebar({ theme, onToggle }: { theme: Theme; onToggle: () => void }) {
  const [info, setInfo] = useState<OnlineView>();
  // Re-polled so "Offline" recovers (and turns back into "Offline" when the controller goes away).
  useEffect(() => {
    let alive = true;
    const stop = startPolling(async () => {
      try {
        const o = await api.getOnline();
        if (alive) setInfo(o);
      } catch {
        if (alive) setInfo(undefined);
      }
    }, ONLINE_POLL_MS, true);
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
        <div className="nav-label" title={info?.dpds_bin}>
          {info ? `v${info.version}` : 'Offline'}
          {info && !info.telemetry && <span className="badge rounded-pill text-bg-warning ms-2">no telemetry</span>}
        </div>
      </div>
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
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/links" element={<Links />} />
          <Route path="/links/new" element={<LinkEditor />} />
          <Route path="/links/:id" element={<LinkEditor />} />
          <Route path="/interfaces" element={<Interfaces />} />
          <Route path="/hints" element={<Hints />} />
          <Route path="/presets" element={<Presets />} />
          <Route path="/presets/new" element={<PresetEditor />} />
          <Route path="/presets/:id" element={<PresetEditor />} />
          <Route path="/settings" element={<Settings />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </main>
    </div>
  );
}
