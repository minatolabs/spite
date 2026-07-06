<script lang="ts">
  import { mail } from './mail.svelte'

  let now = $state(Date.now())

  $effect(() => {
    const t = setInterval(() => (now = Date.now()), 5_000)
    return () => clearInterval(t)
  })

  let syncLabel = $derived.by(() => {
    if (mail.syncing) return 'syncing…'
    const ts = mail.syncState?.last_synced_at
    if (!ts) return mail.syncError ? 'sync failed' : 'not synced yet'
    const secs = Math.max(0, Math.floor(now / 1000 - ts))
    if (secs < 60) return `delta sync ${secs}s ago`
    if (secs < 3600) return `delta sync ${Math.floor(secs / 60)}m ago`
    return `delta sync ${Math.floor(secs / 3600)}h ago`
  })
</script>

<footer class="status">
  <span
    class="sp-led {mail.syncError ? 'sp-led--off' : 'sp-led--online'}"
    title={mail.syncError || 'connected'}
  ></span>
  <span class="label">{syncLabel}</span>
  {#if mail.syncError}
    <span class="err" title={mail.syncError}>offline — reading from local store</span>
  {/if}
  {#if mail.flash}
    <span class="flash">{mail.flash}</span>
  {/if}
</footer>

<style>
  .status {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: 0 var(--sp-3);
    background: var(--sp-surface-chrome);
    border-top: 1px solid var(--sp-border-hard);
    font-size: var(--sp-fs-caption);
    color: var(--sp-text-tertiary);
  }

  .err {
    color: var(--sp-text-accent);
  }

  .flash {
    margin-left: auto;
    color: var(--sp-flag);
  }
</style>
