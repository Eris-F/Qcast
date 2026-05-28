# qcast-web-client

Vite + Svelte 5 + shadcn-svelte + Tailwind v4 frontend, embedded into the
`qcast-sender` binary at compile time and served by webrtcsink's built-in web
server.

## Build

```
npm install
npm run build
```

The Rust workspace embeds `dist/` via `include_dir!`, so a built `dist/` must be
present on disk for `cargo build` to succeed. `dist/` is committed to git for
that reason; rerun `npm run build` after any source change in `src/`.

## Layout

- `src/` — Svelte 5 sources. `src/lib/components/ui/` holds the shadcn-svelte
  components. Sharp-dark theme tokens live in `src/app.css`
  (see `deploy/UI_REWRITE.md` §7).
- `public/gstwebrtc-api-3.0.0.min.js` — prebuilt gst-plugins-rs WebRTC consumer
  library. Phase 3 imports it as `/gstwebrtc-api-3.0.0.min.js` from the viewer.
- `_legacy/` — the pre-rewrite hand-rolled HTML/JS, kept as a reference for
  Phase 3's viewer port. Not loaded at runtime.

## Phase

Phase 2 of the UI rewrite (`deploy/UI_REWRITE.md`). The current `App.svelte` is
a single "It builds." landing page proving the toolchain; real screens (launcher,
share, connect, viewer, settings) land in Phase 3.
