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

## Connecting your mailbox

Spite does not ship a client_id yet, so you currently need your own (free)
Entra app registration:

1. [Entra ID → App registrations → New registration](https://entra.microsoft.com).
   Name it anything; supported accounts: **multitenant** (any organizational
   directory).
2. Authentication → **Allow public client flows = Yes**. No client secret.
3. API permissions → Microsoft Graph → Delegated: `Mail.Read`, `Mail.Send`,
   `User.Read`. (`offline_access` is consented implicitly.)
4. Copy the **Application (client) ID** into Spite's config file:

```jsonc
// Linux: ~/.config/com.minatolabs.spite/config.json
{
  "client_id": "<your-application-client-id>",
  // optional; defaults to https://login.microsoftonline.com/common
  "authority": "https://login.microsoftonline.com/organizations",
  // optional; where the local mail database lives.
  // Defaults to the platform app-data dir, e.g.
  // ~/.local/share/com.minatolabs.spite/spite.db on Linux.
  "db_path": "/path/to/spite.db",
  // optional; how many recent Inbox messages the first sync backfills.
  // Defaults to 200, clamped to 1..=1000.
  "backfill_count": 200,
  // optional; undo-send window in seconds (Gmail-style). The message
  // actually sends only after this countdown; Undo cancels and reopens
  // the draft. Defaults to 15, clamped to 0..=120 (0 sends immediately).
  "undo_send_seconds": 15
}
```

All fields are optional overrides — with no config file, Spite uses its
defaults (and bring-your-own-client-id stays a plain config change).

Sign-in uses the OAuth 2.0 device-code flow: the app shows a code and
`microsoft.com/devicelogin`; complete sign-in in any browser. The refresh
token is stored in the OS keychain (Secret Service / Keychain / Credential
Manager), never on disk.

## Sending mail

- Sends go through Graph `sendMail` as a complete **MIME message built in
  Rust** — the JSON payload can't carry standard headers, and MIME is the
  only way to set `In-Reply-To`/`References` so replies thread correctly in
  recipients' clients. Scope is `Mail.Send` only.
- **Inline attachments are capped at 2 MB total** (Graph's 4 MB request limit
  ÷ base64 expansion). Larger files need upload sessions, which require
  `Mail.ReadWrite` — a later phase.
- **Signatures are client-side** (stored in Spite's local database, with
  separate new-message and reply variants). This is a Graph platform
  limitation: Outlook's roaming signatures are not exposed to third-party
  clients at all.

## Roadmap (v0.1)

- [x] Phase 0 — Tauri 2 scaffold
- [x] Phase 1 — device-code auth, tokens in OS keychain
- [x] Phase 2 — local SQLite store + migrations
- [x] Phase 3 — Graph delta sync
- [x] Phase 4 — read UI (offline-capable list + message view)
- [ ] Phase 5 — compose + send
- [ ] Phase 6 — local full-text search (FTS5)

## License

[MIT](LICENSE)
