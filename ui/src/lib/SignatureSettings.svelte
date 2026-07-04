<script lang="ts">
  import { onMount } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'

  let { onclose }: { onclose: () => void } = $props()

  let sigNew = $state('')
  let sigReply = $state('')
  let saving = $state(false)
  let error = $state('')

  onMount(() => {
    void (async () => {
      try {
        sigNew = (await invoke<string | null>('get_signature', { kind: 'new' })) ?? ''
        sigReply = (await invoke<string | null>('get_signature', { kind: 'reply' })) ?? ''
      } catch (e) {
        error = String(e)
      }
    })()
  })

  async function save() {
    saving = true
    error = ''
    try {
      await invoke('set_signature', { kind: 'new', content: sigNew })
      await invoke('set_signature', { kind: 'reply', content: sigReply })
      onclose()
    } catch (e) {
      error = String(e)
    } finally {
      saving = false
    }
  }
</script>

<div
  class="backdrop"
  onclick={onclose}
  onkeydown={(e) => e.key === 'Escape' && onclose()}
  role="presentation"
>
  <div
    class="modal"
    onclick={(e) => e.stopPropagation()}
    role="dialog"
    aria-label="Signatures"
    tabindex="-1"
    onkeydown={() => {}}
  >
    <h2>Signatures</h2>
    <p class="note">
      Stored locally per account — Microsoft Graph doesn't expose Outlook's
      roaming signatures to any third-party client.
    </p>
    <label>
      New messages
      <textarea bind:value={sigNew} rows="4" placeholder="Plain text signature"></textarea>
    </label>
    <label>
      Replies &amp; forwards
      <textarea bind:value={sigReply} rows="4" placeholder="Plain text signature"></textarea>
    </label>
    {#if error}<p class="error">{error}</p>{/if}
    <div class="actions">
      <button class="sp-btn sp-btn--primary" onclick={save} disabled={saving}>
        {saving ? 'Saving…' : 'Save'}
      </button>
      <button class="sp-btn" onclick={onclose}>Cancel</button>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: var(--sp-z-modal);
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .modal {
    width: min(480px, 90vw);
    padding: var(--sp-5);
    background: var(--sp-surface-raised);
    border: 1px solid var(--sp-border-hard);
    border-radius: var(--sp-r-panel);
    box-shadow: var(--sp-lift);
  }

  h2 {
    margin: 0 0 var(--sp-2);
    font-size: var(--sp-fs-title);
    color: var(--sp-text-display);
  }

  .note {
    margin: 0 0 var(--sp-4);
    font-size: var(--sp-fs-small);
    color: var(--sp-text-secondary);
  }

  label {
    display: block;
    margin-bottom: var(--sp-3);
    font-size: var(--sp-fs-caption);
    text-transform: uppercase;
    letter-spacing: var(--sp-track-label);
    color: var(--sp-text-muted);
  }

  textarea {
    display: block;
    width: 100%;
    margin-top: var(--sp-1);
    padding: var(--sp-2);
    font: 400 var(--sp-fs-small) / var(--sp-lh-ui) var(--sp-font-ui);
    color: var(--sp-text-primary);
    background: var(--sp-surface-well);
    border: 1px solid var(--sp-border-hard);
    border-radius: var(--sp-r-control);
    box-shadow: var(--sp-well);
    resize: vertical;
  }

  .error {
    color: var(--sp-danger);
    font-size: var(--sp-fs-small);
  }

  .actions {
    display: flex;
    gap: var(--sp-2);
    margin-top: var(--sp-4);
  }
</style>
