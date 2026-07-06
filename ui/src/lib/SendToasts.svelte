<script lang="ts">
  import { onMount } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'
  import { listen, type UnlistenFn } from '@tauri-apps/api/event'
  import { CircleAlert, CircleCheck, Undo2 } from 'lucide-svelte'
  import { refreshList } from './mail.svelte'

  type SendQueuedEvent = { id: number; subject: string; recipients: number; deadline_ms: number }
  type OpQueuedEvent = { id: number; label: string; subject: string; deadline_ms: number }
  type ResultEvent = { id: number; error: string | null }
  type Toast = {
    // Op and send ids share a numeric space, so key on kind too.
    kind: 'send' | 'op'
    id: number
    label: string // 'send' | 'archive' | 'delete' | 'move'
    subject: string
    recipients: number
    deadlineMs: number
    state: 'pending' | 'done' | 'failed'
    error?: string
  }

  let toasts: Toast[] = $state([])
  let now = $state(Date.now())

  const key = (t: Pick<Toast, 'kind' | 'id'>) => `${t.kind}:${t.id}`

  onMount(() => {
    const ticker = setInterval(() => (now = Date.now()), 250)
    const unlisteners: UnlistenFn[] = []

    void listen<SendQueuedEvent>('send:queued', ({ payload }) => {
      toasts = [
        ...toasts,
        {
          kind: 'send',
          id: payload.id,
          label: 'send',
          subject: payload.subject,
          recipients: payload.recipients,
          deadlineMs: payload.deadline_ms,
          state: 'pending',
        },
      ]
    }).then((fn) => unlisteners.push(fn))
    void listen<ResultEvent>('send:sent', ({ payload }) => {
      markDone('send', payload.id, null)
    }).then((fn) => unlisteners.push(fn))
    void listen<ResultEvent>('send:failed', ({ payload }) => {
      markFailed('send', payload.id, payload.error)
    }).then((fn) => unlisteners.push(fn))

    void listen<OpQueuedEvent>('op:queued', ({ payload }) => {
      toasts = [
        ...toasts,
        {
          kind: 'op',
          id: payload.id,
          label: payload.label,
          subject: payload.subject,
          recipients: 0,
          deadlineMs: payload.deadline_ms,
          state: 'pending',
        },
      ]
    }).then((fn) => unlisteners.push(fn))
    void listen<ResultEvent>('op:done', ({ payload }) => {
      if (payload.error) {
        markFailed('op', payload.id, payload.error)
        void refreshList() // rolled back on the server side — repaint
      } else {
        markDone('op', payload.id, null)
      }
    }).then((fn) => unlisteners.push(fn))

    return () => {
      clearInterval(ticker)
      unlisteners.forEach((fn) => fn())
    }
  })

  function markDone(kind: 'send' | 'op', id: number, _e: string | null) {
    toasts = toasts.map((t) =>
      t.kind === kind && t.id === id ? { ...t, state: 'done' as const } : t,
    )
    setTimeout(() => dismiss(kind, id), 2500)
  }
  function markFailed(kind: 'send' | 'op', id: number, e: string | null) {
    toasts = toasts.map((t) =>
      t.kind === kind && t.id === id
        ? { ...t, state: 'failed' as const, error: e ?? 'unknown error' }
        : t,
    )
  }

  function dismiss(kind: 'send' | 'op', id: number) {
    toasts = toasts.filter((t) => !(t.kind === kind && t.id === id))
  }

  async function undo(t: Toast) {
    try {
      await invoke(t.kind === 'send' ? 'undo_send' : 'undo_op', { id: t.id })
      if (t.kind === 'op') await refreshList()
      dismiss(t.kind, t.id)
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

  function pendingText(t: Toast): string {
    const secs = remaining(t) > 0 ? ` in ${remaining(t)}s` : ''
    if (t.kind === 'send') {
      return `Sending “${title(t)}” to ${t.recipients} recipient${t.recipients === 1 ? '' : 's'}${secs}…`
    }
    const verb = { archive: 'Archiving', delete: 'Deleting', move: 'Moving' }[t.label] ?? 'Working'
    return `${verb} “${title(t)}”${secs}…`
  }

  function doneText(t: Toast): string {
    if (t.kind === 'send') return `Sent “${title(t)}”.`
    const verb = { archive: 'Archived', delete: 'Deleted', move: 'Moved' }[t.label] ?? 'Done'
    return `${verb} “${title(t)}”.`
  }

  function failText(t: Toast): string {
    return t.kind === 'send'
      ? 'Send failed — the draft reopened in a compose window.'
      : `${t.label} failed — the change was rolled back.`
  }
</script>

{#if toasts.length}
  <div class="stack">
    {#each toasts as t (key(t))}
      <div class="toast" class:failed={t.state === 'failed'}>
        {#if t.state === 'pending'}
          <span class="sp-led"></span>
          <span class="text">{pendingText(t)}</span>
          <button class="sp-btn" onclick={() => void undo(t)}>
            <Undo2 size={13} /> {t.kind === 'send' ? 'Cancel' : 'Undo'}
          </button>
        {:else if t.state === 'done'}
          <CircleCheck size={14} class="ok" />
          <span class="text">{doneText(t)}</span>
        {:else}
          <CircleAlert size={14} />
          <span class="text" title={t.error}>{failText(t)}</span>
          <button class="sp-btn" onclick={() => dismiss(t.kind, t.id)}>Dismiss</button>
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
