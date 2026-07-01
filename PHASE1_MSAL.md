# Spite — Phase 1: Auth (Microsoft identity / Graph, device-code)

Hand this to Fable *after* Phase 0. Start in plan mode, approve before code.

---

## Goal

Sign in to one M365 mailbox, store tokens in the OS keychain, prove a Graph
`/me` call works, and refresh silently on relaunch. **No mail, no sync, no
send yet** — just auth.

---

## Scope reference (what each grants)

| Scope | Grants |
|---|---|
| `Mail.Read` | read messages/folders (read-only) |
| `Mail.ReadWrite` | modify: drafts, mark read, move, delete (NOT send) |
| `Mail.Send` | send mail as the user (`/me/sendMail`) |
| `User.Read` | sign-in + read own profile (`/me`) |
| `offline_access` | returns a **refresh token** (not a mail permission) |

**Least-privilege decision (pick one):**
- **Strict v0.1:** `Mail.Read Mail.Send User.Read offline_access` — drop
  `Mail.ReadWrite` (no server-side mark-read/drafts/delete).
- **Comfortable v0.1:** add `Mail.ReadWrite` if you want mark-as-read on open.

Default to strict unless you decide you need mark-as-read now.

---

## Prerequisites — Entra app registration (do this BEFORE coding)

Register **one** app that Spite ships with. End users never register anything
and never touch Entra — they just sign in with their normal M365 address. This
is exactly how Thunderbird and Apple Mail work.

1. Entra ID → App registrations → New registration. Name: `Spite`.
2. Supported accounts: **"Accounts in any organizational directory
   (multitenant)"** (optionally + personal Microsoft accounts). This is the
   decision — it's what lets any M365 user sign in, not just your tenant.
3. Authentication → **Allow public client flows = Yes** (required for device
   code). **No client secret** — Spite is a public, distributed FOSS client;
   a secret can't be kept secret, so it uses PKCE/device-code, not a secret.
4. API permissions → Microsoft Graph → **Delegated** → add the scopes chosen
   above. (`offline_access`, `openid`, `profile` are consented implicitly.)
   For a multitenant app you do **not** pre-consent for others; each user (or
   their admin) consents at first sign-in.
5. Record the **client_id**. Ship it in the app — for a public client the
   client_id is **not** a secret, so committing it to the FOSS repo is fine
   (Thunderbird does exactly this).

**Authority, not tenant_id.** Because it's multitenant, the app authenticates
against `https://login.microsoftonline.com/common` (or `/organizations` to
exclude personal accounts), **not** your tenant ID. `/common` auto-resolves
each user's tenant from their email. So there is no tenant_id to hard-code.

**Config-driven from day one (do this now, avoids a rewrite later):** put
`client_id` and `authority` in app config with defaults =
Spite's shipped client_id + `.../common`. This gives you two things for free:
- **Local testing:** temporarily point `authority` at your own tenant ID if you
  want to test against just your mailbox first — a config change, not a code
  change.
- **Bring-your-own-client-ID (privacy touch):** advanced users can override
  `client_id`/`authority` with their own Entra app, so the security-conscious
  crowd never routes through your registration. Thunderbird supports this; it's
  a natural minatolabs option and it's free if the values are config from the
  start.

---

## Auth approach (important)

**There is no official Microsoft MSAL library for Rust.** "MSAL" here means
implementing the Microsoft identity platform **OAuth 2.0 device authorization
grant** in the Rust core. Two viable paths:

- `azure_identity` crate's `DeviceCodeCredential`, or
- hand-roll with the `oauth2` crate against the **`/common`** authority's
  `/devicecode` and `/token` endpoints (not a tenant-specific URL).

Use **Context7** to pull the current API for whichever crate you pick — do not
trust training-data signatures for these, they move.

**Run the entire flow in the Rust `core` crate, never in the webview**, so
tokens never touch browser storage (privacy invariant).

Flow: request device code → surface `user_code` + `verification_uri` to the UI
→ poll `/token` → receive access + refresh tokens.

*Alternative to note, not implement:* auth-code + PKCE + loopback redirect gives
a smoother one-click GUI sign-in. Device-code chosen for robustness and because
it matches the tenant's known no-browser-handoff behavior. Revisit if the UX
feels clunky.

**The one adoption friction (not a code problem):** some orgs disable user
consent for unverified third-party apps. Those users will see *"Need admin
approval"* instead of a normal consent screen — this is tenant policy, not a
Spite bug. Going through Microsoft **publisher verification** (the blue verified
badge) makes admins far more likely to allow it. Note it for the roadmap;
nothing to do in Phase 1.

---

## Token storage

- Store via the **`keyring`** crate → OS keychain (Secret Service/libsecret on
  Linux, Keychain on macOS, Credential Manager on Windows).
- Key it per mailbox: service `com.minatolabs.spite`, account = the UPN.
- Persist the **refresh token** (access token optional/short-lived). **Never
  plaintext on disk.**
- **Headless caveat:** Secret Service needs an unlocked keyring daemon. On a
  headless box (this OptiPlex's future Hermes role) that won't exist, so a
  later fallback (encrypted file with an OS-derived key, or `tauri-plugin-
  stronghold`) will be needed. Fine to ignore on the GUI dev machine now — just
  leave the storage layer behind a trait so the backend is swappable.

---

## Refresh handling

- On access-token expiry, exchange the refresh token silently for a new access
  token. If the IdP returns a rotated refresh token, replace the stored one.
- On refresh failure (revoked/expired), fall back to re-running device-code
  sign-in.

---

## Acceptance test

1. `sign_in()` completes the device-code flow.
2. `GET https://graph.microsoft.com/v1.0/me` returns 200; app prints your
   `displayName`.
3. **Relaunch the app** → it reads the refresh token from the keychain and gets
   a valid token **without prompting** (silent refresh proven).

---

## Out of scope for Phase 1

No mail listing, no SQLite, no message read, no compose/send, no delta sync.
Only: sign in, store tokens, prove `/me`, silent refresh on relaunch. The
`store`/`sync` module stubs stay empty until Phases 2–3.

---

## Note

This is OAuth/token code. If a prompt gets rerouted to Opus 4.8 with a notice,
that's today's Fable classifier, not a block — continue.
