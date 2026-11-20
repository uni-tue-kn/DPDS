// jsdom has no matchMedia; this stub answers `(prefers-color-scheme: dark)` from a switch.
import { vi } from 'vitest';

let prefersDark = false;

export function setPrefersDark(value: boolean): void {
  prefersDark = value;
  window.matchMedia = vi.fn((query: string) => ({
    matches: query.includes('prefers-color-scheme') && query.includes('dark') ? prefersDark : false,
    media: query,
    onchange: null,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(() => false),
  })) as unknown as typeof window.matchMedia;
}
