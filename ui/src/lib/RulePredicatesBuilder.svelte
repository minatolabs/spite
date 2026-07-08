<script lang="ts">
  import { Plus, X } from 'lucide-svelte'
  import RecipientsField from './RecipientsField.svelte'
  import StringChips from './StringChips.svelte'
  import { settings, ensureCategories, presetCssVar } from './settings.svelte'
  import type { RulePredicates, RuleRecipient } from './rules.svelte'

  // Builder for a messageRulePredicates object (conditions or exceptions).
  // ROUND-TRIP INVARIANT: this never reconstructs the object from form
  // state — every mutation is a shallow spread of the existing object with
  // one key set or removed, so keys the builder doesn't know about (and
  // nested unknown fields) survive verbatim.
  let {
    predicates,
    onchange,
    kindLabel = 'condition',
  }: {
    predicates: RulePredicates | null | undefined
    onchange: (p: RulePredicates) => void
    kindLabel?: string
  } = $props()

  type Kind = 'recipients' | 'strings' | 'bool' | 'select' | 'size' | 'categories'
  type Def = { key: string; label: string; group: string; kind: Kind; options?: string[] }

  const DEFS: Def[] = [
    // People
    { key: 'fromAddresses', label: 'From', group: 'People', kind: 'recipients' },
    { key: 'sentToAddresses', label: 'Sent to', group: 'People', kind: 'recipients' },
    { key: 'sentToMe', label: 'Sent to me', group: 'People', kind: 'bool' },
    { key: 'sentOnlyToMe', label: 'Sent only to me', group: 'People', kind: 'bool' },
    { key: 'sentCcMe', label: "I'm Cc'd", group: 'People', kind: 'bool' },
    { key: 'sentToOrCcMe', label: 'Sent to or Cc me', group: 'People', kind: 'bool' },
    { key: 'notSentToMe', label: 'Not sent to me', group: 'People', kind: 'bool' },
    { key: 'recipientContains', label: 'Recipient contains', group: 'People', kind: 'strings' },
    { key: 'senderContains', label: 'Sender contains', group: 'People', kind: 'strings' },
    // Content
    { key: 'subjectContains', label: 'Subject contains', group: 'Content', kind: 'strings' },
    { key: 'bodyContains', label: 'Body contains', group: 'Content', kind: 'strings' },
    {
      key: 'bodyOrSubjectContains',
      label: 'Subject or body contains',
      group: 'Content',
      kind: 'strings',
    },
    { key: 'headerContains', label: 'Header contains', group: 'Content', kind: 'strings' },
    // Properties
    {
      key: 'importance',
      label: 'Importance',
      group: 'Properties',
      kind: 'select',
      options: ['low', 'normal', 'high'],
    },
    {
      key: 'sensitivity',
      label: 'Sensitivity',
      group: 'Properties',
      kind: 'select',
      options: ['normal', 'personal', 'private', 'confidential'],
    },
    {
      key: 'messageActionFlag',
      label: 'Action flag',
      group: 'Properties',
      kind: 'select',
      options: [
        'any',
        'call',
        'copyWith',
        'delete',
        'followUp',
        'forward',
        'information',
        'learnMore',
        'mailForward',
        'mailMustBeRead',
        'mailRlS',
        'move',
        'openItem',
        'print',
        'redirect',
        'removeCopy',
        'send',
        'toArray',
      ],
    },
    { key: 'hasAttachments', label: 'Has attachments', group: 'Properties', kind: 'bool' },
    { key: 'withinSizeRange', label: 'Size (KB)', group: 'Properties', kind: 'size' },
    { key: 'categories', label: 'Categorized', group: 'Properties', kind: 'categories' },
    // Message type
    { key: 'isApprovalRequest', label: 'Approval request', group: 'Message type', kind: 'bool' },
    { key: 'isAutomaticForward', label: 'Automatic forward', group: 'Message type', kind: 'bool' },
    { key: 'isAutomaticReply', label: 'Automatic reply', group: 'Message type', kind: 'bool' },
    { key: 'isEncrypted', label: 'Encrypted', group: 'Message type', kind: 'bool' },
    { key: 'isMeetingRequest', label: 'Meeting request', group: 'Message type', kind: 'bool' },
    { key: 'isMeetingResponse', label: 'Meeting response', group: 'Message type', kind: 'bool' },
    {
      key: 'isNonDeliveryReport',
      label: 'Non-delivery report',
      group: 'Message type',
      kind: 'bool',
    },
    {
      key: 'isPermissionControlled',
      label: 'Permission-controlled',
      group: 'Message type',
      kind: 'bool',
    },
    { key: 'isReadReceipt', label: 'Read receipt', group: 'Message type', kind: 'bool' },
    { key: 'isSigned', label: 'Signed', group: 'Message type', kind: 'bool' },
    { key: 'isVoicemail', label: 'Voicemail', group: 'Message type', kind: 'bool' },
  ]
  const GROUPS = ['People', 'Content', 'Properties', 'Message type']
  const byKey = new Map(DEFS.map((d) => [d.key, d]))

  let addOpen = $state(false)

  const current = $derived(predicates ?? {})
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
      case 'recipients':
        set(def.key, [])
        break
      case 'strings':
        set(def.key, [])
        break
      case 'bool':
        set(def.key, true)
        break
      case 'select':
        set(def.key, def.options?.[0] ?? '')
        break
      case 'size':
        set(def.key, { minimumSize: 0, maximumSize: 1024 })
        break
      case 'categories':
        set(def.key, [])
        break
    }
  }

  function toggleCategory(key: string, name: string) {
    const list = (current[key] as string[]) ?? []
    set(key, list.includes(name) ? list.filter((c) => c !== name) : [...list, name])
  }

  function sizeOf(key: string): { minimumSize?: number; maximumSize?: number } {
    return (current[key] as { minimumSize?: number; maximumSize?: number }) ?? {}
  }

  function setSize(key: string, part: 'minimumSize' | 'maximumSize', raw: string) {
    const n = Number(raw)
    // Spread the existing object — unknown sizeRange fields survive.
    set(key, { ...sizeOf(key), [part]: Number.isFinite(n) ? n : 0 })
  }
</script>

<div class="builder">
  {#each activeDefs as key (key)}
    {@const def = byKey.get(key)!}
    <div class="row">
      <span class="row-label">{def.label}</span>
      <div class="row-input">
        {#if def.kind === 'recipients'}
          <RecipientsField
            label=""
            recipients={(current[key] as RuleRecipient[]) ?? []}
            onchange={(r) => set(key, r)}
          />
        {:else if def.kind === 'strings'}
          <StringChips values={(current[key] as string[]) ?? []} onchange={(v) => set(key, v)} />
        {:else if def.kind === 'bool'}
          <select
            value={current[key] === false ? 'no' : 'yes'}
            onchange={(e) => set(key, e.currentTarget.value === 'yes')}
          >
            <option value="yes">yes</option>
            <option value="no">no</option>
          </select>
        {:else if def.kind === 'select'}
          <select
            value={String(current[key] ?? '')}
            onchange={(e) => set(key, e.currentTarget.value)}
          >
            {#each def.options ?? [] as opt (opt)}
              <option value={opt}>{opt}</option>
            {/each}
          </select>
        {:else if def.kind === 'size'}
          <span class="size">
            <input
              type="number"
              min="0"
              value={sizeOf(key).minimumSize ?? 0}
              onchange={(e) => setSize(key, 'minimumSize', e.currentTarget.value)}
            />
            –
            <input
              type="number"
              min="0"
              value={sizeOf(key).maximumSize ?? 0}
              onchange={(e) => setSize(key, 'maximumSize', e.currentTarget.value)}
            />
            KB
          </span>
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
            {#if settings.categories.length === 0}
              <StringChips
                values={(current[key] as string[]) ?? []}
                onchange={(v) => set(key, v)}
                placeholder="category…"
              />
            {/if}
          </span>
        {/if}
      </div>
      <button class="icon-btn" title="Remove {kindLabel}" onclick={() => remove(key)}>
        <X size={12} />
      </button>
    </div>
  {/each}

  {#if unknownCount > 0}
    <p class="preserved">
      {unknownCount} unrecognized {kindLabel}(s) from another client — preserved on save
    </p>
  {/if}

  <div class="add">
    <button class="sp-btn" onclick={() => (addOpen = !addOpen)}>
      <Plus size={12} /> Add {kindLabel}
    </button>
    {#if addOpen}
      <div class="add-menu">
        {#each GROUPS as group (group)}
          {@const defs = addable.filter((d) => d.group === group)}
          {#if defs.length}
            <span class="group">{group}</span>
            {#each defs as def (def.key)}
              <button class="add-opt" onclick={() => add(def)}>{def.label}</button>
            {/each}
          {/if}
        {/each}
      </div>
    {/if}
  </div>
</div>

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

  .row-label {
    flex: none;
    width: 150px;
    padding-top: 4px;
    font-size: var(--sp-fs-caption);
    color: var(--sp-text-muted);
  }

  .row-input {
    flex: 1;
    min-width: 0;
  }

  select,
  input[type='number'] {
    padding: 3px var(--sp-2);
    font: 400 var(--sp-fs-small) / var(--sp-lh-ui) var(--sp-font-ui);
    color: var(--sp-text-primary);
    background: var(--sp-surface-well);
    border: 1px solid var(--sp-border-hard);
    border-radius: var(--sp-r-control);
  }

  input[type='number'] {
    width: 80px;
  }

  .size {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    font-size: var(--sp-fs-small);
    color: var(--sp-text-secondary);
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
    min-width: 220px;
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

  .group {
    padding: var(--sp-1) var(--sp-2) 2px;
    font-size: var(--sp-fs-caption);
    text-transform: uppercase;
    letter-spacing: var(--sp-track-label);
    color: var(--sp-text-muted);
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
</style>
