# Spite — Phase 3: Graph Delta Sync

The heart of the app and the hardest phase. Hand to Fable after Phase 2. Start
in **plan mode**, and read the delta strategy in the plan carefully before
approving — this is where subtle correctness bugs live.

---

## Goal

Populate the local store with Inbox mail from Microsoft Graph: an initial
backfill of recent N messages, then **incremental delta sync** on every
subsequent run that pulls only changes. `core/src/sync.rs` (the reserved stub)
gets its real implementation.

Scope to the **Inbox** for v0.1. Multi-folder is a later extension; don't build
it now.

---

## Design for testability first (this is what makes the hard phase safe)

Delta logic is hard to get right and painful to test against live Graph. So
abstract the network behind a trait, exactly like `MailStore`/`TokenStore`:

- **`MailSource` trait** — the fetch surface sync depends on: fetch a delta page
  (given an optional cursor), follow pagination, return messages + removals +
  the next cursor.
- **`GraphMailSource`** — the real HTTP implementation (extends the minimal
  `graph.rs` client from Phase 1 with the message/delta endpoints, auth header,
  and throttling).
- **`MockMailSource`** — returns canned delta pages for unit tests.

Sync orchestration (`sync.rs`) then runs against the trait, so the whole
algorithm — pagination, deletions, cursor storage, upserts — is unit-testable
with **no live Graph**. Build this seam; don't skip it.

---

## The delta algorithm (contract)

**Initial sync (no stored cursor):**
1. Ensure the Inbox `folders` row exists (fetch `/me/mailFolders/inbox`).
2. Backfill **recent N** messages (N from config, default 200), ordered newest
   first, `$select` limited to summary fields (below). Upsert into the store.
3. Establish the delta baseline and store its cursor in `sync_state.delta_link`
   keyed by the Inbox folder id.

**Incremental sync (stored cursor exists):**
1. Call delta with the stored cursor.
2. Page through `@odata.nextLink` until `@odata.deltaLink`.
3. For each item: changed/new → upsert; `@removed` annotation → delete locally.
4. Store the new `@odata.deltaLink` cursor. Second run with no server changes
   should pull ~zero items.

**`$select` (keep payloads small):** `id`, `subject`, `from`,
`receivedDateTime`, `bodyPreview`, `isRead`, `hasAttachments`, `conversationId`,
`parentFolderId`. **Do not fetch full bodies in sync** — `body_html` stays null
and is lazy-loaded when a message is opened (Phase 4). This matches the
summary/full split from Phase 2.

**Mapping:** Graph JSON → domain `Message`. Handle nulls (e.g. `from` can be
null on drafts), parse `receivedDateTime` to unix epoch.

---

## Open questions — RESOLVE against current Graph docs (Context7 / web), don't guess

These are the exact spots where memory goes stale. Fable must confirm the
current behavior and **state its chosen approach in the plan** for review:

1. **Baseline without full enumeration.** How to get a delta cursor representing
   "state as of now, changes from here" for Inbox messages, so the recent-N
   backfill isn't duplicated by a full-folder delta walk. Check whether
   `messages/delta` supports a "latest token" shortcut; if not, define the
   fallback (e.g., a one-time enumeration whose upserts are harmless because the
   store is idempotent) and note the bandwidth cost.
2. **Exact delta endpoint + query shape** for a single mail folder, including how
   `$select` and paging interact with `messages/delta`.
3. **Deletion signal** — confirm the `@removed` shape and that a delete maps to
   removing the row locally.

---

## Throttling + auth resilience

- **429 / Retry-After:** Graph throttles. Honor the `Retry-After` header with
  backoff. Make this testable via `MockMailSource` (429 then success).
- **401 mid-sync:** refresh the access token (Phase 1 silent path) and retry the
  request. Sync should acquire a fresh token up front and recover from expiry.

---

## Triggering it (provisional, no real UI yet)

UI is Phase 4. For acceptance, expose a `sync_mailbox` Tauri command and trigger
it **once on startup after silent sign-in**, with `tracing` logs emitting counts
(fetched, upserted, removed, cursor stored). Mark the auto-trigger clearly as
provisional — Phase 4 replaces it with UI-driven sync. No buttons, no list view
this phase.

---

## Config touch (justified)

- Add optional **`backfill_count`** (default 200) to `AppConfig`, documented in
  the README.
- **Scopes stay a compile-time constant** (the strict Phase 1 set). The earlier
  "scopes in config" idea is **dropped** — hand-editable OAuth scopes are a
  footgun, not a feature.

---

## Acceptance

1. **Unit tests via `MockMailSource`:** initial backfill upserts N; `nextLink`
   pagination assembles multiple pages; a `@removed` item deletes locally; a
   changed item (e.g. `isRead` flip) updates in place; `deltaLink` lands in
   `sync_state`; a second sync from the stored cursor applies only the mock's
   delta page; a 429-then-200 sequence retries and succeeds.
2. **Live run:** signed in from Phase 1, startup sync populates `spite.db` —
   query shows Inbox messages matching your recent mail, ordered newest-first.
3. **Second live run:** pulls only deltas (few/zero), `delta_link` updated. Prove
   it's incremental, not a re-pull, via the tracing counts.
4. `cargo fmt --check`, `clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`, and `npm --prefix ui run check && build` all green.

---

## Out of scope

No read UI / list view (Phase 4), no full-body fetch (Phase 4 lazy load), no
send (Phase 5), no FTS index (Phase 6), no folders beyond Inbox, no threading,
no multi-account. `graph.rs` grows only the message/delta surface it needs.

---

## Note for your review

Low classifier-reroute risk (network code, not auth secrets). The real risk is
**delta correctness**. When the plan comes back, scrutinize three things: the
backfill-plus-baseline strategy (open question 1), how deletions are handled, and
where/when the cursor is stored. Those three are what separate a sync that
converges from one that silently drifts. That's your high-leverage review, same
role the schema played in Phase 2.
