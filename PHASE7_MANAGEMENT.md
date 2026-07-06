# Spite — Phase 7: Mail Management (scope escalation: `Mail.ReadWrite`)

Our numbering. Maps to `FEATURES.md` "Phase 4 — mail management". Hand to Fable
after Phase 6. **Manual approve** — this writes to and deletes from the real
mailbox. Start in plan mode; the optimistic-UI reconciliation is the high-leverage
review.

---

## Goal

Turn Spite from reader-plus-sender into a full triage client: change read state,
flag, move, archive, delete, real drafts, large attachments, focused inbox,
categories. Un-stub the `e`/`#`/`s` keyboard verbs from Phase 6.

**This is a scope escalation.** It adds the `Mail.ReadWrite` delegated scope —
the first new consent since Phase 1. Handle it deliberately (below). This is the
honest cost of being a real mail client, not a read-only viewer; state it plainly
in the README.

---

## Prerequisites — the consent escalation (partly manual)

1. **Portal (on you):** add **`Mail.ReadWrite`** (delegated) to the Spite Entra
   app registration. Same as Phase 1.
2. **Code — handle re-consent gracefully.** Because requested scopes changed,
   the existing token doesn't cover `Mail.ReadWrite`; the next sign-in needs
   fresh consent. Spite must:
   - Request the new scope set on auth.
   - Detect a missing-scope / consent-required condition (e.g. 403 or the
     interaction/consent error) and **trigger the device-code re-consent flow**
     with a clear message ("Spite needs permission to manage your mail"), not a
     silent failure.
   - Not lose the user's session — re-consent, don't force a full re-setup.

---

## The correctness core: optimistic UI + reconciliation

Every write here should feel instant but must stay honest with the server. Reuse
the pending-queue discipline from the undo-send work:

- Apply the change to the **local store immediately** (optimistic), fire the
  Graph call in the background.
- On success → confirmed; on **failure → roll back the local change** and surface
  a non-blocking error. Never leave the UI showing a state the server rejected.
- The next **delta sync reconciles** anything that drifted. Local changes are
  provisional until confirmed/reconciled.

This is the thing to get right. A mark-read or archive that silently diverges
from the server is exactly the bug that erodes trust in a mail client.

---

## Functional scope

1. **Read state + flags.** `PATCH /me/messages/{id}` for `isRead`;
   `flag.flagStatus` for flag/complete. Optimistic, reconciled on delta. This
   also lights up the **`flagged` chip** that Phase 6 left disabled.
2. **Move / archive / delete.** `POST /me/messages/{id}/move` with
   `destinationId`: archive = move to the `archive` well-known folder; delete =
   move to `deleteditems`; permanent delete = `DELETE`. **Undo toast** with a
   short window before the call fires (same pattern as undo-send).
3. **True drafts.** `POST /me/messages` to create, `PATCH` to autosave,
   `POST /me/messages/{id}/send` to send. **Replaces Phase 5's manual reply
   construction** with `createReply` / `createReplyAll` / `createForward` (now
   available with `Mail.ReadWrite`). Drafts sync to the Drafts folder and are
   editable across sessions.
4. **Large attachments.** Upload sessions
   (`POST /me/messages/{id}/attachments/createUploadSession`), chunked from Rust
   with progress in the compose window. Un-defers the Phase 5 >3MB cap.
5. **Focused inbox.** Read `inferenceClassification` (already synced since the
   read pipeline); Focused/Other tabs in the message list. Overrides via
   `POST /me/inferenceClassification/overrides`.
6. **Categories (display + assign).** Message `categories` on PATCH. Category
   *management* (create/rename/color) is mailbox settings — a later phase.
7. **Un-stub keyboard verbs.** `e` archive, `#` delete, `s` flag now perform the
   real operations (through the optimistic path), replacing the Phase 6
   "needs mail management" stubs.

---

## Safety

- **Delete is destructive.** Delete = move to Deleted Items (recoverable);
  reserve hard `DELETE` for an explicit "delete permanently" with confirmation.
  Undo toast on the soft path.
- **Optimistic rollback must actually roll back** on failure — no orphaned local
  state. Test the failure path explicitly (poisoned proxy).
- Reuse Phase 4 sanitizer for any draft body that includes quoted HTML.
- Bulk operations (multi-select archive/delete) batch via `POST /$batch`
  (20 max) — but keep each item's optimistic/rollback state independent.

---

## Out of scope (later phases)

- Mailbox settings: rules, auto-replies, master category management
  (`MailboxSettings.ReadWrite`).
- Calendar, contacts (their own scopes/phases).
- Shared/delegated mailboxes (`Mail.*.Shared`).

---

## Acceptance

1. Adding `Mail.ReadWrite` triggers a clean re-consent on next sign-in; after
   consent, session continues without full re-setup.
2. Mark read/unread and flag: instant locally, persists on the server, survives
   a delta sync; the Phase 6 `flagged` chip now works.
3. Archive / move / delete: message moves to the right folder, undo toast
   restores it if used; permanent delete requires explicit confirm.
4. **Failure path:** with the network/proxy broken, an optimistic change **rolls
   back** and surfaces an error — no stuck local state.
5. True drafts: create, autosave, edit across sessions, send; reply now uses
   `createReply` and threads correctly.
6. Large attachment (>3MB) uploads via session with progress and sends.
7. Focused/Other tabs reflect `inferenceClassification`; override sticks.
8. `e`/`#`/`s` keyboard verbs perform real actions.
9. `cargo fmt --check`, `clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`, `npm --prefix ui run check && build` all green.

---

## Your review points when the plan lands

1. **Optimistic reconciliation** — confirm success/failure/rollback and
   delta-reconcile are all handled; the failure path especially. This is the core.
2. **Re-consent flow** — confirm the scope change triggers graceful re-consent,
   not a silent 403 or a forced re-setup.
3. **Delete safety** — soft-delete (move to Deleted) by default, hard delete
   gated behind explicit confirmation.
4. **Drafts replace manual reply** — `createReply`/`createReplyAll`/
   `createForward` now, and threading still verified.
5. **Scope is exactly `Mail.ReadWrite`** added — nothing broader
   (no `Mail.ReadWrite.Shared`, no `.All`).
