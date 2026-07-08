<script lang="ts">
  import { onMount } from 'svelte'
  import { AlertTriangle, Copy, GripVertical, Lock, Pencil, Plus, Trash2 } from 'lucide-svelte'
  import RuleActionsBuilder from './RuleActionsBuilder.svelte'
  import RulePredicatesBuilder from './RulePredicatesBuilder.svelte'
  import { mail } from './mail.svelte'
  import {
    rules,
    loadRules,
    toggleRule,
    removeRule,
    reorderRules,
    saveRule,
    duplicateRule,
    ruleSummary,
    externalForwardAddresses,
    type MessageRule,
  } from './rules.svelte'

  let { onclose }: { onclose: () => void } = $props()

  onMount(() => {
    void loadRules()
  })

  function folderName(id: string): string {
    return mail.folders.find((f) => f.id === id)?.display_name ?? 'a folder'
  }

  // --- Editor state. `editing` is a deep clone ($state.snapshot unwraps the
  // proxy) so Cancel never mutates the list; the builders mutate this one
  // object in place via shallow-spread onchange handlers, which preserves
  // every key the UI doesn't render (the round-trip invariant).
  let editing = $state<MessageRule | null>(null)
  let editorError = $state('')
  let saving = $state(false)
  /// Non-empty = the save-time external-forward confirm is showing.
  let confirmExternal = $state<string[]>([])

  function openEditor(rule?: MessageRule) {
    editorError = ''
    confirmExternal = []
    editing = rule
      ? (structuredClone($state.snapshot(rule)) as MessageRule)
      : { displayName: '', isEnabled: true, conditions: {}, actions: {} }
  }

  const actionCount = $derived(
    editing
      ? Object.keys(editing.actions ?? {}).filter(
          (k) => (editing!.actions as Record<string, unknown>)[k] != null,
        ).length
      : 0,
  )

  async function commitSave() {
    if (!editing) return
    saving = true
    const err = await saveRule($state.snapshot(editing) as MessageRule)
    saving = false
    if (err) {
      editorError = err
    } else {
      editing = null
      confirmExternal = []
    }
  }

  function trySave() {
    if (!editing) return
    editorError = ''
    if (!editing.displayName?.trim()) {
      editorError = 'The rule needs a name.'
      return
    }
    if (actionCount === 0) {
      editorError = 'The rule needs at least one action.'
      return
    }
    // Anti-exfiltration friction: a rule that sends matching mail outside
    // the account's domain never saves without an explicit confirm listing
    // the exact external addresses.
    const external = externalForwardAddresses(editing)
    if (external.length) {
      confirmExternal = external
      return
    }
    void commitSave()
  }

  // --- Delete: two-step inline confirm (a rule can forward/delete mail).
  let confirmDeleteId = $state<string | null>(null)

  // --- Drag-to-reorder (existing DnD event idiom, insertion-line variant).
  let draggedId = $state<string | null>(null)
  let dropBeforeId = $state<string | null>(null)

  function onDrop() {
    if (!draggedId || !dropBeforeId || draggedId === dropBeforeId) {
      draggedId = null
      dropBeforeId = null
      return
    }
    const ids = rules.list.map((r) => r.id!).filter(Boolean)
    const from = ids.indexOf(draggedId)
    let to = ids.indexOf(dropBeforeId)
    if (from < 0 || to < 0) return
    ids.splice(from, 1)
    to = ids.indexOf(dropBeforeId) // recompute after removal
    ids.splice(to, 0, draggedId)
    draggedId = null
    dropBeforeId = null
    void reorderRules(ids)
  }
</script>

<div
  class="backdrop"
  onclick={onclose}
  onkeydown={(e) => e.key === 'Escape' && onclose()}
  role="presentation"
>
  <div
    class="modal sp-scroll"
    onclick={(e) => e.stopPropagation()}
    role="dialog"
    aria-label="Inbox rules"
    tabindex="-1"
    onkeydown={() => {}}
  >
    {#if !editing}
      <h2>Inbox rules</h2>
      <p class="note">
        Rules run on the server, in order — they fire even when Spite is closed. Drag to reorder.
      </p>

      {#if rules.error}<p class="error">{rules.error}</p>{/if}
      {#if rules.loading && !rules.loaded}<p class="note">Loading rules…</p>{/if}

      <ul class="rule-list">
        {#each rules.list as rule (rule.id)}
          <li
            class="rule"
            class:drop-before={dropBeforeId === rule.id && draggedId !== rule.id}
            class:disabled={!rule.isEnabled}
            draggable={!rule.isReadOnly}
            ondragstart={(e) => {
              if (rule.isReadOnly || !rule.id) return
              draggedId = rule.id
              e.dataTransfer?.setData('application/x-spite-rule', rule.id)
              if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'
            }}
            ondragover={(e) => {
              if (!draggedId) return
              e.preventDefault()
              dropBeforeId = rule.id ?? null
            }}
            ondragleave={() => {
              if (dropBeforeId === rule.id) dropBeforeId = null
            }}
            ondrop={(e) => {
              e.preventDefault()
              onDrop()
            }}
            ondragend={() => {
              draggedId = null
              dropBeforeId = null
            }}
          >
            <span class="grip" class:locked={rule.isReadOnly} title={rule.isReadOnly ? 'Managed rule — read-only' : 'Drag to reorder'}>
              {#if rule.isReadOnly}<Lock size={12} />{:else}<GripVertical size={12} />{/if}
            </span>
            <input
              type="checkbox"
              checked={rule.isEnabled}
              disabled={rule.isReadOnly}
              onchange={() => void toggleRule(rule)}
              title={rule.isEnabled ? 'Enabled' : 'Disabled'}
            />
            <div class="rule-body">
              <span class="rule-name">
                {rule.displayName || '(unnamed rule)'}
                {#if rule.hasError}
                  <span class="badge error-badge" title="The server reports this rule is in error">
                    <AlertTriangle size={10} /> error
                  </span>
                {/if}
              </span>
              <span class="summary">
                {#each ruleSummary(rule, folderName) as seg, i (i)}
                  <span class={seg.kind}>{seg.text}</span>
                {/each}
              </span>
            </div>
            <span class="rule-actions">
              {#if confirmDeleteId === rule.id}
                <button class="sp-btn danger-btn" onclick={() => { confirmDeleteId = null; void removeRule(rule.id!) }}>
                  Confirm delete
                </button>
                <button class="sp-btn" onclick={() => (confirmDeleteId = null)}>Keep</button>
              {:else}
                <button class="icon-btn" title="Edit" disabled={rule.isReadOnly} onclick={() => openEditor(rule)}>
                  <Pencil size={12} />
                </button>
                <button class="icon-btn" title="Duplicate (created disabled)" onclick={() => void duplicateRule(rule)}>
                  <Copy size={12} />
                </button>
                <button class="icon-btn" title="Delete" disabled={rule.isReadOnly} onclick={() => (confirmDeleteId = rule.id ?? null)}>
                  <Trash2 size={12} />
                </button>
              {/if}
            </span>
          </li>
        {/each}
        {#if rules.loaded && rules.list.length === 0}
          <li class="empty">No rules yet.</li>
        {/if}
      </ul>

      <div class="actions footer">
        <button class="sp-btn sp-btn--primary" onclick={() => openEditor()}>
          <Plus size={12} /> New rule
        </button>
        <button class="sp-btn" onclick={onclose}>Close</button>
      </div>
    {:else}
      <h2>{editing.id ? 'Edit rule' : 'New rule'}</h2>

      <label class="name-field">
        Name
        <input bind:value={editing.displayName} placeholder="Rule name" />
      </label>
      <label class="row">
        <input type="checkbox" bind:checked={editing.isEnabled} />
        Enabled
      </label>

      <section>
        <h3>Conditions</h3>
        <p class="note">All conditions must match (they're AND'd — Outlook semantics). No conditions = every incoming message.</p>
        <RulePredicatesBuilder
          predicates={editing.conditions}
          onchange={(p) => (editing!.conditions = p)}
        />
      </section>

      <section>
        <h3>Exceptions</h3>
        <p class="note">The rule doesn't fire if any exception matches.</p>
        <RulePredicatesBuilder
          predicates={editing.exceptions}
          onchange={(p) => (editing!.exceptions = p)}
          kindLabel="exception"
        />
      </section>

      <section>
        <h3>Actions</h3>
        <RuleActionsBuilder actions={editing.actions} onchange={(a) => (editing!.actions = a)} />
      </section>

      {#if editorError}<p class="error">{editorError}</p>{/if}

      {#if confirmExternal.length}
        <div class="external-confirm">
          <p>
            <AlertTriangle size={13} />
            This rule sends matching mail <strong>outside your organization</strong>:
          </p>
          <ul>
            {#each confirmExternal as addr (addr)}
              <li>{addr}</li>
            {/each}
          </ul>
          <div class="actions">
            <button class="sp-btn danger-btn" disabled={saving} onclick={() => void commitSave()}>
              {saving ? 'Saving…' : 'I understand — save rule'}
            </button>
            <button class="sp-btn" onclick={() => (confirmExternal = [])}>Back</button>
          </div>
        </div>
      {:else}
        <div class="actions footer">
          <button class="sp-btn sp-btn--primary" disabled={saving} onclick={trySave}>
            {saving ? 'Saving…' : 'Save rule'}
          </button>
          <button class="sp-btn" onclick={() => (editing = null)}>Cancel</button>
        </div>
      {/if}
    {/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: var(--sp-z-modal);
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .modal {
    width: min(720px, 94vw);
    max-height: 88vh;
    overflow-y: auto;
    padding: var(--sp-5);
    background: var(--sp-surface-raised);
    border: 1px solid var(--sp-border-hard);
    border-radius: var(--sp-r-panel);
    box-shadow: var(--sp-lift);
  }

  h2 {
    margin: 0 0 var(--sp-2);
    font-size: var(--sp-fs-title);
    color: var(--sp-text-display);
  }

  h3 {
    margin: 0 0 var(--sp-1);
    font-size: var(--sp-fs-small);
    text-transform: uppercase;
    letter-spacing: var(--sp-track-label);
    color: var(--sp-text-secondary);
  }

  section {
    padding: var(--sp-3) 0;
    border-top: var(--sp-stitch);
  }

  .note {
    margin: 0 0 var(--sp-2);
    font-size: var(--sp-fs-small);
    color: var(--sp-text-tertiary);
  }

  .error {
    color: var(--sp-danger);
    font-size: var(--sp-fs-small);
  }

  .rule-list {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .rule {
    display: flex;
    align-items: flex-start;
    gap: var(--sp-2);
    padding: var(--sp-2) 0;
    border-bottom: var(--sp-stitch);
    border-top: 2px solid transparent;
  }

  .rule.drop-before {
    border-top: 2px solid var(--sp-accent-edge);
  }

  .rule.disabled .rule-name,
  .rule.disabled .summary {
    opacity: 0.55;
  }

  .grip {
    padding-top: 3px;
    color: var(--sp-text-muted);
    cursor: grab;
  }

  .grip:active {
    cursor: grabbing;
  }

  .grip.locked {
    cursor: default;
    color: var(--sp-text-tertiary);
  }

  .rule-body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .rule-name {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    font-size: var(--sp-fs-small);
    color: var(--sp-text-primary);
  }

  .badge {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 0 var(--sp-1);
    font-size: var(--sp-fs-caption);
    border-radius: var(--sp-r-pill);
  }

  .error-badge {
    color: var(--sp-danger);
    border: 1px solid var(--sp-danger);
  }

  .summary {
    font-size: var(--sp-fs-caption);
    color: var(--sp-text-secondary);
    overflow-wrap: anywhere;
  }

  .summary .warn {
    color: var(--sp-flag);
  }

  .summary .danger {
    color: var(--sp-danger);
    font-weight: 600;
  }

  .rule-actions {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
  }

  .icon-btn {
    background: none;
    border: 0;
    color: var(--sp-text-tertiary);
    cursor: pointer;
    padding: 3px;
  }

  .icon-btn:hover:not(:disabled) {
    color: var(--sp-text-primary);
  }

  .icon-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .empty {
    padding: var(--sp-3) 0;
    color: var(--sp-text-tertiary);
    font-size: var(--sp-fs-small);
  }

  .name-field {
    display: block;
    margin: 0 0 var(--sp-2);
    font-size: var(--sp-fs-caption);
    text-transform: uppercase;
    letter-spacing: var(--sp-track-label);
    color: var(--sp-text-muted);
  }

  .name-field input {
    display: block;
    width: 100%;
    margin-top: var(--sp-1);
    padding: var(--sp-1) var(--sp-2);
    font: 400 var(--sp-fs-small) / var(--sp-lh-ui) var(--sp-font-ui);
    color: var(--sp-text-primary);
    background: var(--sp-surface-well);
    border: 1px solid var(--sp-border-hard);
    border-radius: var(--sp-r-control);
    text-transform: none;
    letter-spacing: normal;
  }

  .row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin: var(--sp-1) 0 var(--sp-2);
    font-size: var(--sp-fs-small);
    color: var(--sp-text-primary);
  }

  .actions {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin-top: var(--sp-3);
  }

  .actions.footer {
    justify-content: flex-end;
    border-top: var(--sp-stitch);
    padding-top: var(--sp-3);
  }

  .danger-btn {
    color: var(--sp-danger);
    border-color: var(--sp-danger);
  }

  .external-confirm {
    margin-top: var(--sp-3);
    padding: var(--sp-3);
    border: 1px solid var(--sp-danger);
    border-radius: var(--sp-r-control);
    background: var(--sp-surface-well);
  }

  .external-confirm p {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin: 0 0 var(--sp-2);
    font-size: var(--sp-fs-small);
    color: var(--sp-danger);
  }

  .external-confirm ul {
    margin: 0 0 var(--sp-2);
    padding-left: var(--sp-5);
    font-size: var(--sp-fs-small);
    color: var(--sp-text-primary);
  }
</style>
