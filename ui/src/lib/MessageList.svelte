<script lang="ts">
  import { CloudDownload, Flag } from 'lucide-svelte'
  import {
    clickSelect,
    isSelected,
    loadMore,
    mail,
    searchActive,
    selectedFolder,
    type MessageSummary,
  } from './mail.svelte'

  // Focused/Other tabs apply only to the Inbox browse view.
  let showFocusTabs = $derived(!searchActive() && selectedFolder()?.well_known_name === 'inbox')
  let browseRows = $derived.by(() =>
    mail.focusTab === 'all'
      ? mail.messages
      : mail.messages.filter((m) => m.inference_classification === mail.focusTab),
  )

  /** Split highlighted text on the private-use markers the store emits.
   *  Rendering happens via DOM text nodes + <mark> elements — never HTML. */
  function markParts(s: string): { text: string; mark: boolean }[] {
    const out: { text: string; mark: boolean }[] = []
    let mark = false
    for (const piece of s.split(/([\uE000\uE001])/)) {
      if (piece === '\uE000') mark = true
      else if (piece === '\uE001') mark = false
      else if (piece) out.push({ text: piece, mark })
    }
    return out
  }

  function rowClick(id: string, e: MouseEvent | KeyboardEvent) {
    // Ctrl/Cmd/Shift = multi-select; plain click = single-select + open.
    clickSelect(id, { shift: e.shiftKey, ctrl: e.ctrlKey || e.metaKey })
  }

  function selectServer(summary: MessageSummary) {
    mail.serverSelected = summary
    mail.selectedId = summary.id
  }

  // Drag payload: the whole selection if the dragged row is part of it, else
  // just that row. Consumed by FolderTree drop targets.
  function onDragStart(id: string, e: DragEvent) {
    const ids = isSelected(id) && mail.selection.size > 1 ? [...mail.selection] : [id]
    e.dataTransfer?.setData('application/x-spite-messages', JSON.stringify(ids))
    if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'
  }

  function fmtTime(epoch: number): string {
    if (!epoch) return ''
    const d = new Date(epoch * 1000)
    const now = new Date()
    const sameDay = d.toDateString() === now.toDateString()
    if (sameDay) {
      return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
    }
    if (d.getFullYear() === now.getFullYear()) {
      return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
    }
    return d.toLocaleDateString(undefined, {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
    })
  }
</script>

<div class="messages">
  {#if searchActive()}
    {#each mail.hits as hit (hit.entity_type + hit.entity_id)}
      {@const s = hit.summary}
      <button
        class="row"
        class:unread={s ? !s.is_read : false}
        class:selected={hit.entity_id === mail.selectedId}
        data-message-id={s ? s.id : undefined}
        onclick={(e) => s && rowClick(s.id, e)}
      >
        <span class="top">
          {#if s}
            <span class="sp-led" class:sp-led--off={s.is_read}></span>
            <span class="from">{s.from_name || s.from_address || '(unknown sender)'}</span>
            <span class="time">{fmtTime(hit.ts)}</span>
          {:else}
            <span class="badge sp-badge">{hit.entity_type}</span>
            <span class="time">{fmtTime(hit.ts)}</span>
          {/if}
        </span>
        <span class="subject">
          {#each markParts(hit.title) as part, i (i)}
            {#if part.mark}<mark>{part.text}</mark>{:else}{part.text}{/if}
          {/each}
        </span>
        <span class="preview">
          {#each markParts(hit.snippet) as part, i (i)}
            {#if part.mark}<mark>{part.text}</mark>{:else}{part.text}{/if}
          {/each}
        </span>
      </button>
    {:else}
      <p class="empty">
        {mail.searchError || 'No local matches.'}
      </p>
    {/each}

    {#if mail.serverHits.length || mail.serverSearched}
      <div class="server-divider sp-stitch-h">
        <span>server results</span>
      </div>
      {#each mail.serverHits as hit (hit.summary.id)}
        {@const s = hit.summary}
        <button
          class="row"
          class:selected={s.id === mail.selectedId}
          onclick={() => selectServer(s)}
        >
          <span class="top">
            <span class="badge sp-badge"><CloudDownload size={10} /> server</span>
            <span class="from">{s.from_name || s.from_address || '(unknown sender)'}</span>
            <span class="time">{fmtTime(s.received_at)}</span>
          </span>
          <span class="subject">{s.subject || '(no subject)'}</span>
          <span class="preview">{s.preview}</span>
        </button>
      {:else}
        <p class="empty">No additional server results.</p>
      {/each}
    {:else if mail.query.trim() && mail.hits.length < 3}
      <p class="hint">Thin local results — try “Everywhere” for a deep server search.</p>
    {/if}
  {:else}
    {#if showFocusTabs}
      <div class="focus-tabs">
        <button class:on={mail.focusTab === 'all'} onclick={() => (mail.focusTab = 'all')}>All</button>
        <button
          class:on={mail.focusTab === 'focused'}
          onclick={() => (mail.focusTab = 'focused')}>Focused</button
        >
        <button class:on={mail.focusTab === 'other'} onclick={() => (mail.focusTab = 'other')}
          >Other</button
        >
      </div>
    {/if}
    {#if mail.folderLoading}
      <p class="empty">Loading {selectedFolder()?.display_name ?? 'folder'}…</p>
    {/if}
    {#each browseRows as m (m.id)}
      <div
        class="row browse"
        class:unread={!m.is_read}
        class:selected={m.id === mail.selectedId}
        class:multi={isSelected(m.id)}
        data-message-id={m.id}
        role="button"
        tabindex="0"
        draggable="true"
        ondragstart={(e) => onDragStart(m.id, e)}
        onclick={(e) => rowClick(m.id, e)}
        onkeydown={(e) => e.key === 'Enter' && rowClick(m.id, e)}
      >
        <input
          class="check"
          type="checkbox"
          checked={isSelected(m.id)}
          onclick={(e) => {
            e.stopPropagation()
            clickSelect(m.id, { ctrl: true })
          }}
          aria-label="Select message"
        />
        <div class="row-body">
          <span class="top">
            <span class="sp-led" class:sp-led--off={m.is_read}></span>
            <span class="from">{m.from_name || m.from_address || '(unknown sender)'}</span>
            {#if m.flag_status === 'flagged'}<Flag size={11} class="flag" />{/if}
            <span class="time">{fmtTime(m.received_at)}</span>
          </span>
          <span class="subject">{m.subject || '(no subject)'}</span>
          <span class="preview">{m.preview}</span>
        </div>
      </div>
    {:else}
      {#if !mail.folderLoading}
        <p class="empty">
          {mail.focusTab === 'all'
            ? 'No messages in this folder yet.'
            : `No ${mail.focusTab} messages.`}
        </p>
      {/if}
    {/each}
    {#if mail.hasMore && mail.focusTab === 'all'}
      <button class="sp-btn more" onclick={() => void loadMore()}>Load more</button>
    {/if}
  {/if}
</div>

<style>
  .messages {
    display: flex;
    flex-direction: column;
  }

  .focus-tabs {
    display: flex;
    border-bottom: 1px solid var(--sp-border-hard);
    background: var(--sp-surface-sunken);
  }

  .focus-tabs button {
    flex: 1;
    padding: var(--sp-2);
    border: none;
    background: transparent;
    color: var(--sp-text-secondary);
    font: 500 var(--sp-fs-small) / 1 var(--sp-font-ui);
    cursor: pointer;
    border-bottom: 2px solid transparent;
  }

  .focus-tabs button.on {
    color: var(--sp-text-display);
    border-bottom-color: var(--sp-accent-edge);
  }

  :global(.flag) {
    color: var(--sp-flag);
    flex: none;
  }

  .row {
    display: flex;
    flex-direction: column;
    gap: 3px;
    width: 100%;
    padding: var(--sp-2) var(--sp-3);
    border: none;
    border-left: 3px solid transparent;
    border-bottom: 1px solid #060505;
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.03);
    background: transparent;
    color: var(--sp-text-secondary);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }

  .row:hover {
    background: var(--ink-800);
  }

  .row.selected {
    background: var(--sp-selected-fill);
    border-left-color: var(--sp-accent-edge);
  }

  .row:focus-visible {
    outline: none;
    box-shadow: var(--sp-focus-ring);
  }

  /* Browse rows carry a checkbox beside a body column and are draggable. */
  .row.browse {
    flex-direction: row;
    align-items: center;
    gap: var(--sp-2);
    cursor: grab;
  }

  .row.browse:active {
    cursor: grabbing;
  }

  .row.browse.multi {
    background: var(--sp-selected-fill);
  }

  .row-body {
    display: flex;
    flex-direction: column;
    gap: 3px;
    flex: 1;
    min-width: 0;
  }

  .check {
    flex: none;
    accent-color: var(--sp-accent-edge);
    opacity: 0;
    transition: opacity var(--sp-dur-fast) var(--sp-ease);
  }

  .row.browse:hover .check,
  .row.browse.multi .check {
    opacity: 1;
  }

  .top {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
  }

  .from {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--sp-fs-small);
    color: var(--sp-text-secondary);
  }

  .row.unread .from {
    color: var(--sp-text-primary);
    font-weight: 600;
  }

  .time {
    font-size: var(--sp-fs-caption);
    color: var(--sp-text-tertiary);
    flex: none;
  }

  .subject {
    font-size: var(--sp-fs-body);
    color: var(--sp-text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .row.unread .subject {
    color: var(--sp-text-display);
    font-weight: 500;
  }

  .preview {
    font-size: var(--sp-fs-small);
    color: var(--sp-text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .empty {
    padding: var(--sp-4);
    color: var(--sp-text-muted);
    text-align: center;
  }

  .hint {
    padding: var(--sp-3) var(--sp-4);
    font-size: var(--sp-fs-caption);
    color: var(--sp-text-tertiary);
    text-align: center;
  }

  .badge {
    display: inline-flex;
    align-items: center;
    gap: 3px;
  }

  .server-divider {
    display: flex;
    justify-content: center;
    padding: var(--sp-2) 0 var(--sp-1);
  }

  .server-divider span {
    font-size: var(--sp-fs-caption);
    letter-spacing: var(--sp-track-label);
    text-transform: uppercase;
    color: var(--sp-text-muted);
  }

  mark {
    background: rgba(138, 43, 49, 0.35);
    color: var(--sp-text-display);
    border-radius: 2px;
    padding: 0 1px;
  }

  .more {
    margin: var(--sp-3) auto;
  }
</style>
