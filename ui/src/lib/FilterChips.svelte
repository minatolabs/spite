<script lang="ts">
  import { Flag, Mail as MailIcon, Paperclip } from 'lucide-svelte'
  import { mail, runSearch } from './mail.svelte'

  function toggle(key: 'unread_only' | 'has_attachments') {
    mail.chips[key] = !mail.chips[key]
    void runSearch()
  }

  function setDays(days: number) {
    mail.chips.days = mail.chips.days === days ? 0 : days
    void runSearch()
  }

  let fromDebounce: ReturnType<typeof setTimeout> | undefined
  function onFromInput() {
    clearTimeout(fromDebounce)
    fromDebounce = setTimeout(() => void runSearch(), 200)
  }
</script>

<div class="chips">
  <button
    class="chip"
    class:on={mail.chips.unread_only}
    onclick={() => toggle('unread_only')}
  >
    <MailIcon size={11} /> Unread
  </button>
  <button
    class="chip"
    class:on={mail.chips.has_attachments}
    onclick={() => toggle('has_attachments')}
  >
    <Paperclip size={11} /> Attachment
  </button>
  <button class="chip" class:on={mail.chips.days === 7} onclick={() => setDays(7)}>7d</button>
  <button class="chip" class:on={mail.chips.days === 30} onclick={() => setDays(30)}>30d</button>
  <input
    class="chip from"
    placeholder="from…"
    bind:value={mail.chips.from}
    oninput={onFromInput}
  />
  <button
    class="chip disabled"
    title="Flag data isn't synced yet — flags arrive with the mail-management phase"
    disabled
  >
    <Flag size={11} /> Flagged
  </button>
</div>

<style>
  .chips {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    padding: var(--sp-2) var(--sp-3);
    border-bottom: 1px solid var(--sp-border-hard);
    background: var(--sp-surface-sunken);
    flex-wrap: wrap;
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px var(--sp-2);
    font: 500 var(--sp-fs-caption) / 1.4 var(--sp-font-ui);
    color: var(--sp-text-secondary);
    background: var(--ink-700);
    border: 1px solid var(--sp-border-control);
    border-radius: var(--sp-r-pill);
    cursor: pointer;
  }

  .chip:hover:not(.disabled) {
    color: var(--sp-text-primary);
  }

  .chip.on {
    color: var(--sp-text-accent);
    background: rgba(138, 43, 49, 0.18);
    border-color: rgba(138, 43, 49, 0.5);
  }

  .chip.disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .chip.from {
    width: 110px;
    background: var(--sp-surface-well);
    cursor: text;
    color: var(--sp-text-primary);
    outline: none;
  }

  .chip.from::placeholder {
    color: var(--sp-text-placeholder);
  }
</style>
