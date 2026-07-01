# Spite

A FOSS, local-first email client for Microsoft 365, built with
[Tauri 2](https://tauri.app) (Rust core + Svelte UI).

## Principles

- **Standalone.** No dependency on any external service beyond Microsoft
  itself. Authentication is Microsoft MSAL / Graph only.
- **FOSS.** MIT-licensed, developed in the open.
- **Local-first.** Mail is stored locally in SQLite; the app is useful offline.
- **Graph-native.** Microsoft Graph is the mail backend.
- **Privacy-first.** No telemetry, no analytics, no phone-home. Tokens live in
  the OS keychain, never plaintext on disk.

## Layout

- `core/` — `spite-core`, a pure Rust library: Graph sync, local store, secret
  handling, offline logic.
- `src-tauri/` — the Tauri shell: window management and IPC over `spite-core`.
- `ui/` — the web frontend (Svelte 5 + TypeScript + Vite).

## Development

Prerequisites: Rust (stable), Node.js 20+, and the
[Tauri Linux system dependencies](https://tauri.app/start/prerequisites/)
(`libwebkit2gtk-4.1-dev`, GTK 3, etc.) on Linux.

```sh
npm --prefix ui install
cargo tauri dev        # or: npm --prefix ui run tauri dev
```

## Roadmap (v0.1)

- [x] Phase 0 — Tauri 2 scaffold
- [ ] Phase 1 — MSAL device-code auth, tokens in OS keychain
- [ ] Phase 2 — local SQLite store + migrations
- [ ] Phase 3 — Graph delta sync
- [ ] Phase 4 — read UI (offline-capable list + message view)
- [ ] Phase 5 — compose + send
- [ ] Phase 6 — local full-text search (FTS5)

## License

[MIT](LICENSE)
