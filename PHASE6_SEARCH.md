# Spite — Phase 6: Search and Triage (FTS5)

Our numbering. Maps to `FEATURES.md` "Phase 5 — search and triage".
(`FEATURES.md` Phase 6 = mailbox settings, a later build with a new scope — not
this.) Hand to Fable after Phase 5. Start in plan mode; the index-sync design is
the high-leverage review.

---

## Goal

Instant, offline, first-class search over local mail, plus a server-search
fallback for deep/old mail, plus filters, saved searches, and a search-focused
keyboard model. Built on SQLite **FTS5**. This is the part where Spite beats
Outlook on feel.

**No new scope.** Local FTS and server search both run on the existing
`Mail.Read`. **Read-only** this phase: keyboard *navigation* + search, with write
triage verbs stubbed (see keyboard section). Do **not** add `Mail.ReadWrite`.

---

## The key architectural decision: entity-agnostic index

Build ONE unified search index, not a mail-only one, so calendar events and
contacts plug in later with no rearchitecture:

```
CREATE VIRTUAL TABLE search_index USING fts5(
  entity_type UNINDEXED,   -- 'mail' now; 'event', 'contact' later
  entity_id   UNINDEXED,   -- FK back to the source row
  title,                   -- subject / event title / contact name
  subtitle,                -- from/to / event location / contact org
  body,                    -- sanitized text body / event notes
  ts          UNINDEXED,   -- unix epoch for recency ranking
  tokenize = 'unicode61 remove_diacritics 2'
);
```

Only `entity_type = 'mail'` is populated this phase. This is what makes calendar
search work the instant calendar sync exists — **do not** build a mail-only FTS
table.

---

## Functional scope

1. **Local FTS5 index (the core).**
   - Migration **v5**: create `search_index` + keep it in sync with `messages`
     via triggers (insert/update/delete), and **backfill** existing messages.
   - Index the **sanitized text** of bodies (strip HTML to text before indexing —
     don't tokenize `<div>` tags), plus subject, from name/address, to/cc.
   - Use `unicode61` (diacritic-insensitive) with prefix indexing for
     search-as-you-type. Consider the `trigram` tokenizer only if substring
     matching is wanted; note the tradeoff in the plan.

2. **Instant search UI.**
   - Search-as-you-type over the local index, ranked by `bm25()` blended with
     recency (`ts`). Results with `snippet()`/`highlight()` excerpts.
   - Scope toggle: current folder vs all mail.
   - Fully offline; paints from SQLite with no network.

3. **Filters and saved views.**
   - Chips over search + browse: unread, flagged, has-attachment, date-range,
     from/to.
   - Saved searches persisted locally (`saved_searches` table): name + query +
     filters.

4. **Server search (deep fallback).**
   - `POST /search/query`, entityType `message`, KQL (`from:`, `subject:`,
     `hasAttachment:true`, `received>=…`).
   - Trigger on explicit "search everywhere" action or when local hits are thin.
   - **Merge + dedupe against local results by `internetMessageId`.**
   - No new scope (`Mail.Read`).

5. **Keyboard model (read-only this phase).**
   - Active now: `/` focus search, `j`/`k` navigate results/list, `Enter` open,
     `Esc` clear/close, `r` reply, `c` compose (reuse Phase 5).
   - **Stubbed** (show a "needs mail management" hint, no-op): `e` archive,
     `#` delete, `s` flag — all `Mail.ReadWrite`, deferred to that phase.
   - Configurable, vim-flavored by default.

---

## Calendar search: architected, not active

The unified `search_index` is calendar-ready. But calendar search **cannot
function this phase** — there is no calendar data in the store and no
`Calendars.Read` consent. So:

- Build the index and UI to accommodate `entity_type = 'event'`, but do **not**
  fake calendar results or add calendar scope here.
- When the calendar phase lands (calendar sync + `Calendars.Read`), event rows
  get indexed into `search_index` and calendar search activates with **zero
  changes to the search layer**.
- Add a test proving extensibility: insert a non-mail `entity_type` row and
  confirm it's found. That's the guarantee the architecture holds.

---

## Out of scope

- Write triage verbs (archive/delete/flag) — `Mail.ReadWrite`, later phase.
- Actual calendar data/scope — later phase (search layer is ready for it).
- Semantic / vector search — deferred to the "smart" tier, and only ever with a
  **local** embedding model (never a cloud embedding API — that breaks the
  privacy invariant). Note in FEATURES.md as a future candidate on top of FTS.
- People-API autocomplete.

---

## Acceptance

1. Migration v5 creates `search_index`, triggers keep it consistent with
   `messages`, existing mail backfilled. Idempotent.
2. Instant local search returns ranked, highlighted results **offline**.
3. Filters (unread/flagged/attachment/date/from) apply over search.
4. Saved searches persist and reload locally.
5. Server search returns deep results, **deduped against local by
   `internetMessageId`**, on `Mail.Read`.
6. Keyboard: `/` focuses, `j/k` navigate, `Enter` opens, `Esc` clears; write
   verbs visibly stubbed, not wired.
7. Extensibility test: a non-mail `entity_type` row is indexed and found.
8. `cargo fmt --check`, `clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`, `npm --prefix ui run check && build` all green.

---

## Your review points when the plan lands

1. **Index consistency** — confirm the triggers (or sync-time population) keep
   `search_index` exactly in step with `messages`, no stale or orphaned index
   rows after edits/deletes. This is the correctness core of the phase.
2. **Entity-agnostic schema** — confirm the index is genuinely multi-entity
   (`entity_type`), not mail-only, so calendar doesn't force a rebuild later.
3. **Scope stays `Mail.Read`** — including server search; no `Mail.ReadWrite`.
4. **Bodies indexed as sanitized text**, not raw HTML.
5. **Migration v5** — additive (new table/triggers), doesn't alter `messages`.

---

## Run-size note

This is large (local FTS + server + filters + saved + keyboard). A natural split
if you want it reviewable: **Run A** = local FTS5 index + instant search +
filters + keyboard nav (the core feel win); **Run B** = server search + saved
searches. Your call given the Fable budget — one run or two.
