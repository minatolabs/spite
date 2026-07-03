# Spite — Phase 4: Read UI (read-only)

Our numbering. Maps to `FEATURES.md` "Phase 2 — read pipeline". Hand to Fable
after Phase 3. Start in plan mode; the message-rendering section is the
high-leverage review — read it before approving.

**Numbering note (prevents the two-session collision):** on disk, Phases 0–3 =
scaffold, auth, SQLite store, delta sync (all committed). This doc = Phase 4 =
read UI. In `FEATURES.md`'s scheme this is its Phase 2. `FEATURES.md`'s "Phase
4 (mail management)" is a LATER build here and is explicitly out of scope below.

---

## Goal

Turn the working backend (auth + store + delta sync) into a usable, offline-first
reading experience that matches the approved mockup and `tokens.css`. Read-only.

**No new scopes.** Everything runs on the existing `Mail.Read`. Do **not** add
`Mail.ReadWrite`. No mark-as-read, no move/archive/delete, no flags, no drafts —
those are write ops for a later phase.

---

## Aesthetic

Adopt `tokens.css` as-is (import once in the Svelte root). Build to the approved
mockup: warm near-black chassis, oxblood reserved for selection edge / LEDs /
primary buttons, bone serif for message bodies, disciplined skeuo (bevels and
wells with restraint, not on every element). Icons: use a bundled set (Lucide or
Tabler) — the mockup's boxed glyphs were missing-font tofu; do not reproduce them.

---

## Functional scope (read-only)

1. **Folder tree.** Render from the local `folders` table. Well-known folders
   (inbox, sent, drafts, archive, junk, deleted) pinned in order; user folders
   below a stitch line. Selecting a folder filters the list. (Folder sync via
   `GET /me/mailFolders` may already be partial from Phase 3 — extend if needed,
   no new scope.)
2. **Message list.** Render from the local `messages` store, newest-first, per
   selected folder. Unread = brighter text + oxblood LED; selected = oxblood
   left-edge. Offline-first: list paints from SQLite immediately, then delta
   reconciles.
3. **Reading pane.** On select, show headers (from, to, date) and body. Body is
   **lazy-loaded**: `GET /me/messages/{id}?$select=body,uniqueBody` on first open,
   then cached to the store (`body_html`). Plain-text fallback via
   `uniqueBody.contentType`.
4. **Remove the provisional startup-sync trigger** from Phase 3. Replace with
   UI-driven sync: on folder open + on window focus + on an interval. Surface
   state in a status bar ("delta sync 12s ago"), reading the real sync_state.

---

## The security-critical run (do NOT hand-wave this)

Message HTML is hostile input. This is the part that gets an email client owned.
Non-negotiable:

- **Sanitize in Rust with `ammonia` BEFORE the webview sees a single byte.** Strip
  scripts, forms, event handlers, external form actions. No raw message HTML ever
  reaches the DOM. This is a cross-cutting invariant, not a nice-to-have.
- **Render bodies in a sandboxed iframe** (`sandbox` attribute, no
  `allow-scripts`) under a **strict CSP**.
- **Block remote images by default** (tracking-pixel defense) with a per-sender
  "load images" toggle. Rewrite/neutralize remote `src` until allowed.
- **Plain-text path** rendered as text, never as HTML.
- Test fixtures **must** include an XSS corpus (script tags, `onerror=`,
  `javascript:` URLs, CSS `expression()`, SVG payloads). Sanitizer tests are
  acceptance-blocking.

If Fable proposes rendering body HTML directly into Svelte markup, that's an
automatic reject. It goes through `ammonia` → sandboxed iframe, always.

---

## Out of scope (later phases)

- Any write op: mark-read, move, archive, delete, flag, drafts, categories,
  focused-inbox overrides. All need `Mail.ReadWrite` — a later escalation.
  Draw the Archive/Reply buttons if the layout needs them, but wire them to a
  "not yet" no-op, don't implement the Graph calls.
- Compose/send (that's a separate phase on `Mail.Send`).
- Search / FTS.
- Attachments download, threading (can be a follow-on read-only run if desired,
  but not required here — keep the run to one or two features).

---

## Acceptance

1. Offline: with the network off, folders + list + already-fetched message bodies
   render from SQLite. Opening an un-fetched body offline shows a graceful
   "not downloaded" state, not a crash.
2. Online: opening a message lazy-loads its body once, caches it, second open is
   instant and offline-capable.
3. **Sanitizer/XSS tests pass** — the corpus fixtures produce no script execution,
   no DOM injection; remote images are blocked until per-sender allow.
4. The provisional startup-sync trigger is gone; sync now runs on focus/interval
   with visible status.
5. `cargo fmt --check`, `clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`, `npm --prefix ui run check && build` all green.

---

## Your review points when the plan lands

Same discipline as before. Scrutinize three things:
1. **The rendering path** — confirm `ammonia` → sandboxed iframe → CSP, with no
   shortcut where HTML touches the DOM directly. This is the one that matters.
2. **Scope stays `Mail.Read`** — verify no `Mail.ReadWrite` sneaks in via a
   "small" write like mark-as-read.
3. **Offline-first ordering** — list/body paint from SQLite first, then reconcile.
