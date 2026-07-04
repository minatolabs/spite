<script lang="ts">
  import { onMount } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'
  import { listen, type UnlistenFn } from '@tauri-apps/api/event'
  import { CircleAlert, CircleCheck, Undo2 } from 'lucide-svelte'

  type QueuedEvent = { id: number; subject: string; recipients: number; deadline_ms: number }
  type ResultEvent = { id: number; error: string | null }
  type Toast = {
    id: number
    subject: string
    recipients: number
    deadlineMs: number
    state: 'pending' | 'sent' | 'failed'
    error?: string
  }

  let toasts: Toast[] = $state([])
  let now = $state(Date.now())

  onMount(() => {
    const ticker = setInterval(() => (now = Date.now()), 250)
    const unlisteners: UnlistenFn[] = []
    void listen<QueuedEvent>('send:queued', ({ payload }) => {
      toasts = [
        ...toasts,
        {
          id: payload.id,
          subject: payload.subject,
          recipients: payload.recipients,
          deadlineMs: payload.deadline_ms,
          state: 'pending',
        },
      ]
    }).then((fn) => unlisteners.push(fn))
    void listen<ResultEvent>('send:sent', ({ payload }) => {
      toasts = toasts.map((t) => (t.id === payload.id ? { ...t, state: 'sent' as const } : t))
      setTimeout(() => dismiss(payload.id), 2500)
    }).then((fn) => unlisteners.push(fn))
    void listen<ResultEvent>('send:failed', ({ payload }) => {
      toasts = toasts.map((t) =>
        t.id === payload.id
          ? { ...t, state: 'failed' as const, error: payload.error ?? 'unknown error' }
          : t,
      )
    }).then((fn) => unlisteners.push(fn))
    return () => {
      clearInterval(ticker)
      unlisteners.forEach((fn) => fn())
    }
  })

  function dismiss(id: number) {
    toasts = toasts.filter((t) => t.id !== id)
  }

  async function undo(id: number) {
    try {
      await invoke('undo_send', { id })
      dismiss(id)
    } catch {
      // Raced the timer: the send already fired. The sent/failed event
      // updates this toast momentarily.
    }
  }

  function remaining(t: Toast): number {
    return Math.max(0, Math.ceil((t.deadlineMs - now) / 1000))
  }

  function title(t: Toast): string {
    return t.subject || '(no subject)'
  }
</script>

{#if toasts.length}
  <div class="stack">
    {#each toasts as t (t.id)}
      <div class="toast" class:failed={t.state === 'failed'}>
        {#if t.state === 'pending'}
          <span class="sp-led"></span>
          <span class="text">
            Sending “{title(t)}” to {t.recipients} recipient{t.recipients === 1 ? '' : 's'}
            {#if remaining(t) > 0}&nbsp;in {remaining(t)}s{/if}…
          </span>
          <button class="sp-btn" onclick={() => void undo(t.id)}>
            <Undo2 size={13} /> Cancel
          </button>
        {:else if t.state === 'sent'}
          <CircleCheck size={14} class="ok" />
          <span class="text">Sent “{title(t)}”.</span>
        {:else}
          <CircleAlert size={14} />
          <span class="text" title={t.error}>
            Send failed — the draft reopened in a compose window.
          </span>
          <button class="sp-btn" onclick={() => dismiss(t.id)}>Dismiss</button>
        {/if}
      </div>
    {/each}
  </div>
{/if}

<style>
  .stack {
    position: fixed;
    bottom: calc(var(--sp-h-statusbar) + var(--sp-3));
    right: var(--sp-4);
    z-index: var(--sp-z-toast);
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: var(--sp-2);
  }

  .toast {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    max-width: 560px;
    padding: var(--sp-2) var(--sp-4);
    font-size: var(--sp-fs-small);
    color: var(--sp-text-primary);
    background: var(--ink-700);
    border: 1px solid var(--sp-border-hard);
    border-radius: var(--sp-r-panel);
    box-shadow: var(--sp-lift);
  }

  .toast.failed {
    border-color: var(--sp-danger);
  }

  .text {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
