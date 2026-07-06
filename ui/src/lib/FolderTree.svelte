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
  import { mail, moveToFolder, selectFolder, type Folder } from './mail.svelte'

  // Drop-target highlight (drag messages from the list onto a folder).
  let dropTargetId: string | null = $state(null)

  function onDragOver(folderId: string, e: DragEvent) {
    if (!e.dataTransfer?.types.includes('application/x-spite-messages')) return
    if (folderId === mail.folderId) return // no-op onto the source folder
    e.preventDefault()
    e.dataTransfer.dropEffect = 'move'
    dropTargetId = folderId
  }

  function onDrop(folderId: string, e: DragEvent) {
    dropTargetId = null
    const raw = e.dataTransfer?.getData('application/x-spite-messages')
    if (!raw || folderId === mail.folderId) return
    e.preventDefault()
    let ids: string[]
    try {
      ids = JSON.parse(raw)
    } catch {
      return
    }
    for (const id of ids) void moveToFolder(id, folderId)
  }

  const wellKnownOrder = ['inbox', 'sentitems', 'drafts', 'archive', 'junkemail', 'deleteditems']
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
    wellKnownOrder
      .map((w) => mail.folders.find((f) => f.well_known_name === w))
      .filter((f): f is Folder => !!f),
  )
  let userFolders = $derived(mail.folders.filter((f) => !f.well_known_name))
</script>

<nav>
  <p class="section-label">Folders</p>
  {#each pinned as folder (folder.id)}
    {@const Icon = icons[folder.well_known_name ?? ''] ?? FolderIcon}
    <button
      class="folder"
      class:selected={folder.id === mail.folderId}
      class:drop-target={dropTargetId === folder.id}
      onclick={() => void selectFolder(folder.id)}
      ondragover={(e) => onDragOver(folder.id, e)}
      ondragleave={() => (dropTargetId = null)}
      ondrop={(e) => onDrop(folder.id, e)}
    >
      <Icon size={14} />
      <span class="name">{labels[folder.well_known_name ?? ''] ?? folder.display_name}</span>
      {#if mail.unread[folder.id]}
        <span class="sp-count">{mail.unread[folder.id]}</span>
      {/if}
    </button>
  {/each}

  {#if userFolders.length}
    <div class="sp-stitch-h stitch"></div>
    {#each userFolders as folder (folder.id)}
      <button
        class="folder"
        class:selected={folder.id === mail.folderId}
        class:drop-target={dropTargetId === folder.id}
        onclick={() => void selectFolder(folder.id)}
        ondragover={(e) => onDragOver(folder.id, e)}
        ondragleave={() => (dropTargetId = null)}
        ondrop={(e) => onDrop(folder.id, e)}
      >
        <FolderIcon size={14} />
        <span class="name">{folder.display_name}</span>
        {#if mail.unread[folder.id]}
          <span class="sp-count">{mail.unread[folder.id]}</span>
        {/if}
      </button>
    {/each}
  {/if}
</nav>

<style>
  .section-label {
    margin: 0 0 var(--sp-2);
    padding: 0 var(--sp-3);
    font-size: var(--sp-fs-caption);
    font-weight: 600;
    letter-spacing: var(--sp-track-label);
    text-transform: uppercase;
    color: var(--sp-text-muted);
  }

  .folder {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    width: 100%;
    padding: 6px var(--sp-3);
    border: none;
    border-left: 3px solid transparent;
    background: transparent;
    color: var(--sp-text-secondary);
    font: inherit;
    font-size: var(--sp-fs-small);
    text-align: left;
    cursor: pointer;
  }

  .folder:hover {
    background: var(--ink-800);
    color: var(--sp-text-primary);
  }

  .folder.selected {
    background: var(--sp-selected-fill);
    border-left-color: var(--sp-accent-edge);
    color: var(--sp-text-display);
  }

  /* Valid drop target while dragging a message over it. */
  .folder.drop-target {
    background: var(--sp-accent-bg);
    border-left-color: var(--sp-accent-edge);
    box-shadow: inset 0 0 0 1px var(--sp-accent-border);
    color: var(--sp-text-display);
  }

  .folder:focus-visible {
    outline: none;
    box-shadow: var(--sp-focus-ring);
  }

  .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .stitch {
    margin: var(--sp-2) var(--sp-3);
  }
</style>
