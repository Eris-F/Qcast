// Tauri IPC wrapper for the Qcast frontend.
//
// Mirrors the command surface in deploy/UI_REWRITE.md §4. Each call routes
// through `invoke()` when running inside Tauri (detected via the runtime
// `__TAURI_INTERNALS__` global injected by the webview) and falls back to a
// canned mock when running under `npm run dev` so the frontend stays testable
// standalone. The real Rust handlers land in Phase 4.

import { invoke } from '@tauri-apps/api/core';

// ---------------------------------------------------------------------------
// Public types — kept in sync with the Rust IPC contract.
// ---------------------------------------------------------------------------

export interface ShareOptions {
  /** Hotkey label, e.g. "Ctrl+Alt+Q". The Tauri side will normalise/register. */
  killHotkey: string;
  /** False = view-only (host ignores the data-channel input events). */
  allowInput: boolean;
}

export interface ShareSession {
  /** Pairing code, also used as the webrtcsink producer-peer-id. */
  code: string;
  /** ISO-8601 timestamp. */
  startedAt: string;
}

export interface LanSession {
  /** mDNS-advertised peer-id, identical to the pairing code on the host. */
  peerId: string;
  /** Windows hostname of the host machine. */
  displayName: string;
  /** ip:port of the signalling server. */
  addr: string;
  /** Last time we saw this advertisement (epoch ms). */
  lastSeen: number;
}

export interface Settings {
  defaultKillHotkey: string;
  autoCheckUpdates: boolean;
}

export type SettingsPatch = Partial<Settings>;

export interface UpdateInfo {
  version: string;
  notes: string;
  publishedAt: string;
}

// ---------------------------------------------------------------------------
// Runtime detection + dispatch.
// ---------------------------------------------------------------------------

/**
 * `__TAURI_INTERNALS__` is the v2 marker the webview shim sets before any user
 * code runs. We deliberately avoid the deprecated `window.__TAURI__` shape.
 */
function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

async function call<T>(
  cmd: string,
  args: Record<string, unknown> | undefined,
  fallback?: () => T | Promise<T>,
): Promise<T> {
  if (isTauri()) {
    return invoke<T>(cmd, args);
  }
  if (fallback) {
    return await fallback();
  }
  throw new Error(`IPC "${cmd}" not mocked outside Tauri`);
}

// ---------------------------------------------------------------------------
// Mock data. Kept stable so the dev UI is reproducible.
// ---------------------------------------------------------------------------

const MOCK_CODE = 'GHF/ABA/6TJ';

const MOCK_LAN_SESSIONS: LanSession[] = [
  {
    peerId: 'MOCK/AAA/111',
    displayName: 'ERIS-DESKTOP',
    addr: '192.168.1.10:8443',
    lastSeen: Date.now(),
  },
  {
    peerId: 'MOCK/BBB/222',
    displayName: 'ALICE-LAPTOP',
    addr: '192.168.1.20:8443',
    lastSeen: Date.now(),
  },
];

const DEFAULT_SETTINGS: Settings = {
  defaultKillHotkey: 'Ctrl+Alt+Q',
  autoCheckUpdates: true,
};

// Local mock state — survives reloads only in-memory, which is enough for dev.
let mockShare: ShareSession | null = null;
let mockSettings: Settings = { ...DEFAULT_SETTINGS };

// ---------------------------------------------------------------------------
// Public IPC surface.
// ---------------------------------------------------------------------------

export const ipc = {
  startShare: (opts: ShareOptions): Promise<ShareSession> =>
    call<ShareSession>('start_share', { opts }, () => {
      mockShare = { code: MOCK_CODE, startedAt: new Date().toISOString() };
      return mockShare;
    }),

  stopShare: (): Promise<void> =>
    call<void>('stop_share', {}, () => {
      mockShare = null;
    }),

  currentShare: (): Promise<ShareSession | null> =>
    call<ShareSession | null>('current_share', {}, () => mockShare),

  connectToCode: (code: string): Promise<void> =>
    call<void>('connect_to_code', { code }, () => undefined),

  connectToLan: (peerId: string): Promise<void> =>
    call<void>('connect_to_lan', { peerId }, () => undefined),

  disconnect: (): Promise<void> => call<void>('disconnect', {}, () => undefined),

  listLanSessions: (): Promise<LanSession[]> =>
    call<LanSession[]>('list_lan_sessions', {}, () =>
      // Bump lastSeen each poll so the UI's "refreshing…" tick has something
      // to react to in dev.
      MOCK_LAN_SESSIONS.map((s) => ({ ...s, lastSeen: Date.now() })),
    ),

  getSettings: (): Promise<Settings> =>
    call<Settings>('get_settings', {}, () => ({ ...mockSettings })),

  updateSettings: (patch: SettingsPatch): Promise<Settings> =>
    call<Settings>('update_settings', { patch }, () => {
      mockSettings = { ...mockSettings, ...patch };
      return { ...mockSettings };
    }),

  checkForUpdates: (): Promise<UpdateInfo | null> =>
    call<UpdateInfo | null>('check_for_updates', {}, () => null),

  applyUpdate: (): Promise<void> => call<void>('apply_update', {}, () => undefined),
} as const;

/** App version reported in Settings → About. Vite-injected at build time. */
export const APP_VERSION = '0.2.0';
