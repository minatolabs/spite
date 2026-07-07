# Spite — Phase 8 (Run A): Mailbox Settings core

Our numbering. Maps to `FEATURES.md` "Phase 6 — mailbox settings" (Run A: the
non-rules items; **rules is Run B**, deferred). Hand to Fable after Phase 7.
**Manual approve** — this writes account-level settings. Start in plan mode.

---

## Goal

Automatic replies (out-of-office), master categories (create/rename/color), and
reading working-hours/timezone/date-format. This also completes the Phase 7
category feature by giving it a real managed palette instead of raw strings.

**Scope escalation.** Adds `MailboxSettings.ReadWrite` (delegated) — the first
new scope since Phase 7. Handle re-consent gracefully (below). Rules (a
messageRules engine) are **out of scope for this run** — Run B.

---

## Prerequisites — the consent escalation (partly manual)

1. **Portal (on you):** add **`MailboxSettings.ReadWrite`** (delegated) to the
   Spite Entra app registration. Exactly that scope — not `.All`, not shared.
2. **Code — graceful re-consent** (reuse the Phase 7 pattern): request the new
   scope set, detect consent-required / missing-scope (403 / interaction
   error), trigger the device-code re-consent with a clear message
   ("Spite needs permission to manage your mailbox settings"), and continue the
   session without a full re-setup.

---

## Functional scope

1. **Automatic replies (out-of-office).**
   - `GET /me/mailboxSettings` → `automaticRepliesSetting`; `PATCH` to update.
   - UI: on/off, scheduled window (start/end datetime) vs "on until I turn off,"
     separate **internal** and **external** reply bodies, and the external
     audience toggle (`externalAudience`: none / contactsOnly / all).
   - Small HTML/plain editor for the bodies (reuse the compose editor's sanitize
     path for any HTML shown/stored).
   - Reflect current server state on open; optimistic update with rollback on
     failure (reuse the Phase 7 pipeline).

2. **Master categories (completes Phase 7 categories).**
   - `GET /me/outlook/masterCategories`, `POST` to create, `PATCH` to
     rename/recolor, `DELETE` to remove.
   - Graph categories use a fixed preset color set (`preset0`…`preset24`). Map
     those presets onto the Spite palette per `tokens.css`: categories get the
     **brass and verdigris** treatments; **oxblood stays reserved** for the
     app's own accent. Don't invent colors outside the Graph preset set.
   - Wire into Phase 7's `+ category` assignment: the assign UI now picks from
     managed master categories (name + mapped color chip) instead of free text.
   - CRUD UI in a settings pane: list, add, rename, recolor, delete.

3. **Working hours / timezone / date format (read).**
   - Read `workingHours`, `timeZone`, and date/time format from
     `/me/mailboxSettings`. Store them.
   - No UI beyond surfacing timezone/format where dates render (optional). The
     real consumer is the **calendar phase** — this read is groundwork so
     calendar renders in the user's own timezone. Just read + store correctly.

---

## Safety / consistency

- Optimistic + rollback for auto-replies and category CRUD (reuse Phase 7).
- Deleting a master category must not corrupt messages already tagged with it —
  decide and document behavior (Graph leaves the string on messages; surface it
  gracefully, don't crash the assign UI on an unknown/removed category).
- Sanitize any HTML in auto-reply bodies through the existing `ammonia` path.

---

## Out of scope

- **Rules** (`messageRules` engine + conditions/actions UI) — Run B, same scope.
- Calendar (its own scope/phase; this only *reads* working-hours for it).
- Any new scope beyond `MailboxSettings.ReadWrite`.

---

## Acceptance

1. Adding `MailboxSettings.ReadWrite` triggers a clean re-consent on next
   sign-in; session continues after consent.
2. Auto-replies: set a scheduled OOF with distinct internal/external bodies and
   audience → persists on the server (verify on outlook.com), and reflects back
   correctly on reopen; turning it off works.
3. Master categories: create/rename/recolor/delete round-trips to Graph; the
   Phase 7 `+ category` picker now shows managed categories with mapped colors;
   a deleted category doesn't break the assign UI.
4. Working-hours/timezone/date-format read and stored (log or surface to prove
   it).
5. Failure path: a rejected auto-reply or category write rolls back, banner
   shown.
6. `cargo fmt --check`, `clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`, `npm --prefix ui run check && build` all green.

---

## Your review points when the plan lands

1. **Re-consent flow** — scope change triggers graceful re-consent, not a silent
   403 or forced re-setup.
2. **Category color mapping** — presets map onto brass/verdigris per tokens,
   oxblood stays reserved; assign UI survives a deleted/unknown category.
3. **Scope is exactly `MailboxSettings.ReadWrite`** — nothing broader.
4. **Auto-reply HTML sanitized** through the existing path.
5. **Optimistic rollback** reused, not reinvented.
