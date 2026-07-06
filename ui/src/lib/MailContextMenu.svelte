<script lang="ts">
  import { onMount } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'
  import { Archive, Flag, FolderInput, Forward, MailOpen, Reply, Trash2 } from 'lucide-svelte'
  import { archive, mail, moveToFolder, softDelete, toggleFlag, toggleRead } from './mail.svelte'
  import FolderPicker from './FolderPicker.svelte'

  let showPicker = $state(false)
  let pickerFor: string | null = $state(null)

  let open = $state(false)
  let x = $state(0)
  let y = $state(0)
  let targetId: string | null = $state(null)

  function summaryFor(id: string) {
    return (
      mail.messages.find((m) => m.id === id) ??
      mail.hits.find((h) => h.entity_id === id)?.summary ??
      null
    )
  }

  function onContextMenu(e: MouseEvent) {
    const el = e.target as HTMLElement
    // Keep the native menu inside editable fields (copy/paste); suppress the
    // default browser menu (Back/Forward/Reload/Inspect) everywhere else.
    if (el.closest('input, textarea, [contenteditable="true"]')) return
    e.preventDefault()

    const row = el.closest<HTMLElement>('[data-message-id]')
    if (row?.dataset.messageId) {
      targetId = row.dataset.messageId
      mail.selectedId = targetId
      x = e.clientX
      y = e.clientY
      open = true
    } else {
      open = false
    }
  }

  function close() {
    open = false
  }

  function run(fn: () => void) {
    fn()
    close()
  }

  // Capture targetId at click time so TS sees a non-null string in the async
  // action (the reactive `targetId` is string | null).
  function act(fn: (id: string) => void) {
    const id = targetId
    if (id) run(() => fn(id))
  }

  onMount(() => {
    window.addEventListener('contextmenu', onContextMenu)
    window.addEventListener('click', close)
    window.addEventListener('blur', close)
    return () => {
      window.removeEventListener('contextmenu', onContextMenu)
      window.removeEventListener('click', close)
      window.removeEventListener('blur', close)
    }
  })

  let summary = $derived(targetId ? summaryFor(targetId) : null)
</script>

{#if open && targetId && summary}
  <div class="menu" style="left: {x}px; top: {y}px" role="menu" tabindex="-1">
    <button
      role="menuitem"
      onclick={() => act((id) => void invoke('open_compose', { mode: 'reply', messageId: id }))}
    >
      <Reply size={13} /> Reply
    </button>
    <button
      role="menuitem"
      onclick={() => act((id) => void invoke('open_compose', { mode: 'forward', messageId: id }))}
    >
      <Forward size={13} /> Forward
    </button>
    <div class="sep"></div>
    <button role="menuitem" onclick={() => summary && run(() => toggleRead(summary))}>
      <MailOpen size={13} /> Mark {summary.is_read ? 'unread' : 'read'}
    </button>
    <button role="menuitem" onclick={() => summary && run(() => toggleFlag(summary))}>
      <Flag size={13} /> {summary.flag_status === 'flagged' ? 'Unflag' : 'Flag'}
    </button>
    <div class="sep"></div>
    <button role="menuitem" onclick={() => act((id) => void archive(id))}>
      <Archive size={13} /> Archive
    </button>
    <button
      role="menuitem"
      onclick={() =>
        act((id) => {
          pickerFor = id
          showPicker = true
        })}
    >
      <FolderInput size={13} /> Move to…
    </button>
    <button role="menuitem" class="danger" onclick={() => act((id) => void softDelete(id))}>
      <Trash2 size={13} /> Delete
    </button>
  </div>
{/if}

{#if showPicker && pickerFor}
  <FolderPicker
    onpick={(dest) => {
      const id = pickerFor
      showPicker = false
      pickerFor = null
      if (id) void moveToFolder(id, dest)
    }}
    onclose={() => {
      showPicker = false
      pickerFor = null
    }}
  />
{/if}

<style>
  .menu {
    position: fixed;
    z-index: var(--sp-z-modal);
    min-width: 180px;
    padding: var(--sp-1);
    background: var(--ink-700);
    border: 1px solid var(--sp-border-hard);
    border-radius: var(--sp-r-control);
    box-shadow: var(--sp-lift);
  }

  .menu button {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    width: 100%;
    padding: 6px var(--sp-3);
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--sp-text-primary);
    font: inherit;
    font-size: var(--sp-fs-small);
    text-align: left;
    cursor: pointer;
  }

  .menu button:hover {
    background: var(--sp-selected-fill);
  }

  .menu button.danger:hover {
    color: var(--sp-danger-hover);
  }

  .sep {
    height: 1px;
    margin: var(--sp-1) 0;
    background: var(--sp-border-hard);
  }
</style>
