<script lang="ts">
  import { onMount } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'
  import {
    Archive,
    Flag,
    FolderInput,
    Forward,
    ImageOff,
    Inbox,
    Mail,
    MailOpen,
    Reply,
    ReplyAll,
    Tag,
    Trash2,
    X,
  } from 'lucide-svelte'
  import {
    archive,
    mail,
    markRead,
    moveToFolder,
    setCategories,
    setFocused,
    softDelete,
    toggleFlag,
    toggleRead,
    type Message,
    type MessageBody,
  } from './mail.svelte'
  import FolderPicker from './FolderPicker.svelte'

  let showMovePicker = $state(false)

  // Auto-mark-read after a short dwell so arrow-key / j-k scrubbing past a
  // message doesn't mark it. Configurable; 0 disables.
  let autoReadMs = $state(500)
  onMount(() => {
    invoke<number>('get_auto_read_dwell')
      .then((ms) => (autoReadMs = ms))
      .catch(() => {})
  })

  let addingCategory = $state(false)
  let newCategory = $state('')

  async function addCategory() {
    const name = newCategory.trim()
    if (!name || !message) return
    const next = [...message.categories, name]
    message = { ...message, categories: next }
    addingCategory = false
    newCategory = ''
    await setCategories(message.summary.id, next)
  }

  async function removeCategory(cat: string) {
    if (!message) return
    const next = message.categories.filter((c) => c !== cat)
    message = { ...message, categories: next }
    await setCategories(message.summary.id, next)
  }

  function afterListChange() {
    // Archive/delete remove the row and clear selection.
    message = null
  }

  // Reading-pane action wrappers: flip the local `message` optimistically too
  // (the store helpers update the list, not this pane's copy), so the button
  // reflects the change instantly.
  function readToggle() {
    if (!message) return
    const s = message.summary
    toggleRead(s)
    message = { ...message, summary: { ...s, is_read: !s.is_read } }
  }
  function flagToggle() {
    if (!message) return
    const s = message.summary
    const next = s.flag_status === 'flagged' ? 'notFlagged' : 'flagged'
    toggleFlag(s)
    message = { ...message, summary: { ...s, flag_status: next } }
  }
  function focusMove(focused: boolean) {
    if (!message) return
    const s = message.summary
    setFocused(s, focused)
    message = {
      ...message,
      summary: { ...s, inference_classification: focused ? 'focused' : 'other' },
    }
  }

  let message: Message | null = $state(null)
  let body: MessageBody | null = $state(null)
  let bodyState: 'idle' | 'loading' | 'ready' | 'unavailable' = $state('idle')
  let bodyError = $state('')
  let allowRemote = $state(false)

  let hasRemoteImages = $derived.by(() => {
    const b: MessageBody | null = body
    return !!b && b.content_type === 'html' && /<img[^>]+src=["']https?:/i.test(b.body)
  })

  // Auto-mark-read after the configured dwell. $effect re-runs (and runs its
  // cleanup) on every selection change, so scrubbing past a message with
  // arrow keys / j-k cancels the pending mark before it fires.
  $effect(() => {
    const id = mail.selectedId
    if (!id || autoReadMs <= 0 || mail.serverSelected) return
    const summary = mail.messages.find((m) => m.id === id)
    if (!summary || summary.is_read) return
    const timer = setTimeout(() => {
      const cur = mail.messages.find((m) => m.id === id)
      if (mail.selectedId === id && cur && !cur.is_read) {
        markRead(cur)
        if (message && message.summary.id === id) {
          message = { ...message, summary: { ...message.summary, is_read: true } }
        }
      }
    }, autoReadMs)
    return () => clearTimeout(timer)
  })

  $effect(() => {
    const id = mail.selectedId
    message = null
    body = null
    bodyError = ''
    allowRemote = false
    bodyState = id ? 'loading' : 'idle'
    if (!id) return
    void (async () => {
      try {
        let m = await invoke<Message | null>('get_message', { id })
        if (mail.selectedId !== id) return
        // Server-only hit: not in the local store — synthesize the header
        // block from the search hit's summary.
        if (!m && mail.serverSelected?.id === id) {
          m = {
            summary: mail.serverSelected,
            conversation_id: null,
            body_html: null,
            body_content_type: null,
            categories: [],
          }
        }
        message = m
        if (m) {
          allowRemote = await invoke<boolean>('get_sender_pref', {
            address: m.summary.from_address,
          })
        }
        const b = await invoke<MessageBody>('fetch_message_body', { id })
        if (mail.selectedId !== id) return
        body = b
        bodyState = 'ready'
      } catch (e) {
        if (mail.selectedId !== id) return
        bodyError = String(e)
        bodyState = 'unavailable'
      }
    })()
  })

  // Security path (see core/src/sanitize.rs): `html` here is ALREADY
  // sanitized by ammonia in Rust — the raw message never reaches this
  // process. It still renders inside a fully sandboxed iframe (bare
  // `sandbox`: no scripts, no same-origin, no forms) whose per-document
  // meta CSP is the remote-image gate: img-src omits https until the user
  // allows this sender.
  function srcdoc(html: string, remote: boolean): string {
    const imgSrc = remote ? 'data: cid: https:' : 'data: cid:'
    return `<!doctype html><html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src ${imgSrc}">
<style>
  body {
    font-family: 'Iowan Old Style', Charter, 'Bitstream Charter', 'Source Serif 4', Georgia, serif;
    font-size: 14px; line-height: 1.7;
    color: #cfc2b5; background: #141110;
    margin: 16px; overflow-wrap: anywhere;
  }
  a { color: #c98a7e; }
  img { max-width: 100%; height: auto; }
  pre { white-space: pre-wrap; }
  blockquote { border-left: 2px solid #3a332f; margin-left: 0; padding-left: 12px; color: #b6a99d; }
  table { max-width: 100%; }
</style></head><body>${html}</body></html>`
  }

  async function loadImagesOnce() {
    allowRemote = true
  }

  async function alwaysAllowSender() {
    if (!message) return
    await invoke('set_sender_pref', {
      address: message.summary.from_address,
      allowRemoteImages: true,
    })
    allowRemote = true
  }

  function openCompose(composeMode: 'reply' | 'replyAll' | 'forward') {
    if (!mail.selectedId) return
    void invoke('open_compose', { mode: composeMode, messageId: mail.selectedId })
  }

  function fmtDate(epoch: number): string {
    if (!epoch) return ''
    return new Date(epoch * 1000).toLocaleString(undefined, {
      weekday: 'short',
      year: 'numeric',
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    })
  }
</script>

{#if !mail.selectedId}
  <div class="placeholder">
    <Mail size={28} />
    <p>Select a message to read it.</p>
  </div>
{:else}
  <article class="pane">
    {#if message}
      <header class="headers">
        <h1 class="subject">{message.summary.subject || '(no subject)'}</h1>
        <p class="meta">
          <span class="from-name">{message.summary.from_name || '(unknown sender)'}</span>
          {#if message.summary.from_address}
            <span class="from-addr">&lt;{message.summary.from_address}&gt;</span>
          {/if}
          <span class="date">{fmtDate(message.summary.received_at)}</span>
        </p>
        <p class="msg-actions">
          <button class="sp-btn" onclick={() => openCompose('reply')}>
            <Reply size={13} /> Reply
          </button>
          <button class="sp-btn" onclick={() => openCompose('replyAll')}>
            <ReplyAll size={13} /> Reply all
          </button>
          <button class="sp-btn" onclick={() => openCompose('forward')}>
            <Forward size={13} /> Forward
          </button>
          <span class="gap"></span>
          <button
            class="sp-btn"
            onclick={readToggle}
            title={message.summary.is_read ? 'Mark unread' : 'Mark read'}
          >
            {#if message.summary.is_read}<Mail size={13} />{:else}<MailOpen size={13} />{/if}
          </button>
          <button
            class="sp-btn"
            class:flagged={message.summary.flag_status === 'flagged'}
            onclick={flagToggle}
            title="Flag"
          >
            <Flag size={13} />
          </button>
          <button
            class="sp-btn"
            onclick={() => {
              if (message) {
                void archive(message.summary.id)
                afterListChange()
              }
            }}
            title="Archive"
          >
            <Archive size={13} />
          </button>
          <button
            class="sp-btn sp-btn--danger"
            onclick={() => {
              if (message) {
                void softDelete(message.summary.id)
                afterListChange()
              }
            }}
            title="Delete (to Deleted Items)"
          >
            <Trash2 size={13} />
          </button>
          <button class="sp-btn" onclick={() => (showMovePicker = true)} title="Move to folder">
            <FolderInput size={13} /> Move
          </button>
          {#if message.summary.inference_classification === 'other'}
            <button class="sp-btn" onclick={() => focusMove(true)}>
              <Inbox size={13} /> Move to Focused
            </button>
          {:else}
            <button class="sp-btn" onclick={() => focusMove(false)}>Move to Other</button>
          {/if}
        </p>
        <p class="categories">
          <Tag size={12} />
          {#each message.categories as cat (cat)}
            <span class="cat">
              {cat}
              <button class="cat-x" onclick={() => removeCategory(cat)}><X size={10} /></button>
            </span>
          {/each}
          {#if addingCategory}
            <input
              class="cat-input"
              bind:value={newCategory}
              onkeydown={(e) => {
                if (e.key === 'Enter') void addCategory()
                if (e.key === 'Escape') addingCategory = false
              }}
              placeholder="category…"
            />
          {:else}
            <button class="cat-add" onclick={() => (addingCategory = true)}>+ category</button>
          {/if}
        </p>
      </header>
    {/if}

    {#if bodyState === 'ready' && body && hasRemoteImages && !allowRemote}
      <div class="images-bar">
        <ImageOff size={13} />
        <span>Remote images blocked.</span>
        <button class="sp-btn" onclick={loadImagesOnce}>Load images</button>
        <button class="sp-btn" onclick={alwaysAllowSender}>Always for this sender</button>
      </div>
    {/if}

    <div class="body sp-scroll">
      {#if bodyState === 'loading'}
        <p class="state muted">Loading message…</p>
      {:else if bodyState === 'unavailable'}
        <div class="state">
          <p class="muted">
            This message body hasn't been downloaded yet, and it couldn't be
            fetched right now — you may be offline.
          </p>
          <p class="detail">{bodyError}</p>
        </div>
      {:else if bodyState === 'ready' && body}
        {#if body.content_type === 'html'}
          <!-- Sanitized in Rust; sandboxed; CSP-gated. Never rendered inline. -->
          <iframe
            sandbox=""
            title="Message body"
            srcdoc={srcdoc(body.body, allowRemote)}
          ></iframe>
        {:else}
          <pre class="text-body sp-body-serif">{body.body}</pre>
        {/if}
      {/if}
    </div>
  </article>
{/if}

{#if showMovePicker && message}
  <FolderPicker
    excludeId={message.summary.folder_id}
    onpick={(dest) => {
      showMovePicker = false
      if (message) {
        void moveToFolder(message.summary.id, dest)
        afterListChange()
      }
    }}
    onclose={() => (showMovePicker = false)}
  />
{/if}

<style>
  .placeholder {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--sp-3);
    color: var(--sp-text-muted);
  }

  .pane {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .headers {
    padding: var(--sp-4) var(--sp-5) var(--sp-3);
    border-bottom: 1px solid var(--sp-border-hard);
    box-shadow: var(--sp-seam-top);
  }

  .subject {
    margin: 0 0 var(--sp-2);
    font-size: var(--sp-fs-subject);
    font-weight: 600;
    color: var(--sp-text-display);
  }

  .meta {
    margin: 0;
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-2);
    font-size: var(--sp-fs-small);
  }

  .from-name {
    color: var(--sp-text-primary);
    font-weight: 500;
  }

  .from-addr {
    color: var(--sp-text-tertiary);
  }

  .date {
    margin-left: auto;
    color: var(--sp-text-tertiary);
  }

  .msg-actions {
    margin: var(--sp-3) 0 0;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--sp-2);
  }

  .gap {
    width: var(--sp-3);
  }

  .sp-btn.flagged {
    color: var(--sp-flag);
  }

  .categories {
    margin: var(--sp-2) 0 0;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--sp-2);
    color: var(--sp-text-muted);
  }

  .cat {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 1px var(--sp-2);
    font-size: var(--sp-fs-caption);
    color: var(--sp-text-secondary);
    background: var(--ink-700);
    border: 1px solid var(--sp-border-control);
    border-radius: var(--sp-r-pill);
  }

  .cat-x,
  .cat-add {
    border: none;
    background: none;
    color: var(--sp-text-tertiary);
    cursor: pointer;
    font: inherit;
    font-size: var(--sp-fs-caption);
    padding: 0 2px;
    display: inline-flex;
    align-items: center;
  }

  .cat-add:hover {
    color: var(--sp-text-primary);
  }

  .cat-input {
    width: 100px;
    border: 1px solid var(--sp-border-hard);
    background: var(--sp-surface-well);
    color: var(--sp-text-primary);
    border-radius: var(--sp-r-control);
    font: inherit;
    font-size: var(--sp-fs-caption);
    padding: 2px var(--sp-2);
    outline: none;
  }

  .images-bar {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2) var(--sp-5);
    font-size: var(--sp-fs-small);
    color: var(--sp-text-secondary);
    background: var(--sp-surface-sunken);
    border-bottom: 1px solid var(--sp-border-hard);
  }

  .body {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: auto;
  }

  iframe {
    flex: 1;
    width: 100%;
    border: none;
    background: var(--sp-surface-raised);
  }

  .text-body {
    margin: 0;
    padding: var(--sp-5);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .state {
    padding: var(--sp-5);
  }

  .muted {
    color: var(--sp-text-secondary);
  }

  .detail {
    margin-top: var(--sp-2);
    font-size: var(--sp-fs-caption);
    color: var(--sp-text-muted);
    overflow-wrap: anywhere;
  }
</style>
