<script lang="ts">
  import { Archive, CheckSquare, Flag, FolderInput, Mail, MailOpen, Trash2, X } from 'lucide-svelte'
  import {
    bulkArchive,
    bulkDelete,
    bulkFlag,
    bulkMarkRead,
    bulkMove,
    clearSelection,
    mail,
    selectAll,
  } from './mail.svelte'
  import FolderPicker from './FolderPicker.svelte'

  let showPicker = $state(false)
  let count = $derived(mail.selection.size)
</script>

{#if count > 0}
  <div class="bar">
    <span class="count">{count} selected</span>
    <button class="sp-btn" onclick={selectAll} title="Select all"><CheckSquare size={13} /> All</button>
    <span class="spacer"></span>
    <button class="sp-btn" onclick={() => void bulkMarkRead(true)}><MailOpen size={13} /> Read</button>
    <button class="sp-btn" onclick={() => void bulkMarkRead(false)}><Mail size={13} /> Unread</button>
    <button class="sp-btn" onclick={() => void bulkFlag(true)}><Flag size={13} /> Flag</button>
    <button class="sp-btn" onclick={() => void bulkArchive()}><Archive size={13} /> Archive</button>
    <button class="sp-btn" onclick={() => (showPicker = true)}><FolderInput size={13} /> Move</button>
    <button class="sp-btn sp-btn--danger" onclick={() => void bulkDelete()}><Trash2 size={13} /> Delete</button>
    <button class="sp-btn" onclick={clearSelection} title="Clear selection"><X size={13} /></button>
  </div>
{/if}

{#if showPicker}
  <FolderPicker
    onpick={(dest) => {
      showPicker = false
      void bulkMove(dest)
    }}
    onclose={() => (showPicker = false)}
  />
{/if}

<style>
  .bar {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2) var(--sp-4);
    background: var(--sp-surface-chrome);
    border-bottom: 1px solid var(--sp-border-hard);
    box-shadow: var(--sp-seam-top);
  }

  .count {
    font-size: var(--sp-fs-small);
    font-weight: 600;
    color: var(--sp-text-display);
  }

  .spacer {
    flex: 1;
  }
</style>
