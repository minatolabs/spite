<script lang="ts">
  import { CloudDownload } from 'lucide-svelte'
  import { loadMore, mail, searchActive, type MessageSummary } from './mail.svelte'

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

  function selectLocal(id: string) {
    mail.serverSelected = null
    mail.selectedId = id
  }

  function selectServer(summary: MessageSummary) {
    mail.serverSelected = summary
    mail.selectedId = summary.id
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
        onclick={() => s && selectLocal(s.id)}
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
    {#each mail.messages as m (m.id)}
      <button
        class="row"
        class:unread={!m.is_read}
        class:selected={m.id === mail.selectedId}
        onclick={() => selectLocal(m.id)}
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
