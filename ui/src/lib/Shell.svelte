<script lang="ts">
  import { onMount } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'
  import { getCurrentWindow } from '@tauri-apps/api/window'
  import { LogOut, Mailbox, PenLine, RefreshCw, Settings2 } from 'lucide-svelte'
  import BulkBar from './BulkBar.svelte'
  import FilterChips from './FilterChips.svelte'
  import FolderTree from './FolderTree.svelte'
  import MailContextMenu from './MailContextMenu.svelte'
  import MessageList from './MessageList.svelte'
  import ReadingPane from './ReadingPane.svelte'
  import SearchBar from './SearchBar.svelte'
  import SendToasts from './SendToasts.svelte'
  import SettingsPane from './SettingsPane.svelte'
  import SignatureSettings from './SignatureSettings.svelte'
  import StatusBar from './StatusBar.svelte'
  import {
    archive,
    clearSearch,
    clearSelection,
    initMail,
    mail,
    searchActive,
    selectAll,
    selectedFolder,
    softDelete,
    syncNow,
    toggleFlag,
  } from './mail.svelte'

  type Account = { upn: string; display_name: string }
  let { account, onsignout }: { account: Account; onsignout: () => void } = $props()

  let showSignatures = $state(false)
  let showSettings = $state(false)

  function composeNew() {
    void invoke('open_compose', { mode: 'new', messageId: null })
  }

  // Vim-flavored, read-only this phase; write verbs are stubs until the
  // mail-management phase. Overridable via config.json { "keymap": {...} }.
  let keymap = $state<Record<string, string>>({
    focusSearch: '/',
    next: 'j',
    prev: 'k',
    open: 'Enter',
    clear: 'Escape',
    reply: 'r',
    compose: 'c',
    archive: 'e',
    delete: '#',
    flag: 's',
  })

  function visibleIds(): string[] {
    if (searchActive()) {
      return [
        ...mail.hits.filter((h) => h.summary).map((h) => h.entity_id),
        ...mail.serverHits.map((h) => h.summary.id),
      ]
    }
    return mail.messages.map((m) => m.id)
  }

  function moveSelection(delta: number) {
    const ids = visibleIds()
    if (!ids.length) return
    const at = mail.selectedId ? ids.indexOf(mail.selectedId) : -1
    const next = ids[Math.min(Math.max(at + delta, 0), ids.length - 1)]
    const server = mail.serverHits.find((h) => h.summary.id === next)
    mail.serverSelected = server ? server.summary : null
    mail.selectedId = next
  }

  function isTyping(target: EventTarget | null): boolean {
    const el = target as HTMLElement | null
    return (
      !!el &&
      (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable)
    )
  }

  function onGlobalKeydown(e: KeyboardEvent) {
    // Ctrl/Cmd+A selects all visible messages (outside inputs).
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'a' && !isTyping(e.target)) {
      e.preventDefault()
      selectAll()
      return
    }
    if (e.ctrlKey || e.metaKey || e.altKey || isTyping(e.target)) return
    const key = e.key
    // Esc clears an active multi-selection first.
    if (key === 'Escape' && mail.selection.size > 0) {
      e.preventDefault()
      clearSelection()
      return
    }
    if (key === keymap.focusSearch) {
      e.preventDefault()
      document.getElementById('search-input')?.focus()
    } else if (key === keymap.next) {
      e.preventDefault()
      moveSelection(1)
    } else if (key === keymap.prev) {
      e.preventDefault()
      moveSelection(-1)
    } else if (key === keymap.open) {
      // Selection already opens in the reading pane; Enter is a no-op
      // confirm so the muscle memory works.
      if (mail.selectedId) e.preventDefault()
    } else if (key === keymap.clear) {
      if (searchActive()) {
        e.preventDefault()
        clearSearch()
      }
    } else if (key === keymap.reply) {
      if (mail.selectedId) {
        e.preventDefault()
        void invoke('open_compose', { mode: 'reply', messageId: mail.selectedId })
      }
    } else if (key === keymap.compose) {
      e.preventDefault()
      composeNew()
    } else if (key === keymap.archive) {
      if (mail.selectedId) {
        e.preventDefault()
        void archive(mail.selectedId)
      }
    } else if (key === keymap.delete) {
      if (mail.selectedId) {
        e.preventDefault()
        void softDelete(mail.selectedId)
      }
    } else if (key === keymap.flag) {
      const m = mail.messages.find((x) => x.id === mail.selectedId)
      if (m) {
        e.preventDefault()
        toggleFlag(m)
      }
    }
  }

  onMount(() => {
    void initMail()
    invoke<Record<string, string>>('get_keymap')
      .then((overrides) => (keymap = { ...keymap, ...overrides }))
      .catch(() => {})
    // UI-driven sync (replaces the Phase 3 startup trigger): folder open
    // (in selectFolder), window focus, and a 60s interval.
    const interval = setInterval(() => void syncNow(), 60_000)
    let unlisten: (() => void) | undefined
    getCurrentWindow()
      .onFocusChanged(({ payload: focused }) => {
        if (focused) void syncNow()
      })
      .then((fn) => (unlisten = fn))
    return () => {
      clearInterval(interval)
      unlisten?.()
    }
  })
</script>

<svelte:window onkeydown={onGlobalKeydown} />

<div class="shell">
  <header class="toolbar">
    <span class="wordmark">SPITE</span>
    <button class="sp-btn sp-btn--primary" onclick={composeNew}>
      <PenLine size={13} /> Compose
    </button>
    <span class="folder-name">{selectedFolder()?.display_name ?? ''}</span>
    <span class="spacer"></span>
    <SearchBar />
    <button
      class="sp-btn"
      onclick={() => void syncNow()}
      disabled={mail.syncing}
      title="Sync now"
    >
      <RefreshCw size={13} />
      {mail.syncing ? 'Syncing…' : 'Sync'}
    </button>
    <button class="sp-btn" onclick={() => (showSettings = true)} title="Mailbox settings">
      <Mailbox size={13} />
    </button>
    <button class="sp-btn" onclick={() => (showSignatures = true)} title="Signatures">
      <Settings2 size={13} />
    </button>
    <span class="account" title={account.upn}>{account.display_name}</span>
    <button class="sp-btn" onclick={onsignout} title="Sign out">
      <LogOut size={13} />
    </button>
  </header>

  {#if showSignatures}
    <SignatureSettings onclose={() => (showSignatures = false)} />
  {/if}

  {#if showSettings}
    <SettingsPane onclose={() => (showSettings = false)} />
  {/if}

  <SendToasts />
  <MailContextMenu />

  {#if mail.actionError}
    <div class="action-error" role="alert">
      <span>{mail.actionError}</span>
      <button onclick={() => (mail.actionError = '')} aria-label="Dismiss">×</button>
    </div>
  {/if}

  <div class="panes">
    <aside class="folders sp-scroll">
      <FolderTree />
    </aside>
    <section class="list-pane">
      {#if mail.selection.size > 0}
        <BulkBar />
      {:else}
        <FilterChips />
      {/if}
      <div class="list sp-scroll">
        <MessageList />
      </div>
    </section>
    <section class="reading">
      <ReadingPane />
    </section>
  </div>

  <StatusBar />
</div>

<style>
  .shell {
    height: 100svh;
    display: grid;
    grid-template-rows: var(--sp-h-toolbar) 1fr var(--sp-h-statusbar);
  }

  .action-error {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-2) var(--sp-4);
    font-size: var(--sp-fs-small);
    color: var(--sp-text-on-accent);
    background: var(--sp-danger);
    border-bottom: 1px solid var(--sp-border-hard);
  }

  .action-error span {
    flex: 1;
    overflow-wrap: anywhere;
  }

  .action-error button {
    border: none;
    background: none;
    color: inherit;
    font-size: var(--sp-fs-title);
    line-height: 1;
    cursor: pointer;
    padding: 0 var(--sp-1);
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: 0 var(--sp-3);
    background: var(--sp-surface-chrome);
    border-bottom: 1px solid var(--sp-border-hard);
    box-shadow: var(--sp-seam-top);
  }

  .wordmark {
    font-size: var(--sp-fs-small);
    font-weight: 600;
    letter-spacing: var(--sp-track-wordmark);
    color: var(--sp-text-accent);
  }

  .folder-name {
    font-size: var(--sp-fs-md);
    color: var(--sp-text-display);
  }

  .spacer {
    flex: 1;
  }

  .account {
    font-size: var(--sp-fs-small);
    color: var(--sp-text-secondary);
  }

  .panes {
    display: grid;
    grid-template-columns: var(--sp-w-folders) var(--sp-w-list) 1fr;
    min-height: 0;
  }

  .folders {
    background: var(--sp-surface-sunken);
    border-right: 1px solid var(--sp-border-hard);
    box-shadow: var(--sp-seam-right);
    overflow-y: auto;
    padding: var(--sp-3) 0;
  }

  .list-pane {
    display: flex;
    flex-direction: column;
    background: var(--sp-surface-sunken);
    border-right: 1px solid var(--sp-border-hard);
    box-shadow: var(--sp-seam-right);
    min-height: 0;
  }

  .list {
    flex: 1;
    overflow-y: auto;
    min-height: 0;
  }

  .reading {
    background: var(--sp-surface-raised);
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
</style>
