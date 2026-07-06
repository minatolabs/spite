<script lang="ts">
  import { onMount } from 'svelte'
  import { Bookmark, Globe, Search, X } from 'lucide-svelte'
  import {
    applySavedSearch,
    clearSearch,
    deleteSavedSearch,
    loadSavedSearches,
    mail,
    runSearch,
    saveCurrentSearch,
    searchActive,
    searchEverywhere,
  } from './mail.svelte'

  let debounce: ReturnType<typeof setTimeout> | undefined
  let showSaved = $state(false)

  function onInput() {
    clearTimeout(debounce)
    debounce = setTimeout(() => void runSearch(), 150)
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      clearSearch()
      ;(e.target as HTMLInputElement).blur()
    } else if (e.key === 'Enter' && e.ctrlKey) {
      void searchEverywhere()
    }
  }

  async function saveCurrent() {
    const name = prompt('Name this search:')
    if (name?.trim()) await saveCurrentSearch(name.trim())
  }

  onMount(() => {
    void loadSavedSearches()
  })
</script>

<div class="searchbar">
  <div class="box sp-field">
    <Search size={13} />
    <input
      id="search-input"
      placeholder="Search mail…  ( / )"
      bind:value={mail.query}
      oninput={onInput}
      onkeydown={onKeydown}
    />
    {#if searchActive()}
      <button class="icon" onclick={clearSearch} title="Clear (Esc)"><X size={13} /></button>
    {/if}
  </div>

  <button
    class="sp-btn"
    class:active={mail.scopeAll}
    onclick={() => {
      mail.scopeAll = !mail.scopeAll
      void runSearch()
    }}
    title="Toggle scope: current folder vs all mail"
  >
    {mail.scopeAll ? 'All mail' : 'This folder'}
  </button>

  <button
    class="sp-btn"
    onclick={() => void searchEverywhere()}
    disabled={mail.serverSearching || !mail.query.trim()}
    title="Deep search on the server (whole mailbox)"
  >
    <Globe size={13} />
    {mail.serverSearching ? 'Searching…' : 'Everywhere'}
  </button>

  <div class="saved">
    <button class="sp-btn" onclick={() => (showSaved = !showSaved)} title="Saved searches">
      <Bookmark size={13} />
    </button>
    {#if showSaved}
      <div class="saved-menu">
        {#each mail.savedSearches as saved (saved.id)}
          <div class="saved-row">
            <button
              class="apply"
              onclick={() => {
                showSaved = false
                void applySavedSearch(saved)
              }}
            >
              {saved.name}
            </button>
            <button
              class="icon"
              onclick={() => void deleteSavedSearch(saved.id)}
              title="Delete saved search"
            >
              <X size={12} />
            </button>
          </div>
        {:else}
          <p class="empty">No saved searches yet.</p>
        {/each}
        {#if searchActive()}
          <button class="sp-btn save-current" onclick={saveCurrent}>Save current search</button>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .searchbar {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
  }

  .box {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    width: 280px;
    color: var(--sp-text-tertiary);
  }

  .box input {
    flex: 1;
    border: none;
    background: transparent;
    color: var(--sp-text-primary);
    font: inherit;
    font-size: var(--sp-fs-small);
    outline: none;
    min-width: 0;
  }

  .box input::placeholder {
    color: var(--sp-text-placeholder);
  }

  .icon {
    border: none;
    background: none;
    color: var(--sp-text-tertiary);
    cursor: pointer;
    padding: 0;
    display: inline-flex;
  }

  .icon:hover {
    color: var(--sp-text-primary);
  }

  .sp-btn.active {
    box-shadow: var(--sp-bevel-pressed);
    color: var(--sp-text-accent);
  }

  .saved {
    position: relative;
  }

  .saved-menu {
    position: absolute;
    top: calc(100% + 4px);
    right: 0;
    z-index: var(--sp-z-dropdown);
    min-width: 220px;
    padding: var(--sp-2);
    background: var(--ink-700);
    border: 1px solid var(--sp-border-hard);
    border-radius: var(--sp-r-control);
    box-shadow: var(--sp-lift);
  }

  .saved-row {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
  }

  .apply {
    flex: 1;
    text-align: left;
    border: none;
    background: none;
    color: var(--sp-text-primary);
    font: inherit;
    font-size: var(--sp-fs-small);
    padding: 5px var(--sp-2);
    border-radius: 4px;
    cursor: pointer;
  }

  .apply:hover {
    background: var(--sp-selected-fill);
  }

  .empty {
    margin: 0;
    padding: var(--sp-2);
    font-size: var(--sp-fs-caption);
    color: var(--sp-text-muted);
  }

  .save-current {
    width: 100%;
    margin-top: var(--sp-2);
    justify-content: center;
  }
</style>
