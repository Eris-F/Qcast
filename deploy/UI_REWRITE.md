# Qcast UI rewrite — design doc

**Status:** Phase 0 / planning. Locked decisions captured from the 2026-05-28
brainstorm after the first v0.1.0 Windows install test. Phases 1–5 implement.

Cross-refs: [`HANDOFF.md`](../HANDOFF.md),
[`WINDOWS_INSTALLER.md`](./WINDOWS_INSTALLER.md), the memory file
`qcast-ui-rewrite.md`, the upstream pivot memory `qcast-remote-support-pivot.md`.

---

## 1. Why this rewrite

v0.1.0 installs cleanly on Windows but the app it ships is essentially
**receiver-only**:

- `src-tauri/tauri.conf.json` points `frontendDist` at `crates/qcast-sender/web-client/`,
  which is the legacy *viewer* HTML — including the pre-pivot client-side password
  gate that no longer reflects the security model.
- `src-tauri/src/main.rs` is a one-liner `tauri::Builder::default().run(...)` —
  no role picker, no Tauri IPC commands, no tray wiring (the `tray-icon` Cargo
  feature is enabled but never used).
- The **sender** has no Tauri-side UI at all. Starting a share still requires the
  CLI path baked into `qcast-sender`.

"UI overhaul" therefore means building the missing screens, not just repainting.

## 2. Locked decisions (one-line each)

| Area | Decision |
|---|---|
| Role picker | Always show launcher on every start. No "remember" checkbox. Two big cards: **Host** / **Client**, each with a one-line formal helper underneath (e.g. "Host — Share this machine's screen and allow remote control" / "Client — Connect to and control a remote Host"). |
| Window during session | Window collapses to tray when Share starts. `skipTaskbar=true`. Tray menu = Stop sharing / Show Qcast / Quit. |
| UI framework | Vite + **Svelte 5 (runes)** + **shadcn-svelte** + **Tailwind v4** (via `@tailwindcss/vite`). Replaces hand-rolled HTML/JS in `crates/qcast-sender/web-client/`. |
| Aesthetic | **Sharp dark.** Base color `zinc`. Border-radius ≤ 4px. 1px hard borders, no soft shadows on surfaces. Kept blue accent `#4ea3ff` (gradients only on the primary CTA). |
| Pairing | Short readable code (existing `access_code::generate()`, e.g. `GHF/ABA/6TJ`), **regenerated per session**. Code IS the signalling `producer-peer-id`. **Code is consent — no Accept prompt.** |
| LAN discovery | mDNS `_qcast._tcp.local`. Sender advertises peer-id + Windows hostname. Receiver browses; **one-click join from LAN list, no code needed for LAN entries**. List **auto-refreshes every 1.5s.** Cross-LAN still uses typed-code fallback. |
| Updates | `tauri-plugin-updater` against GitHub Releases. Silent check on launch → non-blocking banner. Settings → "Check for updates" button. Signing key in `.local-secrets/qcast-updater.key.txt` (gitignored). |
| Uninstall | Existing NSIS-generated uninstaller is already correct; **no further work** this pass. |

## 3. Screen-by-screen wireframes

ASCII fidelity only. Visual polish belongs in the components, not here.

### 3.1 Launcher (every launch)

```
┌────────────────────────────────────────────────────────────┐
│  ⏺ Qcast                                       v0.2.0 ⚙   │
│                                                            │
│                                                            │
│   What would you like to do?                               │
│                                                            │
│   ┌──────────────────────┐    ┌──────────────────────┐    │
│   │                      │    │                      │    │
│   │        Host          │    │       Client         │    │
│   │                      │    │                      │    │
│   │   Share this         │    │   Connect to and     │    │
│   │   machine's screen   │    │   control a remote   │    │
│   │   and allow remote   │    │   Host.              │    │
│   │   control.           │    │                      │    │
│   └──────────────────────┘    └──────────────────────┘    │
│                                                            │
│                                                            │
└────────────────────────────────────────────────────────────┘
```

Notes:
- Top-left dot is the brand mark (animated subtle pulse).
- ⚙ opens **Settings** modal.
- Version label = clickable → "Check for updates".
- Plain-language subtitles ("let them help me…") matter more than role names.

### 3.2 Host — pre-flight

```
┌────────────────────────────────────────────────────────────┐
│  ←  Host                                              ⚙   │
│     Share this machine's screen and allow remote control.  │
│                                                            │
│   Your pairing code:                                       │
│   ┌──────────────────────────────────┐                     │
│   │       GHF / ABA / 6TJ            │   [ 📋 Copy ]      │
│   └──────────────────────────────────┘                     │
│   Read this to your friend (or they'll see you on their    │
│   network automatically).                                  │
│                                                            │
│   ── Settings for this session ────────────────────────    │
│                                                            │
│   Stop hotkey:        [ Ctrl + Alt + Q     ] [ Change ]    │
│   Let them control:   ◉ Mouse and keyboard                 │
│                       ○ View-only                          │
│                                                            │
│                                                            │
│                                  [ ▶  Start sharing ]      │
└────────────────────────────────────────────────────────────┘
```

Notes:
- **Pairing code element** is load-bearing and visually dominant.
- Default monitor = primary; multi-monitor picker is a follow-up if needed.
- Once `Start sharing` is clicked → window collapses to tray, tray icon turns
  red. A toast confirms: "Sharing started. Your code is GHF/ABA/6TJ."

### 3.3 Host — in-session (window summoned from tray)

```
┌────────────────────────────────────────────────────────────┐
│  ⏺ Sharing · 00:12:34                                ⚙   │
│                                                            │
│   Code:        GHF / ABA / 6TJ          [ 📋 Copy ]        │
│   Stop hotkey: Ctrl + Alt + Q                              │
│                                                            │
│   Connected:                                               │
│     ● Anonymous viewer · 18ms · 1.2 Mb/s                   │
│                                                            │
│                                  [ ■  Stop sharing ]       │
└────────────────────────────────────────────────────────────┘
```

### 3.4 Client — LAN list + typed code

```
┌────────────────────────────────────────────────────────────┐
│  ←  Client                                            ⚙   │
│     Connect to and control a remote Host.                  │
│                                                            │
│   Hosts on this network                (refreshing…)       │
│   ┌──────────────────────────────────────────────────┐    │
│   │  ● ERIS-DESKTOP                       [ Join ]   │    │
│   │  ● ALICE-LAPTOP                       [ Join ]   │    │
│   └──────────────────────────────────────────────────┘    │
│                                                            │
│   ── Or enter a code your friend gave you ─────────────    │
│                                                            │
│   [ G H F / A B A / 6 T J ]            [ Connect ]         │
│                                                            │
└────────────────────────────────────────────────────────────┘
```

Notes:
- The LAN list auto-refreshes **every 1.5s** (and is also push-updated when the
  backend mDNS browser fires an event). 1.5s is the worst-case visible delay
  for "Host starts sharing → Client sees them appear."
- Click `Join` on a LAN entry → connecting screen, no code prompt.
- If empty: muted text "No Hosts are sharing on this network right now."

### 3.5 Viewer (the existing screen, cleaned up)

```
┌────────────────────────────────────────────────────────────┐
│  Qcast · ● Live          18ms · 1.2 Mb/s · 1080p  [⛶]      │
│                                                            │
│                                                            │
│                  ┌────────────────────────┐                │
│                  │                        │                │
│                  │       <video>          │                │
│                  │                        │                │
│                  └────────────────────────┘                │
│                                                            │
└────────────────────────────────────────────────────────────┘
```

- Removes the password gate entirely. Auth happens at the signalling layer now.
- The status pill states from the current `app.js` carry over: `connecting`,
  `live`, `waiting`, `disconnected`.
- Letterbox + click-fullscreen behavior preserved.

### 3.6 Settings (modal)

```
┌────────────────────────────────────────────────────────────┐
│  Settings                                              ✕   │
│                                                            │
│   Updates                                                  │
│     Current version: 0.2.0                                 │
│     [ Check for updates ]                                  │
│     [✓] Check automatically on launch                      │
│                                                            │
│   Sharing                                                  │
│     Default stop hotkey:  [ Ctrl + Alt + Q ] [ Change ]    │
│                                                            │
│   About                                                    │
│     Qcast is a screen-share tool for helping friends with  │
│     their PCs. Source: github.com/Eris-F/Qcast              │
│                                                            │
└────────────────────────────────────────────────────────────┘
```

## 4. Tauri IPC command surface

All commands are **typed on both sides**. Rust side lives in a new
`src-tauri/src/commands.rs`. JS side gets a thin `src/lib/ipc.ts` wrapper.

```rust
// Pairing & sessions
#[tauri::command] fn start_share(opts: ShareOptions) -> Result<ShareSession, IpcError>;
#[tauri::command] fn stop_share() -> Result<(), IpcError>;
#[tauri::command] fn current_share() -> Option<ShareSession>;
#[tauri::command] fn connect_to_code(code: String) -> Result<(), IpcError>;
#[tauri::command] fn connect_to_lan(peer_id: String) -> Result<(), IpcError>;
#[tauri::command] fn disconnect() -> Result<(), IpcError>;

// Discovery
#[tauri::command] fn list_lan_sessions() -> Vec<LanSession>;
// + a Tauri event "lan_sessions_changed" emitted by the backend mDNS task.

// Config
#[tauri::command] fn get_settings() -> Settings;
#[tauri::command] fn update_settings(patch: SettingsPatch) -> Settings;

// Updates
#[tauri::command] fn check_for_updates() -> Result<Option<UpdateInfo>, IpcError>;
#[tauri::command] fn apply_update() -> Result<(), IpcError>;
```

```typescript
// src/lib/ipc.ts
export interface ShareOptions {
  killHotkey: string;          // e.g. "Ctrl+Alt+Q"
  allowInput: boolean;
}
export interface ShareSession {
  code: string;                // "GHF/ABA/6TJ"
  startedAt: string;           // ISO-8601
}
export interface LanSession {
  peerId: string;              // same as code
  displayName: string;         // hostname
  addr: string;                // ip:port of signalling server
  lastSeen: number;            // epoch ms
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
```

## 5. Pairing flow end-to-end (after Phase 1)

```
Sender                             Signaller (in-process)              Receiver
──────                             ────────────────────────             ────────
1. Click "Start sharing"
   → access_code::generate() = "GHF/ABA/6TJ"
   → webrtcsink producer-peer-id = "GHF/ABA/6TJ"
   → mDNS publish:
       _qcast._tcp.local
       TXT: peer-id=GHF/ABA/6TJ
       host: ERIS-DESKTOP

                                                                 mDNS browse
                                                                 → sees "ERIS-DESKTOP"
                                                                 User clicks Join
                                                                 → gstwebrtc-api
                                                                   subscribe(producer-id=
                                                                     "GHF/ABA/6TJ")
2. webrtcsink emits offer       ←  signalling routes ←  consumer arrived for
   for that consumer                                      producer "GHF/ABA/6TJ"
3. ICE relay through built-in TURN; data channel up
4. Capture + encode flowing; GstNavigation events flow back
```

Cross-LAN typed-code: receiver enters `GHF/ABA/6TJ` AND the sender's signalling
address. v1 does not solve "code carries the address too" — out of scope for
this pass, friend-group can share the address along with the code or use
ZeroTier/Tailscale to flatten the network.

## 6. mDNS service definition

```
Service type:   _qcast._tcp.local.
Instance name:  <Windows hostname>           (e.g. "ERIS-DESKTOP")
Port:           <signalling server port>     (default 8443)
TXT records:
  v=1                      schema version (fail-closed if mismatched)
  peer-id=GHF/ABA/6TJ      session id == pairing code
  app=qcast
  build=0.2.0
```

Sender publishes on share start, unpublishes on stop. Crate candidate:
`mdns-sd` (pure Rust, no avahi/bonjour dependency).

## 7. Sharp-dark theme tokens

Resolved by `npx shadcn-svelte@latest init` with base color `zinc`, then
overridden in `src/app.css`:

```css
:root {
  --radius: 4px;              /* hard corners */
  --border: oklch(0.22 0.005 270);   /* 1px hairline */
  --background: oklch(0.10 0.005 270);
  --foreground: oklch(0.95 0.005 270);
  --primary: oklch(0.62 0.18 256);   /* the kept blue accent */
  --primary-foreground: oklch(1 0 0);
  --muted: oklch(0.18 0.005 270);
  --muted-foreground: oklch(0.65 0.005 270);
  --destructive: oklch(0.60 0.22 25);
  --ring: oklch(0.62 0.18 256 / 0.4);
}
```

(OKLCH values picked to roughly match the existing `#0b0d12` / `#4ea3ff` palette;
fine-tuned in Phase 2.)

## 8. Phase boundaries — what changes per phase

| Phase | Lands | Touches |
|---|---|---|
| 0 (this doc) | docs only | `deploy/UI_REWRITE.md` |
| 1 | code-as-peer-id + mDNS publish/browse + drop session.json gate | `crates/qcast-sender/src/{host,lib,access_code,...}.rs`, new `mdns.rs` |
| 2 | Vite + Svelte + shadcn scaffold | replaces `crates/qcast-sender/web-client/` |
| 3 | All screens (launcher, share, connect, viewer, settings); IPC mocked | `crates/qcast-sender/web-client/src/**`, `src-tauri/src/commands.rs` (stubs) |
| 4 | Tray wiring + IPC backend | `src-tauri/src/{main,commands,tray}.rs`, qcast_sender lib integration |
| 5 | Updater + signing + CI | `src-tauri/Cargo.toml`, `tauri.conf.json`, `.local-secrets/`, `.github/workflows/release.yml` |

## 9. Open questions (deferred)

- Cross-LAN code-carries-address (a magic-link format like
  `qcast://ip:port/GHF/ABA/6TJ`). Not Phase 1.
- True cryptographic auth on the signaller (Path B in the brainstorm). Not Phase 1.
- Multi-monitor picker.
- Saved-friends list (the TOFU option we rejected for now).
- Wayland/Linux receiver story (deferred — Win↔Win only per the pivot memory).
