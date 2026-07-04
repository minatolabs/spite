# Spite — Phase 5: Compose and Send

Our numbering. Maps to `FEATURES.md` "Phase 3 — compose and send". The complete
outbound experience in one run: all these share the send path, so they belong
together. Hand to Fable after Phase 4. **Manual approve** — this sends mail as
the user, an outward action with real consequences. Start in plan mode.

---

## Goal

Make Spite send. Compose, send, reply, reply-all, forward, plus autocomplete,
small attachments, and signatures. First time the app acts outward.

**No new scope.** Everything runs on the existing `Mail.Send`. Do **not** add
`Mail.ReadWrite`. That exclusion is what forces the manual-threading approach
below, which is the interesting engineering of this phase.

---

## Functional scope

1. **Separate compose window (Tauri multiwindow).** A distinct window, not a
   modal, so the user can keep reading while composing. New window per compose/
   reply/forward. Fields: to, cc, bcc (collapsible), subject, body. Small,
   deliberate HTML editor (contenteditable: bold, italic, lists, links, quote)
   with a plain-text toggle. Send button, and the app's first destructive-ish
   confirmation surface (see safety below).

2. **Send.** `POST /me/sendMail` with `saveToSentItems: true`. Build the message
   payload (recipients, subject, body with `contentType` html or text). On
   success, close the window; on failure, keep the draft in the window and
   surface the error, never silently lose a composed message.

3. **Reply / reply-all / forward — the manual-threading part.** Without
   `Mail.ReadWrite` there is no `createReply`/`createForward` server draft. So
   construct the outgoing message by hand:
   - **Subject:** prefix `Re:` (reply/reply-all) or `Fw:` (forward), without
     double-prefixing an already-prefixed subject.
   - **Recipients:** reply → original sender; reply-all → sender + original
     to/cc minus yourself; forward → user-entered.
   - **Quoted body:** include the original body (sanitized, reuse Phase 4's
     `ammonia` path — quoted HTML is still hostile) with a standard attribution
     header ("On <date>, <sender> wrote:").
   - **Threading headers (do this right):** set `internetMessageHeaders` with
     `In-Reply-To` and `References` built from the original message's
     `internetMessageId`, so the thread survives on the recipient's client. This
     is the correctness core of the phase — get the References chain right.

4. **Recipient autocomplete (local, v1).** Rank addresses from the local store
   (frequency of sent-to and received-from). No People API yet (that's a later
   phase). Purely local, offline-capable.

5. **Small inline attachments.** `fileAttachment` as base64 inline on
   `sendMail`, for total payload under ~3 MB. Larger files need an upload
   session, which needs `Mail.ReadWrite` → explicitly deferred, show a clear
   "file too large for now" message rather than failing obscurely.

6. **Signatures (client-side).** Graph does not expose Outlook roaming
   signatures at all, so this is a Spite feature: per-account signatures stored
   in SQLite, with separate new-message and reply variants, inserted into the
   composer. Document in the README that this is a Graph limitation, not a Spite
   shortcut.

---

## Safety (this phase sends real mail)

- **Confirm before send** on the first outward action, or at minimum make the
  send button deliberate (no accidental Enter-to-send without a modifier).
- **Never lose a draft** on send failure — keep the composed content in the
  window and let the user retry.
- Reuse the Phase 4 sanitizer for any quoted/forwarded HTML. Quoted content is
  attacker-controlled; it does not get a pass because it's in a reply.
- Recipient sanity: reply-all must exclude the user's own address so you don't
  mail yourself, and must not silently drop recipients.

---

## Out of scope (later phases)

- True server-side drafts, autosave, `createReply`/`createForward` — all need
  `Mail.ReadWrite` (a later escalation phase).
- Large-file upload sessions (`Mail.ReadWrite`).
- People-API autocomplete (later phase; local ranking only here).
- Read/flag/move/delete/archive — all write ops, not this phase.

---

## Acceptance

1. Compose a new message in a separate window, send it, it arrives and appears
   in Sent.
2. Reply and reply-all: recipients correct (reply-all excludes self), subject
   prefixed once, original quoted, and **the sent message threads correctly** in
   the recipient's client (verify `In-Reply-To`/`References` are set from the
   original `internetMessageId`).
3. Forward: `Fw:` subject, body carried, user-entered recipients.
4. Autocomplete suggests addresses from local history, offline.
5. A small inline attachment sends; an oversized one shows the clear deferred
   message, not a crash.
6. A configured signature appears in the composer (new vs reply variant).
7. Send-failure keeps the draft in the window; no lost content.
8. `cargo fmt --check`, `clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`, `npm --prefix ui run check && build` all green.

---

## Your review points when the plan lands

1. **The threading headers** — confirm `In-Reply-To`/`References` are built from
   the original `internetMessageId`, with a correct `References` chain. This is
   the thing that silently breaks threads if wrong.
2. **Scope stays `Mail.Send`** — no `Mail.ReadWrite` sneaking in via "just a
   draft."
3. **Quoted/forwarded HTML goes through `ammonia`** — no sanitizer bypass on
   reply/forward.
4. **reply-all recipient logic** — excludes self, drops nobody else.
