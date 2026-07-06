<script lang="ts">
  import {
    Archive,
    FileText,
    Folder as FolderIcon,
    Inbox,
    Send,
    ShieldAlert,
    Trash2,
  } from 'lucide-svelte'
  import { mail, type Folder } from './mail.svelte'

  let {
    excludeId = null,
    onpick,
    onclose,
  }: {
    excludeId?: string | null
    onpick: (folderId: string) => void
    onclose: () => void
  } = $props()

  const order = ['inbox', 'sentitems', 'drafts', 'archive', 'junkemail', 'deleteditems']
  const icons: Record<string, typeof Inbox> = {
    inbox: Inbox,
    sentitems: Send,
    drafts: FileText,
    archive: Archive,
    junkemail: ShieldAlert,
    deleteditems: Trash2,
  }
  const labels: Record<string, string> = {
    inbox: 'Inbox',
    sentitems: 'Sent',
    drafts: 'Drafts',
    archive: 'Archive',
    junkemail: 'Junk',
    deleteditems: 'Deleted',
  }

  let pinned = $derived(
    order
      .map((w) => mail.folders.find((f) => f.well_known_name === w))
      .filter((f): f is Folder => !!f && f.id !== excludeId),
  )
  let userFolders = $derived(
    mail.folders.filter((f) => !f.well_known_name && f.id !== excludeId),
  )
</script>

<div
  class="backdrop"
  onclick={onclose}
  onkeydown={(e) => e.key === 'Escape' && onclose()}
  role="presentation"
>
  <div class="picker" role="menu" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
    <p class="head">Move to…</p>
    {#each pinned as folder (folder.id)}
      {@const Icon = icons[folder.well_known_name ?? ''] ?? FolderIcon}
      <button role="menuitem" onclick={() => onpick(folder.id)}>
        <Icon size={13} />
        {labels[folder.well_known_name ?? ''] ?? folder.display_name}
      </button>
    {/each}
    {#if userFolders.length}
      <div class="sep"></div>
      {#each userFolders as folder (folder.id)}
        <button role="menuitem" onclick={() => onpick(folder.id)}>
          <FolderIcon size={13} />
          {folder.display_name}
        </button>
      {/each}
    {/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: var(--sp-z-modal);
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.4);
  }

  .picker {
    min-width: 220px;
    max-height: 70vh;
    overflow-y: auto;
    padding: var(--sp-1);
    background: var(--ink-700);
    border: 1px solid var(--sp-border-hard);
    border-radius: var(--sp-r-panel);
    box-shadow: var(--sp-lift);
  }

  .head {
    margin: 0;
    padding: var(--sp-2) var(--sp-3);
    font-size: var(--sp-fs-caption);
    text-transform: uppercase;
    letter-spacing: var(--sp-track-label);
    color: var(--sp-text-muted);
  }

  button {
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

  button:hover {
    background: var(--sp-selected-fill);
  }

  .sep {
    height: 1px;
    margin: var(--sp-1) var(--sp-2);
    background: var(--sp-border-hard);
  }
</style>
