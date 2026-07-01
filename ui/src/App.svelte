<script lang="ts">
  import { onMount } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'
  import { listen, type UnlistenFn } from '@tauri-apps/api/event'

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

  onMount(() => {
    let unlisten: UnlistenFn | undefined
    listen<DeviceCodePrompt>('auth:device-code', (event) => {
      prompt = event.payload
    }).then((fn) => (unlisten = fn))

    invoke<Account>('silent_sign_in')
      .then((a) => (account = a))
      .catch(() => {}) // not signed in yet — show the sign-in button
      .finally(() => (checking = false))

    return () => unlisten?.()
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

<main>
  <h1>Spite</h1>
  {#if checking}
    <p class="muted">Checking sign-in…</p>
  {:else if account}
    <p>
      Signed in as <strong>{account.display_name}</strong>
      <span class="muted">({account.upn})</span>
    </p>
    <button onclick={signOut}>Sign out</button>
  {:else}
    <button onclick={signIn} disabled={busy}>
      {busy ? 'Waiting for sign-in…' : 'Sign in with Microsoft'}
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

<style>
  main {
    min-height: 100svh;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 1rem;
    padding: 1rem;
    text-align: center;
  }

  h1 {
    margin: 0;
    font-size: 2rem;
    font-weight: 600;
    letter-spacing: -0.02em;
  }

  p {
    margin: 0;
  }

  .muted {
    opacity: 0.7;
  }

  button {
    font: inherit;
    padding: 0.5rem 1rem;
    border: 1px solid currentColor;
    border-radius: 6px;
    background: transparent;
    color: inherit;
    cursor: pointer;
  }

  button:disabled {
    opacity: 0.6;
    cursor: default;
  }

  .prompt {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    max-width: 28rem;
  }

  .prompt code {
    font-size: 1.5rem;
    letter-spacing: 0.15em;
    user-select: all;
  }

  .error {
    color: #e5484d;
    max-width: 28rem;
    overflow-wrap: anywhere;
  }
</style>
