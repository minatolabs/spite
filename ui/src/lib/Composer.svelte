<script lang="ts">
  import { onMount } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'
  import { getCurrentWindow } from '@tauri-apps/api/window'
  import {
    Bold,
    Italic,
    Link,
    List,
    ListOrdered,
    Paperclip,
    Quote,
    Send,
    Type,
  } from 'lucide-svelte'
  import AddressField from './AddressField.svelte'

  type EmailAddress = { name: string; address: string }
  type ComposeContext = {
    mode: string
    to: EmailAddress[]
    cc: EmailAddress[]
    subject: string
    quoted_html: string | null
    in_reply_to: string | null
    references: string[]
    signature: string | null
    degraded: boolean
  }
  type Attachment = { name: string; content_type: string; content_base64: string; size: number }

  let { label }: { label: string } = $props()

  const MAX_TOTAL = 2 * 1024 * 1024

  let mode = $state('new')
  let to: EmailAddress[] = $state([])
  let cc: EmailAddress[] = $state([])
  let bcc: EmailAddress[] = $state([])
  let showCcBcc = $state(false)
  let subject = $state('')
  let plainText = $state(false)
  let textBody = $state('')
  let bodyEl: HTMLDivElement | undefined = $state()
  let attachments: Attachment[] = $state([])
  let inReplyTo: string | null = $state(null)
  let references: string[] = $state([])
  let degraded = $state(false)
  let loading = $state(true)
  let sending = $state(false)
  let confirming = $state(false)
  let error = $state('')
  let attachError = $state('')
  let dirty = $state(false)

  function escapeHtml(s: string): string {
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
  }

  onMount(() => {
    void (async () => {
      try {
        const ctx = await invoke<ComposeContext>('get_compose_context', { label })
        mode = ctx.mode
        to = ctx.to
        cc = ctx.cc
        showCcBcc = ctx.cc.length > 0
        subject = ctx.subject
        inReplyTo = ctx.in_reply_to
        references = ctx.references
        degraded = ctx.degraded
        const sig = ctx.signature
          ? `<p><br></p><p>--&nbsp;<br>${escapeHtml(ctx.signature).replace(/\n/g, '<br>')}</p>`
          : '<p><br></p>'
        if (bodyEl) {
          // quoted_html is sanitized in Rust (ammonia) before it gets here.
          bodyEl.innerHTML = sig + (ctx.quoted_html ? `<hr>${ctx.quoted_html}` : '')
        }
      } catch (e) {
        error = String(e)
      } finally {
        loading = false
      }
    })()

    const win = getCurrentWindow()
    const unlistenPromise = win.onCloseRequested((event) => {
      if (dirty && !confirm('Discard this draft?')) {
        event.preventDefault()
      }
    })
    return () => {
      void unlistenPromise.then((fn) => fn())
    }
  })

  function markDirty() {
    dirty = true
  }

  function exec(command: string, value?: string) {
    document.execCommand(command, false, value)
    bodyEl?.focus()
    markDirty()
  }

  function addLink() {
    const url = prompt('Link URL (https://…):')
    if (url && /^https?:\/\//i.test(url)) exec('createLink', url)
  }

  function togglePlainText() {
    if (!plainText) {
      textBody = bodyEl?.innerText ?? ''
    } else if (bodyEl) {
      bodyEl.innerHTML = `<p>${escapeHtml(textBody).replace(/\n/g, '<br>')}</p>`
    }
    plainText = !plainText
    markDirty()
  }

  async function pickFiles(e: Event) {
    attachError = ''
    const files = (e.target as HTMLInputElement).files
    if (!files) return
    for (const file of files) {
      const currentTotal = attachments.reduce((n, a) => n + a.size, 0)
      if (currentTotal + file.size > MAX_TOTAL) {
        attachError = `"${file.name}" is too large — inline attachments are capped at 2 MB total; larger uploads arrive with a later phase.`
        continue
      }
      const b64 = await new Promise<string>((resolve, reject) => {
        const r = new FileReader()
        r.onload = () => resolve(String(r.result).split(',', 2)[1] ?? '')
        r.onerror = () => reject(r.error)
        r.readAsDataURL(file)
      })
      attachments = [
        ...attachments,
        {
          name: file.name,
          content_type: file.type || 'application/octet-stream',
          content_base64: b64,
          size: file.size,
        },
      ]
      markDirty()
    }
    ;(e.target as HTMLInputElement).value = ''
  }

  function removeAttachment(i: number) {
    attachments = attachments.toSpliced(i, 1)
  }

  let recipientCount = $derived(to.length + cc.length + bcc.length)

  function requestSend() {
    error = ''
    if (!recipientCount) {
      error = 'Add at least one recipient.'
      return
    }
    confirming = true
  }

  async function confirmSend() {
    confirming = false
    sending = true
    error = ''
    try {
      const body = plainText ? textBody : (bodyEl?.innerHTML ?? '')
      await invoke('send_mail', {
        draft: {
          to,
          cc,
          bcc,
          subject,
          body,
          content_type: plainText ? 'text' : 'html',
          in_reply_to: inReplyTo,
          references,
          attachments: attachments.map(({ name, content_type, content_base64 }) => ({
            name,
            content_type,
            content_base64,
          })),
        },
      })
      dirty = false
      await getCurrentWindow().close()
    } catch (e) {
      // Draft stays exactly as composed; the user can fix and retry.
      error = String(e)
    } finally {
      sending = false
    }
  }

  function onKeydown(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
      e.preventDefault()
      requestSend()
    }
  }
</script>

<svelte:window onkeydown={onKeydown} />

<main class="composer">
  {#if degraded}
    <div class="notice">
      Offline: composed from local data — threading headers are unavailable and
      reply-all may be reduced to reply.
    </div>
  {/if}

  <div class="fields">
    <AddressField label="To" bind:chips={to} onchange={markDirty} />
    {#if showCcBcc}
      <AddressField label="Cc" bind:chips={cc} onchange={markDirty} />
      <AddressField label="Bcc" bind:chips={bcc} onchange={markDirty} />
    {:else}
      <button class="ccbcc" onclick={() => (showCcBcc = true)}>Cc / Bcc</button>
    {/if}
    <div class="subject-row">
      <span class="label">Subj</span>
      <input
        class="subject"
        bind:value={subject}
        oninput={markDirty}
        placeholder="Subject"
      />
    </div>
  </div>

  <div class="editor-toolbar">
    {#if !plainText}
      <button class="sp-btn" onclick={() => exec('bold')} title="Bold"><Bold size={13} /></button>
      <button class="sp-btn" onclick={() => exec('italic')} title="Italic"><Italic size={13} /></button>
      <button class="sp-btn" onclick={() => exec('insertUnorderedList')} title="Bullet list"><List size={13} /></button>
      <button class="sp-btn" onclick={() => exec('insertOrderedList')} title="Numbered list"><ListOrdered size={13} /></button>
      <button class="sp-btn" onclick={addLink} title="Link"><Link size={13} /></button>
      <button class="sp-btn" onclick={() => exec('formatBlock', 'blockquote')} title="Quote"><Quote size={13} /></button>
    {/if}
    <span class="spacer"></span>
    <button class="sp-btn" onclick={togglePlainText} title="Toggle plain text">
      <Type size={13} />
      {plainText ? 'Rich text' : 'Plain text'}
    </button>
    <label class="sp-btn attach" title="Attach files (2 MB total)">
      <Paperclip size={13} /> Attach
      <input type="file" multiple onchange={pickFiles} />
    </label>
  </div>

  {#if loading}
    <p class="state">Preparing…</p>
  {:else if plainText}
    <textarea
      class="body-text sp-scroll"
      bind:value={textBody}
      oninput={markDirty}
      spellcheck="true"
    ></textarea>
  {:else}
    <div
      class="body-html sp-body-serif sp-scroll"
      contenteditable="true"
      bind:this={bodyEl}
      oninput={markDirty}
      role="textbox"
      aria-multiline="true"
      aria-label="Message body"
      tabindex="0"
    ></div>
  {/if}

  {#if attachments.length || attachError}
    <div class="attachments">
      {#each attachments as att, i (att.name + i)}
        <span class="chip" title="{att.name} ({Math.round(att.size / 1024)} KB)">
          {att.name}
          <button class="x" onclick={() => removeAttachment(i)}>×</button>
        </span>
      {/each}
      {#if attachError}<span class="attach-error">{attachError}</span>{/if}
    </div>
  {/if}

  {#if error}
    <div class="error" role="alert">{error}</div>
  {/if}

  <footer class="actions">
    {#if confirming}
      <span class="confirm-text">
        Send to {recipientCount} recipient{recipientCount === 1 ? '' : 's'}?
      </span>
      <button class="sp-btn sp-btn--primary" onclick={confirmSend} disabled={sending}>
        <Send size={13} /> Confirm send
      </button>
      <button class="sp-btn" onclick={() => (confirming = false)}>Back</button>
    {:else}
      <button class="sp-btn sp-btn--primary" onclick={requestSend} disabled={sending || loading}>
        <Send size={13} />
        {sending ? 'Sending…' : 'Send'}
      </button>
      <span class="hint">Ctrl+Enter to send · plain Enter never sends</span>
    {/if}
  </footer>
</main>

<style>
  .composer {
    height: 100svh;
    display: flex;
    flex-direction: column;
    background: var(--sp-surface-raised);
  }

  .notice {
    padding: var(--sp-2) var(--sp-4);
    font-size: var(--sp-fs-small);
    color: var(--sp-text-accent);
    background: var(--sp-surface-sunken);
    border-bottom: 1px solid var(--sp-border-hard);
  }

  .fields {
    padding: var(--sp-2) var(--sp-4) 0;
  }

  .ccbcc {
    border: none;
    background: none;
    color: var(--sp-text-tertiary);
    font-size: var(--sp-fs-caption);
    cursor: pointer;
    padding: var(--sp-1) 0;
  }

  .ccbcc:hover {
    color: var(--sp-text-primary);
  }

  .subject-row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-1) 0;
    border-bottom: var(--sp-stitch-strong);
  }

  .label {
    flex: none;
    width: 42px;
    font-size: var(--sp-fs-caption);
    text-transform: uppercase;
    letter-spacing: var(--sp-track-label);
    color: var(--sp-text-muted);
  }

  .subject {
    flex: 1;
    border: none;
    background: transparent;
    color: var(--sp-text-display);
    font: inherit;
    font-size: var(--sp-fs-md);
    padding: 5px 0;
    outline: none;
  }

  .editor-toolbar {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    padding: var(--sp-2) var(--sp-4);
    background: var(--sp-surface-chrome);
    border-bottom: 1px solid var(--sp-border-hard);
  }

  .spacer {
    flex: 1;
  }

  .attach input[type='file'] {
    display: none;
  }

  .body-html,
  .body-text {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: var(--sp-4);
    outline: none;
    border: none;
    background: transparent;
    color: var(--sp-text-body);
    font-size: var(--sp-fs-md);
    line-height: var(--sp-lh-body);
    resize: none;
  }

  .body-text {
    font-family: var(--sp-font-mono);
    font-size: var(--sp-fs-body);
  }

  .body-html :global(blockquote) {
    border-left: 2px solid var(--ink-400);
    margin-left: 0;
    padding-left: var(--sp-3);
    color: var(--sp-text-secondary);
  }

  .attachments {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-2);
    align-items: center;
    padding: var(--sp-2) var(--sp-4);
    border-top: var(--sp-stitch);
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px var(--sp-2);
    font-size: var(--sp-fs-small);
    color: var(--sp-text-primary);
    background: var(--ink-700);
    border: 1px solid var(--sp-border-control);
    border-radius: var(--sp-r-pill);
  }

  .x {
    border: none;
    background: none;
    color: var(--sp-text-tertiary);
    cursor: pointer;
    padding: 0 2px;
  }

  .attach-error {
    font-size: var(--sp-fs-small);
    color: var(--sp-danger);
  }

  .error {
    padding: var(--sp-2) var(--sp-4);
    font-size: var(--sp-fs-small);
    color: var(--sp-danger-hover);
    background: rgba(199, 62, 70, 0.12);
    border-top: 1px solid var(--sp-border-hard);
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-2) var(--sp-4);
    background: var(--sp-surface-chrome);
    border-top: 1px solid var(--sp-border-hard);
  }

  .hint {
    font-size: var(--sp-fs-caption);
    color: var(--sp-text-muted);
  }

  .confirm-text {
    font-size: var(--sp-fs-small);
    color: var(--sp-text-display);
  }

  .state {
    padding: var(--sp-4);
    color: var(--sp-text-secondary);
  }
</style>
