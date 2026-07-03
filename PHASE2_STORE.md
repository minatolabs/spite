# Spite — Phase 2: Local SQLite Store

Hand to Fable after Phase 1. Start in plan mode, review the **schema** carefully,
approve, then let it build. One feature: the storage layer.

---

## Goal

A local SQLite store that's created on first run, with a `MailStore` trait
(real SQLite backend + in-memory test backend) mirroring the `TokenStore`
pattern from Phase 1. Schema, migrations, domain types, and round-trip tests.

**No network, no Graph, no UI.** This phase only builds the layer that Phase 3
(sync) fills and Phase 4 (UI) displays. The app still shows the signed-in
screen; the store is exercised by tests, not the UI, this phase.

---

## No external prerequisites

Unlike Phase 1, nothing is blocked on you. No portal work, no config to supply.
This phase can run start-to-finish and hit acceptance on its own.

---

## Decisions

- **Binding: `rusqlite` with the `bundled` feature.** Simpler than `sqlx` for a
  local file DB, no async runtime needed for queries, and consistent with the
  `spawn_blocking` pattern you already use for the keychain. Call blocking DB
  ops via `spawn_blocking` when invoked from async code.
- **Enable an FTS5-capable build now** (bundled rusqlite includes FTS5) so
  Phase 6 search needs no dependency change. Do **not** create the FTS index
  yet — that's Phase 6.
- **Migrations: versioned via `PRAGMA user_version`** (hand-rolled stepping or a
  light crate like `rusqlite_migration`). Must be idempotent — running twice is
  a no-op, not an error.
- **DB location: Tauri `app_data_dir`** (e.g.
  `~/.local/share/com.minatolabs.spite/spite.db` on Linux), overridable via
  config (see below).
- Use **Context7** for current `rusqlite` / migration-crate APIs — don't trust
  training-data signatures.

---

## Schema (v0.1 minimal)

Keep it lean. Store forward-compat fields (conversation_id) but don't use them.

**`folders`**
| col | type | note |
|---|---|---|
| id | TEXT PK | Graph folder id |
| display_name | TEXT | |
| well_known_name | TEXT null | e.g. `inbox`, `sentitems` |
| parent_id | TEXT null | |

**`messages`**
| col | type | note |
|---|---|---|
| id | TEXT PK | Graph message id |
| folder_id | TEXT | FK → folders.id |
| conversation_id | TEXT null | stored, unused until threading |
| subject | TEXT | |
| from_name | TEXT | |
| from_address | TEXT | |
| received_at | INTEGER | unix epoch (sortable) |
| preview | TEXT | Graph `bodyPreview` |
| is_read | INTEGER | bool |
| has_attachments | INTEGER | bool |
| body_html | TEXT null | fetched lazily / Phase 3+ |

**`sync_state`** (created now, populated in Phase 3)
| col | type | note |
|---|---|---|
| folder_id | TEXT PK | |
| delta_link | TEXT null | Graph delta cursor |
| last_synced_at | INTEGER null | |

Single-mailbox for v0.1 — no `accounts` table. Structure so adding account
scoping later is a migration, not a redesign (don't hard-wire assumptions that
block it).

---

## Domain types + trait (mirror TokenStore)

Domain types live in `core/`, decoupled from Graph JSON (mapping happens in
Phase 3). Split summary vs full so the list view stays cheap:

- `MessageSummary` — id, folder_id, subject, from_name, from_address,
  received_at, preview, is_read, has_attachments (no body).
- `Message` — full, includes `body_html`.
- `Folder`, `SyncState`.

`MailStore` trait (async or blocking-behind-spawn_blocking, your call):
- `upsert_folders(&[Folder])`, `list_folders() -> Vec<Folder>`
- `upsert_messages(&[Message])` (idempotent by PK)
- `list_messages(folder_id, limit, offset) -> Vec<MessageSummary>` (ordered by
  `received_at` desc)
- `get_message(id) -> Option<Message>`
- `get_sync_state(folder_id) -> Option<SyncState>`, `set_sync_state(SyncState)`

Two impls, exactly like Phase 1's token store:
- **`SqliteMailStore`** — the real backend.
- **`MemoryMailStore`** — for tests.

This lands in `core/src/store.rs` — the stub reserved since Phase 0 finally gets
its real implementation.

---

## Config touch (small, justified — not a refactor)

The store needs a DB location, so this is the natural place for a minimal config
addition, nothing more:

- Add optional **`db_path`** to `AppConfig` (defaults to `app_data_dir`).
- Document the full `config.json` schema in the README: `client_id`,
  `authority`, optional `db_path`. Reaffirm bring-your-own-client-id already
  works via `client_id`/`authority` overrides.

The larger scopes-in-config idea stays parked for Phase 3, where sync actually
uses the scopes. **Do not** refactor the working Phase 1 config beyond adding
`db_path`.

---

## Acceptance test

1. First run creates the DB at `app_data_dir` with all three tables and
   `user_version` set.
2. Migrations are idempotent — second run doesn't error or duplicate.
3. Round-trip unit tests on **both** `MemoryMailStore` and `SqliteMailStore`:
   upsert folders + messages → `list_messages` returns them newest-first →
   `get_message` returns the full record → upsert same id again updates, not
   duplicates.
4. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace` all green. `npm --prefix ui run check && build`
   still clean (no UI change expected).

---

## Out of scope for Phase 2

No Graph/network, no sync, no UI, no FTS index (build capability only), no send,
no drafts, no threading logic, no multi-account. `sync.rs` stub stays untouched
until Phase 3.

---

## Note

This phase is pure local code — low risk of Fable classifier reroutes (unlike
Phase 1's auth). The one thing worth your careful review in the plan is the
**schema**, since changing it after data exists means writing migrations. Eyeball
the columns and types before approving.
