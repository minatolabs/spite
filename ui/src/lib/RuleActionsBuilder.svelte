<script lang="ts">
  import { Plus, X } from 'lucide-svelte'
  import FolderPicker from './FolderPicker.svelte'
  import RecipientsField from './RecipientsField.svelte'
  import { mail } from './mail.svelte'
  import { settings, ensureCategories, presetCssVar } from './settings.svelte'
  import type { RuleActions, RuleRecipient } from './rules.svelte'

  // Builder for a messageRuleActions object. Same round-trip invariant as
  // the predicates builder: shallow-spread mutations only, unknown keys
  // survive verbatim and are surfaced, never hidden.
  let {
    actions,
    onchange,
  }: {
    actions: RuleActions | null | undefined
    onchange: (a: RuleActions) => void
  } = $props()

  type Kind = 'folder' | 'flag' | 'recipients' | 'categories' | 'select'
  type Def = {
    key: string
    label: string
    kind: Kind
    danger?: boolean
    external?: boolean
    note?: string
    options?: string[]
  }

  const DEFS: Def[] = [
    { key: 'moveToFolder', label: 'Move to folder', kind: 'folder' },
    { key: 'copyToFolder', label: 'Copy to folder', kind: 'folder' },
    {
      key: 'delete',
      label: 'Delete (moves to Deleted Items)',
      kind: 'flag',
      note: 'Recoverable from Deleted Items.',
    },
    {
      key: 'permanentDelete',
      label: 'Permanently delete',
      kind: 'flag',
      danger: true,
      note: 'Skips Deleted Items — matching mail is unrecoverable.',
    },
    {
      key: 'forwardTo',
      label: 'Forward to',
      kind: 'recipients',
      external: true,
      note: 'Recipients receive a copy of every matching message.',
    },
    {
      key: 'forwardAsAttachmentTo',
      label: 'Forward as attachment to',
      kind: 'recipients',
      external: true,
      note: 'Recipients receive every matching message as an attachment.',
    },
    {
      key: 'redirectTo',
      label: 'Redirect to',
      kind: 'recipients',
      external: true,
      note: 'Matching mail is redirected — it arrives as if sent to them.',
    },
    { key: 'assignCategories', label: 'Assign categories', kind: 'categories' },
    { key: 'markAsRead', label: 'Mark as read', kind: 'flag' },
    {
      key: 'markImportance',
      label: 'Mark importance',
      kind: 'select',
      options: ['low', 'normal', 'high'],
    },
    { key: 'stopProcessingRules', label: 'Stop processing more rules', kind: 'flag' },
  ]
  const byKey = new Map(DEFS.map((d) => [d.key, d]))

  let addOpen = $state(false)
  let pickingFolderFor = $state<string | null>(null)

  const current = $derived(actions ?? {})
  const activeKeys = $derived(
    Object.keys(current).filter((k) => current[k] !== null && current[k] !== undefined),
  )
  const activeDefs = $derived(activeKeys.filter((k) => byKey.has(k)))
  const unknownCount = $derived(activeKeys.length - activeDefs.length)
  const addable = $derived(DEFS.filter((d) => !activeKeys.includes(d.key)))

  function set(key: string, value: unknown) {
    onchange({ ...current, [key]: value })
  }

  function remove(key: string) {
    const next = { ...current }
    delete next[key]
    onchange(next)
  }

  function add(def: Def) {
    addOpen = false
    void ensureCategories()
    switch (def.kind) {
      case 'folder':
        pickingFolderFor = def.key
        break
      case 'flag':
        set(def.key, true)
        break
      case 'recipients':
        set(def.key, [])
        break
      case 'categories':
        set(def.key, [])
        break
      case 'select':
        set(def.key, def.options?.[0] ?? '')
        break
    }
  }

  function folderName(id: string): string {
    const f = mail.folders.find((f) => f.id === id)
    return f?.display_name ?? 'a folder'
  }

  function toggleCategory(key: string, name: string) {
    const list = (current[key] as string[]) ?? []
    set(key, list.includes(name) ? list.filter((c) => c !== name) : [...list, name])
  }
</script>

<div class="builder">
  {#each activeDefs as key (key)}
    {@const def = byKey.get(key)!}
    <div class="row" class:danger={def.danger}>
      <span class="row-label">{def.label}</span>
      <div class="row-input">
        {#if def.kind === 'folder'}
          <button class="sp-btn" onclick={() => (pickingFolderFor = key)}>
            {folderName(String(current[key]))}
          </button>
        {:else if def.kind === 'flag'}
          <span class="on">on</span>
        {:else if def.kind === 'recipients'}
          <RecipientsField
            label=""
            recipients={(current[key] as RuleRecipient[]) ?? []}
            warnExternal={def.external}
            onchange={(r) => set(key, r)}
          />
        {:else if def.kind === 'categories'}
          <span class="cat-toggles">
            {#each settings.categories as cat (cat.id)}
              <button
                class="cat-toggle"
                class:on={((current[key] as string[]) ?? []).includes(cat.displayName)}
                onclick={() => toggleCategory(key, cat.displayName)}
              >
                <span class="dot" style="background: var({presetCssVar(cat.color)})"></span>
                {cat.displayName}
              </button>
            {/each}
          </span>
        {:else if def.kind === 'select'}
          <select
            value={String(current[key] ?? '')}
            onchange={(e) => set(key, e.currentTarget.value)}
          >
            {#each def.options ?? [] as opt (opt)}
              <option value={opt}>{opt}</option>
            {/each}
          </select>
        {/if}
        {#if def.note}
          <p class="note" class:danger-note={def.danger}>{def.note}</p>
        {/if}
      </div>
      <button class="icon-btn" title="Remove action" onclick={() => remove(key)}>
        <X size={12} />
      </button>
    </div>
  {/each}

  {#if unknownCount > 0}
    <p class="preserved">
      ⚠ {unknownCount} unrecognized action(s) from another client — preserved on save
    </p>
  {/if}

  <div class="add">
    <button class="sp-btn" onclick={() => (addOpen = !addOpen)}>
      <Plus size={12} /> Add action
    </button>
    {#if addOpen}
      <div class="add-menu">
        {#each addable as def (def.key)}
          <button class="add-opt" class:danger-opt={def.danger} onclick={() => add(def)}>
            {def.label}
          </button>
        {/each}
      </div>
    {/if}
  </div>
</div>

{#if pickingFolderFor}
  <FolderPicker
    onpick={(folderId) => {
      set(pickingFolderFor!, folderId)
      pickingFolderFor = null
    }}
    onclose={() => {
      // Cancelled before choosing: don't leave a folder action without a
      // folder (an empty string would be an invalid rule).
      if (pickingFolderFor && !current[pickingFolderFor]) remove(pickingFolderFor)
      pickingFolderFor = null
    }}
  />
{/if}

<style>
  .builder {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
  }

  .row {
    display: flex;
    align-items: flex-start;
    gap: var(--sp-2);
    padding: var(--sp-1) 0;
    border-bottom: var(--sp-stitch);
  }

  .row.danger .row-label {
    color: var(--sp-danger);
  }

  .row-label {
    flex: none;
    width: 190px;
    padding-top: 4px;
    font-size: var(--sp-fs-caption);
    color: var(--sp-text-muted);
  }

  .row-input {
    flex: 1;
    min-width: 0;
  }

  .on {
    font-size: var(--sp-fs-small);
    color: var(--sp-text-secondary);
  }

  select {
    padding: 3px var(--sp-2);
    font: 400 var(--sp-fs-small) / var(--sp-lh-ui) var(--sp-font-ui);
    color: var(--sp-text-primary);
    background: var(--sp-surface-well);
    border: 1px solid var(--sp-border-hard);
    border-radius: var(--sp-r-control);
  }

  .note {
    margin: 2px 0 0;
    font-size: var(--sp-fs-caption);
    color: var(--sp-text-tertiary);
  }

  .danger-note {
    color: var(--sp-danger);
  }

  .cat-toggles {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-1);
  }

  .cat-toggle {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px var(--sp-2);
    font-size: var(--sp-fs-caption);
    color: var(--sp-text-secondary);
    background: var(--sp-surface-well);
    border: 1px solid var(--sp-border-control);
    border-radius: var(--sp-r-pill);
    cursor: pointer;
  }

  .cat-toggle.on {
    color: var(--sp-text-display);
    border-color: var(--sp-flag);
  }

  .dot {
    width: 8px;
    height: 8px;
    border-radius: 2px;
    flex: none;
  }

  .icon-btn {
    background: none;
    border: 0;
    color: var(--sp-text-tertiary);
    cursor: pointer;
    padding: 4px 2px;
  }

  .icon-btn:hover {
    color: var(--sp-danger);
  }

  .preserved {
    margin: var(--sp-1) 0;
    font-size: var(--sp-fs-caption);
    color: var(--sp-flag);
  }

  .add {
    position: relative;
    margin-top: var(--sp-1);
  }

  .add-menu {
    position: absolute;
    bottom: 100%;
    left: 0;
    z-index: var(--sp-z-popover, 200);
    margin-bottom: 3px;
    min-width: 240px;
    max-height: 300px;
    overflow-y: auto;
    padding: var(--sp-1);
    background: var(--sp-surface-raised);
    border: 1px solid var(--sp-border-hard);
    border-radius: var(--sp-r-control);
    box-shadow: var(--sp-lift);
    display: flex;
    flex-direction: column;
  }

  .add-opt {
    padding: 3px var(--sp-2);
    border: none;
    background: none;
    color: var(--sp-text-primary);
    font: inherit;
    font-size: var(--sp-fs-small);
    text-align: left;
    cursor: pointer;
    border-radius: var(--sp-r-control);
  }

  .add-opt:hover {
    background: var(--sp-surface-well);
  }

  .danger-opt {
    color: var(--sp-danger);
  }
</style>
