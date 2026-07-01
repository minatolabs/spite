# Spite — Fable 5 Build Kickoff

Execution brief for building **Spite v0.1**. Pair with `SPITE_PLAN.md`.
Where the two conflict, the invariants below win.

---

## Guardrails (the agent must honor these)

- **[INVARIANT] Standalone.** Spite has **no dependency on any minatolabs
  service**. Authentication is **Microsoft MSAL / Graph only**.
  *(This supersedes the "identity layer / minatolabs-identity" invariant in
  older drafts of SPITE_PLAN.md — delete that line from the plan before running.)*
- **[INVARIANT] FOSS.** MIT or Apache-2.0, public repo under `github.com/minatolabs`.
- **[INVARIANT] Local-first.** Mail stored locally (SQLite); the app is useful offline.
- **[INVARIANT] Graph-native.** Microsoft Graph is the mail backend for v0.1.
- **[INVARIANT] Privacy-first.** No telemetry, no analytics, no phone-home.
- Tokens live in the **OS keychain**, never plaintext on disk.

---

## Stack

- **Tauri 2.x** — Rust core + web frontend.
- **Rust core** owns: Graph sync, local store, secret handling, offline logic.
- **Web UI** owns: rendering + interaction (the webview also renders HTML email —
  this is why Tauri is the honest lightweight choice for an email client).
- **Local store:** SQLite via a Rust binding (`rusqlite` or `sqlx`).
- **Auth:** MSAL delegated flow per mailbox; **device-code flow** when a browser
  handoff isn't available.
- **Graph scopes** (delegated, signed-in user's own mailbox):
  `Mail.Read`, `Mail.ReadWrite`, `Mail.Send`, `User.Read`, `offline_access`.
  Do **not** use `Mail.ReadBasic.All` — it's an invalid scope and throws AADSTS650053.
- **Sync:** incremental Graph **delta** queries into the local store.

---

## v0.1 scope — one vertical slice, no scope creep

**IN:** connect exactly one M365 mailbox (delegated) · pull recent N messages into
the local store · read mail offline (list + message view) · basic compose + send ·
local full-text search over stored messages.

**OUT (park these):** multiple accounts · IMAP/JMAP/Gmail · calendar/contacts ·
rules/filters · server-side push · threading heuristics · plugins · mobile.

---

## Build phases — ordered for an early working slice

Commit and checkpoint after each phase. Do **not** hand the agent all phases at once.

0. **Scaffold.** Tauri 2 app, license, README, CI stub, module layout (`core/` vs `ui/`).
   *Accept:* `cargo tauri dev` opens an empty window.
1. **Auth.** MSAL delegated device-code flow; tokens in OS keychain; refresh handling.
   *Accept:* sign in, print `me` display name from Graph `/me`.
2. **Local store.** SQLite schema (messages, threads, folders, sync_state) + migrations.
   *Accept:* schema created on first run; a unit test round-trips a message.
3. **Graph sync.** Backfill recent N + incremental delta into the store.
   *Accept:* messages land in the DB; a second sync pulls only deltas.
4. **Read UI.** Message list + message view, reading from the **local** store.
   *Accept:* airplane-mode read of already-synced mail works.
5. **Compose + send.** Minimal composer → Graph `sendMail`.
   *Accept:* a sent mail shows up in Sent.
6. **Local FTS.** Full-text search over stored messages (SQLite FTS5).
   *Accept:* a query returns hits from the local store, offline.

---

## Running this efficiently on Fable 5 + $100 (for you, not the agent)

- **Use Claude Code.** That's where files get written and where the $100 is spent.
- **Plan first.** Start each phase in plan mode; approve the plan before it writes
  code. Cheapest way to avoid expensive wrong turns.
- **One phase per session; commit after each.** Don't dump all 7 phases at once.
- **Hybrid the models to stretch the budget.** Reserve **Fable 5** for the hard
  parts — Graph delta-sync correctness, MSAL refresh/keychain, offline logic.
  Let **Sonnet / Opus** handle scaffold, schema boilerplate, and UI plumbing.
  Frontier-model tokens spent on boilerplate is where budgets die.
- **Classifier heads-up.** The new Fable 5 safety filter can bounce auth/token/OAuth
  prompts to **Opus 4.8** with a notice. That's not a block — Opus 4.8 writes that
  code fine. Just continue or rephrase.
- **Expectation:** $100 gets you a **working v0.1 vertical slice + most of the core**,
  not a polished, fully-debugged client. Budget the last mile (HTML-email quirks,
  error states, edge cases) as follow-on work.

---

## Launch prompt (paste into Claude Code, Fable 5 selected)

> You are building **Spite**, a FOSS local-first Graph-native email client in
> **Tauri 2** (Rust core + web UI). Read `SPITE_KICKOFF.md` and `SPITE_PLAN.md`.
> Enforce the invariants — especially: **no minatolabs dependency, MSAL/Graph auth
> only**. Work phase by phase from the build-phases list. Start in **plan mode** for
> Phase 0 (scaffold) and stop for my approval before writing code. Do not scope
> beyond v0.1.
