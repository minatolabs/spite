<script lang="ts">
  import { loadMore, mail } from './mail.svelte'

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
  {#each mail.messages as m (m.id)}
    <button
      class="row"
      class:unread={!m.is_read}
      class:selected={m.id === mail.selectedId}
      onclick={() => (mail.selectedId = m.id)}
    >
      <span class="top">
        <span class="sp-led" class:sp-led--off={m.is_read}></span>
        <span class="from">{m.from_name || m.from_address || '(unknown sender)'}</span>
        <span class="time">{fmtTime(m.received_at)}</span>
      </span>
      <span class="subject">{m.subject || '(no subject)'}</span>
      <span class="preview">{m.preview}</span>
    </button>
  {:else}
    <p class="empty">No messages in this folder yet.</p>
  {/each}
  {#if mail.hasMore}
    <button class="sp-btn more" onclick={() => void loadMore()}>Load more</button>
  {/if}
</div>

<style>
  .messages {
    display: flex;
    flex-direction: column;
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

  .more {
    margin: var(--sp-3) auto;
  }
</style>
