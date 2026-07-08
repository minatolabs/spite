# Spite — Phase 8B (Run B): Message Rules (full engine)

Our numbering. Completes `FEATURES.md` "Phase 6 — mailbox settings" (the rules
half). Hand to Fable after Run A. **Manual approve** — rules silently move,
delete, and forward mail server-side. Start in plan mode.

---

## Goal

A full message-rules editor over Graph's `messageRules`: the complete condition
and action surface, exceptions, ordering (sequence), and enable toggles. These
run **server-side**, so they fire even when Spite is closed — that's the point.

**No new scope.** Reuses `MailboxSettings.ReadWrite` from Run A. No portal step,
no re-consent this run.

---

## The full surface (all of messageRules)

Rules live at `GET/POST/PATCH/DELETE /me/mailFolders/inbox/messageRules`. Each
rule = `displayName`, `sequence` (order), `isEnabled`, `conditions`,
`exceptions` (same shape as conditions), `actions`.

**Conditions + exceptions (implement all):** `fromAddresses`,
`sentToAddresses`, `sentToMe`, `sentOnlyToMe`, `sentCcMe`, `notSentToMe`,
`recipientContains`, `senderContains`, `subjectContains`,
`bodyContains`, `bodyOrSubjectContains`, `headerContains`,
`importance`, `sensitivity`, `messageActionFlag`, `isApprovalRequest`,
`isAutomaticForward`, `isAutomaticReply`, `isEncrypted`, `isMeetingRequest`,
`isMeetingResponse`, `isNonDeliveryReport`, `isPermissionControlled`,
`isReadReceipt`, `isSigned`, `isVoicemail`, `withinSizeRange`,
`hasAttachments`, `categories`.

**Actions (implement all):** `moveToFolder`, `copyToFolder`, `delete`,
`permanentDelete`, `forwardTo`, `forwardAsAttachmentTo`, `redirectTo`,
`assignCategories`, `markAsRead`, `markImportance`, `stopProcessingRules`.

Group them in the UI so it's navigable, don't dump 30 checkboxes flat. Address
conditions take recipient pickers (reuse the compose recipient input);
folder actions use the FolderPicker from Phase 7; category actions use the
managed master categories from Run A.

---

## Round-trip safety (the correctness core)

Spite is editing the SAME rules Outlook shows. So:

- **Load, edit, and save must preserve every field** — including any the UI
  doesn't surface. Never drop a condition/action on save because the editor
  didn't render it. Round-trip the full rule object: edit the parts the UI
  exposes, write back everything else untouched.
- Since this run implements the *full* surface, "unknown field" should be rare,
  but Graph evolves — if a rule carries something unrecognized, preserve it
  verbatim rather than stripping it.
- **Sequence integrity:** reordering rewrites `sequence` values; ensure no
  duplicate/gap corruption, and that reorder is atomic (don't leave two rules
  claiming the same slot).

---

## UI

- Rules list: name, enabled toggle, a human-readable one-line summary ("If from
  X and has attachment → move to Y"), drag-to-reorder (reuse Phase 7 DnD).
- Rule editor: name; conditions builder; exceptions builder (same component);
  actions builder; enable toggle. Multiple conditions are AND'd (Graph
  semantics — state this in the UI so users aren't surprised).
- Create / edit / duplicate / delete. Delete confirms (a rule can forward/delete
  mail; removing it is consequential but recoverable by recreating).

---

## Safety / consistency

- Optimistic + rollback (reuse Phase 7): apply locally, PATCH/POST in the
  background, roll back on failure with the action-error banner.
- **Forward/redirect actions send mail to third parties** — surface these
  clearly in the summary ("→ forwards to external@x.com") so a user can't
  create a silent auto-forward without seeing it. This is a data-exfiltration-
  shaped action; make it visible, not buried.
- `delete` = to Deleted Items; `permanentDelete` = gone. Label the difference
  unmistakably in the action picker (you don't want someone picking permanent
  delete thinking it's soft).
- Rules cache to the settings KV (from Run A) so the list renders offline;
  edits require network (writes go to Graph).

---

## Out of scope

- Rules on folders other than Inbox (Graph scopes messageRules to a folder;
  Inbox is the standard case — note multi-folder as a later extension).
- No new scope; calendar/contacts unaffected.

---

## Acceptance

1. List existing rules (including any created in Outlook) with correct
   human-readable summaries; a complex Outlook-made rule displays without
   corruption.
2. Create a rule (e.g. from-address + has-attachment → move to folder +
   mark-read), save → verify it appears and works on outlook.com, and fires
   server-side (send a matching test mail with Spite closed → it gets moved).
3. Edit that rule (add an exception), save → **all original fields preserved**,
   only the intended change applied (verify the raw rule on outlook.com).
4. Reorder rules → `sequence` updates cleanly, no duplicate slots.
5. Enable/disable toggle round-trips.
6. Forward/redirect actions show a clear external-recipient warning in the
   summary.
7. Failure path: a rejected rule write rolls back with the banner.
8. `cargo fmt --check`, `clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`, `npm --prefix ui run check && build` all green.

---

## Your review points when the plan lands

1. **Round-trip preservation** — editing a rule writes back untouched every
   field the UI didn't surface. This is the core; a lossy save corrupts rules
   the user built in Outlook.
2. **Sequence/reorder integrity** — no duplicate or gapped `sequence` values.
3. **Forward/redirect visibility** — external-forward actions are surfaced, not
   silent (anti-exfiltration hygiene).
4. **permanentDelete vs delete** — clearly distinguished in the UI.
5. **Scope unchanged** — still `MailboxSettings.ReadWrite`, nothing new.
