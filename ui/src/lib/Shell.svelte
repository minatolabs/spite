<script lang="ts">
  import { onMount } from 'svelte'
  import { getCurrentWindow } from '@tauri-apps/api/window'
  import { LogOut, RefreshCw } from 'lucide-svelte'
  import FolderTree from './FolderTree.svelte'
  import MessageList from './MessageList.svelte'
  import ReadingPane from './ReadingPane.svelte'
  import StatusBar from './StatusBar.svelte'
  import { initMail, mail, selectedFolder, syncNow } from './mail.svelte'

  type Account = { upn: string; display_name: string }
  let { account, onsignout }: { account: Account; onsignout: () => void } = $props()

  onMount(() => {
    void initMail()
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

<div class="shell">
  <header class="toolbar">
    <span class="wordmark">SPITE</span>
    <span class="folder-name">{selectedFolder()?.display_name ?? ''}</span>
    <span class="spacer"></span>
    <button
      class="sp-btn"
      onclick={() => void syncNow()}
      disabled={mail.syncing}
      title="Sync now"
    >
      <RefreshCw size={13} />
      {mail.syncing ? 'Syncing…' : 'Sync'}
    </button>
    <span class="account" title={account.upn}>{account.display_name}</span>
    <button class="sp-btn" onclick={onsignout} title="Sign out">
      <LogOut size={13} />
    </button>
  </header>

  <div class="panes">
    <aside class="folders sp-scroll">
      <FolderTree />
    </aside>
    <section class="list sp-scroll">
      <MessageList />
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

  .list {
    background: var(--sp-surface-sunken);
    border-right: 1px solid var(--sp-border-hard);
    box-shadow: var(--sp-seam-right);
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
