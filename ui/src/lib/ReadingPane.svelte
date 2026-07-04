<script lang="ts">
  import { invoke } from '@tauri-apps/api/core'
  import { Forward, ImageOff, Mail, Reply, ReplyAll } from 'lucide-svelte'
  import { mail, type Message, type MessageBody } from './mail.svelte'

  let message: Message | null = $state(null)
  let body: MessageBody | null = $state(null)
  let bodyState: 'idle' | 'loading' | 'ready' | 'unavailable' = $state('idle')
  let bodyError = $state('')
  let allowRemote = $state(false)

  let hasRemoteImages = $derived.by(() => {
    const b: MessageBody | null = body
    return !!b && b.content_type === 'html' && /<img[^>]+src=["']https?:/i.test(b.body)
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
        const m = await invoke<Message | null>('get_message', { id })
        if (mail.selectedId !== id) return
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
    gap: var(--sp-2);
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
