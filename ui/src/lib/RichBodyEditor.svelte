<script lang="ts">
  import { Bold, Italic, Link, Type } from 'lucide-svelte'

  // Minimal rich editor (bold/italic/link) with a plain-text toggle, mirroring
  // the Composer. `value` is always valid HTML; the plain textarea edits an
  // escaped view of it. The parent renders this only after settings load, so
  // the contenteditable can seed from `value` on mount.
  let {
    value = $bindable(''),
    placeholder = '',
  }: { value?: string; placeholder?: string } = $props()

  let bodyEl = $state<HTMLDivElement>()
  let plain = $state(false)
  let textBody = $state('')
  let initialHtml = value
  let seeded = false

  $effect(() => {
    if (!plain && bodyEl && !seeded) {
      bodyEl.innerHTML = initialHtml
      seeded = true
    }
  })

  function onInput() {
    if (bodyEl) value = bodyEl.innerHTML
  }
  function exec(cmd: string, arg?: string) {
    document.execCommand(cmd, false, arg)
    bodyEl?.focus()
    onInput()
  }
  function addLink() {
    const url = prompt('Link URL (https://…):')
    if (url && /^https?:\/\//i.test(url)) exec('createLink', url)
  }
  function escapeHtml(s: string) {
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
  }
  function syncFromText() {
    value = `<p>${escapeHtml(textBody).replace(/\n/g, '<br>')}</p>`
  }
  function togglePlain() {
    if (!plain) {
      textBody = bodyEl?.innerText ?? ''
      syncFromText()
    } else {
      // Returning to rich: re-seed the contenteditable from the current HTML.
      initialHtml = value
      seeded = false
    }
    plain = !plain
  }
</script>

<div class="rich">
  <div class="mini-toolbar">
    {#if !plain}
      <button class="sp-btn" onclick={() => exec('bold')} title="Bold"><Bold size={12} /></button>
      <button class="sp-btn" onclick={() => exec('italic')} title="Italic"><Italic size={12} /></button>
      <button class="sp-btn" onclick={addLink} title="Link"><Link size={12} /></button>
    {/if}
    <span class="spacer"></span>
    <button class="sp-btn" onclick={togglePlain} title="Toggle plain text">
      <Type size={12} />
      {plain ? 'Rich' : 'Plain'}
    </button>
  </div>
  {#if plain}
    <textarea class="body-text" bind:value={textBody} oninput={syncFromText} {placeholder}
    ></textarea>
  {:else}
    <div
      class="body-html"
      contenteditable="true"
      bind:this={bodyEl}
      oninput={onInput}
      role="textbox"
      aria-multiline="true"
      aria-label="Reply body"
      tabindex="0"
    ></div>
  {/if}
</div>

<style>
  .rich {
    border: 1px solid var(--sp-border-hard);
    border-radius: var(--sp-r-control);
    background: var(--sp-surface-well);
    box-shadow: var(--sp-well);
    overflow: hidden;
  }
  .mini-toolbar {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    padding: var(--sp-1);
    border-bottom: 1px solid var(--sp-border-hard);
    background: var(--sp-surface-sunken);
  }
  .spacer {
    flex: 1;
  }
  .body-html,
  .body-text {
    display: block;
    width: 100%;
    min-height: 5.5rem;
    max-height: 12rem;
    overflow-y: auto;
    padding: var(--sp-2);
    font: 400 var(--sp-fs-small) / var(--sp-lh-ui) var(--sp-font-ui);
    color: var(--sp-text-primary);
    background: transparent;
    border: 0;
    resize: vertical;
  }
  .body-html:focus,
  .body-text:focus {
    outline: none;
  }
</style>
