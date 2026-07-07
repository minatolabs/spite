<script lang="ts">
  import { onMount } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'
  import { listen, type UnlistenFn } from '@tauri-apps/api/event'
  import Composer from './lib/Composer.svelte'
  import Shell from './lib/Shell.svelte'

  const urlParams = new URLSearchParams(window.location.search)
  const composeLabel = urlParams.get('compose') ? urlParams.get('label') : null

  type Account = { upn: string; display_name: string }
  type DeviceCodePrompt = {
    user_code: string
    verification_uri: string
    expires_in_secs: number
  }

  let checking = $state(true)
  let account: Account | null = $state(null)
  let prompt: DeviceCodePrompt | null = $state(null)
  let busy = $state(false)
  let error = $state('')
  let consentRequired = $state(false)

  onMount(() => {
    let unlisten: UnlistenFn | undefined
    listen<DeviceCodePrompt>('auth:device-code', (event) => {
      prompt = event.payload
    }).then((fn) => (unlisten = fn))

    // Baseline: suppress the browser's default right-click menu app-wide
    // (no Back/Forward/Reload/Inspect), keeping it only in editable fields.
    // MailContextMenu adds the custom mail menu on message rows on top.
    const suppressMenu = (e: MouseEvent) => {
      const el = e.target as HTMLElement
      if (!el.closest('input, textarea, [contenteditable="true"]')) e.preventDefault()
    }
    window.addEventListener('contextmenu', suppressMenu)

    invoke<Account>('silent_sign_in')
      .then((a) => (account = a))
      .catch((e) => {
        // A scope escalation (Phase 7's Mail.ReadWrite, Phase 8A's
        // MailboxSettings.ReadWrite) makes the cached token insufficient; the
        // shell reports this distinctly so we can ask for re-consent rather
        // than showing a generic sign-in.
        if (String(e).toLowerCase().includes('permission to manage your mail')) {
          consentRequired = true
        }
      })
      .finally(() => (checking = false))

    return () => {
      unlisten?.()
      window.removeEventListener('contextmenu', suppressMenu)
    }
  })

  async function signIn() {
    busy = true
    error = ''
    prompt = null
    try {
      account = await invoke<Account>('sign_in')
    } catch (e) {
      error = String(e)
    } finally {
      busy = false
      prompt = null
    }
  }

  async function signOut() {
    try {
      await invoke('sign_out')
      account = null
    } catch (e) {
      error = String(e)
    }
  }
</script>

{#if composeLabel}
  <Composer label={composeLabel} />
{:else if account}
  <Shell {account} onsignout={signOut} />
{:else}
  <main class="gate">
    <h1>SPITE</h1>
    {#if checking}
      <p class="muted">Checking sign-in…</p>
    {:else}
      {#if consentRequired}
        <p class="consent">
          Spite needs permission to manage your mail (read, flag, move, delete,
          and drafts) and your mailbox settings (out-of-office and categories).
          Sign in again to grant it — your account stays connected.
        </p>
      {/if}
      <button class="sp-btn sp-btn--primary" onclick={signIn} disabled={busy}>
        {busy
          ? 'Waiting for sign-in…'
          : consentRequired
            ? 'Grant permission'
            : 'Sign in with Microsoft'}
      </button>
      {#if prompt}
        <div class="prompt">
          <p>
            On any device, open
            <strong>{prompt.verification_uri}</strong>
            and enter the code:
          </p>
          <code>{prompt.user_code}</code>
        </div>
      {/if}
      {#if error}
        <p class="error">{error}</p>
      {/if}
    {/if}
  </main>
{/if}

<style>
  .gate {
    height: 100svh;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--sp-4);
    padding: var(--sp-4);
    text-align: center;
    background: var(--sp-surface-base);
  }

  h1 {
    margin: 0;
    font-size: var(--sp-fs-title);
    font-weight: 600;
    letter-spacing: var(--sp-track-wordmark);
    color: var(--sp-text-display);
  }

  .consent {
    max-width: 26rem;
    color: var(--sp-text-secondary);
    font-size: var(--sp-fs-small);
    line-height: var(--sp-lh-ui);
  }

  .muted {
    color: var(--sp-text-secondary);
  }

  .prompt {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    max-width: 28rem;
    color: var(--sp-text-primary);
  }

  .prompt code {
    font-family: var(--sp-font-mono);
    font-size: var(--sp-fs-title);
    letter-spacing: 0.15em;
    user-select: all;
    color: var(--sp-text-display);
  }

  .error {
    color: var(--sp-danger);
    max-width: 28rem;
    overflow-wrap: anywhere;
  }
</style>
