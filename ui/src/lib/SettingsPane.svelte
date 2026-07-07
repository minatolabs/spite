<script lang="ts">
  import { onMount } from 'svelte'
  import { Trash2 } from 'lucide-svelte'
  import RichBodyEditor from './RichBodyEditor.svelte'
  import {
    settings,
    loadMailboxSettings,
    saveAutomaticReplies,
    createCategory,
    recolorCategory,
    deleteCategory,
    presetCssVar,
    CATEGORY_SWATCHES,
    type AutomaticReplies,
  } from './settings.svelte'

  let { onclose }: { onclose: () => void } = $props()

  // Pane-local, editable copy of the out-of-office settings, seeded once from
  // the loaded server state.
  let enabled = $state(false)
  let scheduled = $state(false)
  let startLocal = $state('')
  let endLocal = $state('')
  let audience = $state('All')
  let internal = $state('')
  let external = $state('')
  let seeded = $state(false)
  let savingOof = $state(false)
  let oofSaved = $state(false)

  // New-category form.
  let newName = $state('')
  let newColor = $state(CATEGORY_SWATCHES[0].preset)
  let addingCat = $state(false)

  onMount(() => {
    void loadMailboxSettings()
  })

  // Graph dateTime ("2026-07-06T07:00:00.0000000") ⇆ datetime-local ("…T07:00").
  const toLocal = (dt: { dateTime: string } | null | undefined) =>
    dt?.dateTime ? dt.dateTime.slice(0, 16) : ''

  $effect(() => {
    if (settings.mailboxLoaded && !seeded && settings.mailbox) {
      const ar = settings.mailbox.automaticRepliesSetting
      enabled = ar.status !== 'Disabled'
      scheduled = ar.status === 'Scheduled'
      startLocal = toLocal(ar.scheduledStartDateTime)
      endLocal = toLocal(ar.scheduledEndDateTime)
      audience = ar.externalAudience || 'All'
      internal = ar.internalReplyMessage || ''
      external = ar.externalReplyMessage || ''
      seeded = true
    }
  })

  const zoneName = $derived(settings.mailbox?.timeZone || 'UTC')

  function localToGraph(local: string) {
    if (!local) return null
    // datetime-local gives minutes precision; Graph wants full seconds.
    const dateTime = local.length === 16 ? `${local}:00.0000000` : local
    return { dateTime, timeZone: zoneName }
  }

  async function saveOof() {
    savingOof = true
    oofSaved = false
    const replies: AutomaticReplies = {
      status: !enabled ? 'Disabled' : scheduled ? 'Scheduled' : 'AlwaysEnabled',
      externalAudience: audience,
      scheduledStartDateTime: scheduled ? localToGraph(startLocal) : null,
      scheduledEndDateTime: scheduled ? localToGraph(endLocal) : null,
      internalReplyMessage: internal,
      externalReplyMessage: external,
    }
    const ok = await saveAutomaticReplies(replies)
    savingOof = false
    oofSaved = ok
  }

  async function addCategory() {
    const name = newName.trim()
    if (!name) return
    addingCat = true
    const ok = await createCategory(name, newColor)
    addingCat = false
    if (ok) newName = ''
  }

  const dayLabel = (d: string) => d.charAt(0).toUpperCase() + d.slice(1, 3)
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
    aria-label="Mailbox settings"
    tabindex="-1"
    onkeydown={() => {}}
  >
    <h2>Mailbox settings</h2>

    {#if settings.loading && !settings.mailboxLoaded}
      <p class="note">Loading from your mailbox…</p>
    {/if}
    {#if settings.error}
      <p class="error">{settings.error}</p>
    {/if}

    {#if settings.mailboxLoaded}
      <!-- 1. Automatic replies -->
      <section>
        <h3>Automatic replies</h3>
        <label class="row">
          <input type="checkbox" bind:checked={enabled} />
          Send automatic replies
        </label>

        {#if enabled}
          <div class="indent">
            <label class="row">
              <input type="radio" bind:group={scheduled} value={false} />
              On until I turn it off
            </label>
            <label class="row">
              <input type="radio" bind:group={scheduled} value={true} />
              Only during a time period
            </label>

            {#if scheduled}
              <div class="window">
                <label>Start<input type="datetime-local" bind:value={startLocal} /></label>
                <label>End<input type="datetime-local" bind:value={endLocal} /></label>
                <span class="tz">Times in {zoneName}</span>
              </div>
            {/if}

            <label class="field">
              Reply to senders inside my organization
              <RichBodyEditor bind:value={internal} placeholder="Internal reply…" />
            </label>

            <label class="field">
              External senders
              <select bind:value={audience}>
                <option value="None">Don't reply to external senders</option>
                <option value="ContactsOnly">Only my contacts</option>
                <option value="All">Everyone outside my organization</option>
              </select>
            </label>

            {#if audience !== 'None'}
              <label class="field">
                External reply
                <RichBodyEditor bind:value={external} placeholder="External reply…" />
              </label>
            {/if}
          </div>
        {/if}

        <div class="actions">
          <button class="sp-btn sp-btn--primary" onclick={saveOof} disabled={savingOof}>
            {savingOof ? 'Saving…' : 'Save automatic replies'}
          </button>
          {#if oofSaved}<span class="ok">Saved</span>{/if}
        </div>
      </section>

      <!-- 2. Master categories -->
      <section>
        <h3>Categories</h3>
        <p class="note">
          Colors use Outlook's preset palette. Categories can't be renamed (a
          Microsoft limitation) — delete and recreate to change a name.
        </p>
        <ul class="cats">
          {#each settings.categories as cat (cat.id)}
            <li>
              <span class="chip" style="background: var({presetCssVar(cat.color)})"></span>
              <span class="cat-name">{cat.displayName}</span>
              <span class="swatches">
                {#each CATEGORY_SWATCHES as sw (sw.preset)}
                  <button
                    class="swatch"
                    class:active={cat.color === sw.preset}
                    style="background: var({sw.cssVar})"
                    title={sw.label}
                    aria-label={sw.label}
                    onclick={() => recolorCategory(cat.id, sw.preset)}
                  ></button>
                {/each}
              </span>
              <button class="icon-btn" title="Delete" onclick={() => deleteCategory(cat.id)}>
                <Trash2 size={12} />
              </button>
            </li>
          {/each}
          {#if settings.categories.length === 0}
            <li class="empty">No categories yet.</li>
          {/if}
        </ul>
        <div class="add-cat">
          <input
            bind:value={newName}
            placeholder="New category name"
            onkeydown={(e) => e.key === 'Enter' && addCategory()}
          />
          <span class="swatches">
            {#each CATEGORY_SWATCHES as sw (sw.preset)}
              <button
                class="swatch"
                class:active={newColor === sw.preset}
                style="background: var({sw.cssVar})"
                title={sw.label}
                aria-label={sw.label}
                onclick={() => (newColor = sw.preset)}
              ></button>
            {/each}
          </span>
          <button class="sp-btn" onclick={addCategory} disabled={addingCat || !newName.trim()}>
            Add
          </button>
        </div>
      </section>

      <!-- 3. Working hours / timezone / format (read-only) -->
      <section>
        <h3>Regional</h3>
        <dl class="regional">
          <dt>Time zone</dt>
          <dd>{settings.mailbox?.timeZone || '—'}</dd>
          <dt>Date format</dt>
          <dd>{settings.mailbox?.dateFormat || '—'}</dd>
          <dt>Time format</dt>
          <dd>{settings.mailbox?.timeFormat || '—'}</dd>
          {#if settings.mailbox?.workingHours}
            <dt>Working hours</dt>
            <dd>
              {settings.mailbox.workingHours.daysOfWeek.map(dayLabel).join(' ')}
              · {settings.mailbox.workingHours.startTime.slice(0, 5)}–{settings.mailbox.workingHours.endTime.slice(
                0,
                5,
              )}
            </dd>
          {/if}
        </dl>
        <p class="note">Read-only — used to render dates in your own timezone.</p>
      </section>
    {/if}

    <div class="actions footer">
      <button class="sp-btn" onclick={onclose}>Close</button>
    </div>
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
    width: min(560px, 92vw);
    max-height: 88vh;
    overflow-y: auto;
    padding: var(--sp-5);
    background: var(--sp-surface-raised);
    border: 1px solid var(--sp-border-hard);
    border-radius: var(--sp-r-panel);
    box-shadow: var(--sp-lift);
  }
  h2 {
    margin: 0 0 var(--sp-3);
    font-size: var(--sp-fs-title);
    color: var(--sp-text-display);
  }
  section {
    padding: var(--sp-3) 0;
    border-top: var(--sp-stitch);
  }
  h3 {
    margin: 0 0 var(--sp-2);
    font-size: var(--sp-fs-small);
    text-transform: uppercase;
    letter-spacing: var(--sp-track-label);
    color: var(--sp-text-secondary);
  }
  .note {
    margin: var(--sp-1) 0 var(--sp-2);
    font-size: var(--sp-fs-small);
    color: var(--sp-text-tertiary);
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin: var(--sp-1) 0;
    font-size: var(--sp-fs-small);
    color: var(--sp-text-primary);
  }
  .indent {
    padding-left: var(--sp-3);
    border-left: var(--sp-stitch);
    margin: var(--sp-2) 0;
  }
  .field {
    display: block;
    margin: var(--sp-3) 0 var(--sp-1);
    font-size: var(--sp-fs-caption);
    text-transform: uppercase;
    letter-spacing: var(--sp-track-label);
    color: var(--sp-text-muted);
  }
  .field :global(.rich),
  .field select {
    margin-top: var(--sp-1);
    text-transform: none;
    letter-spacing: normal;
  }
  select,
  input[type='datetime-local'],
  .add-cat input {
    padding: var(--sp-1) var(--sp-2);
    font: 400 var(--sp-fs-small) / var(--sp-lh-ui) var(--sp-font-ui);
    color: var(--sp-text-primary);
    background: var(--sp-surface-well);
    border: 1px solid var(--sp-border-hard);
    border-radius: var(--sp-r-control);
  }
  select {
    width: 100%;
  }
  .window {
    display: flex;
    flex-wrap: wrap;
    align-items: end;
    gap: var(--sp-2);
    margin: var(--sp-2) 0;
  }
  .window label {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
    font-size: var(--sp-fs-caption);
    color: var(--sp-text-muted);
  }
  .tz {
    font-size: var(--sp-fs-caption);
    color: var(--sp-text-tertiary);
  }
  .cats {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .cats li {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-1) 0;
  }
  .cats .empty {
    color: var(--sp-text-tertiary);
    font-size: var(--sp-fs-small);
  }
  .chip {
    width: 12px;
    height: 12px;
    border-radius: 3px;
    box-shadow: var(--sp-bevel);
    flex: none;
  }
  .cat-name {
    flex: 1;
    font-size: var(--sp-fs-small);
    color: var(--sp-text-primary);
  }
  .swatches {
    display: flex;
    gap: 3px;
  }
  .swatch {
    width: 14px;
    height: 14px;
    border-radius: 3px;
    border: 1px solid var(--sp-border-hard);
    cursor: pointer;
    padding: 0;
  }
  .swatch.active {
    outline: 2px solid var(--sp-text-primary);
    outline-offset: 1px;
  }
  .icon-btn {
    background: none;
    border: 0;
    color: var(--sp-text-tertiary);
    cursor: pointer;
    padding: 2px;
  }
  .icon-btn:hover {
    color: var(--sp-danger);
  }
  .add-cat {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin-top: var(--sp-2);
  }
  .add-cat input {
    flex: 1;
  }
  .regional {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: var(--sp-1) var(--sp-3);
    margin: 0;
    font-size: var(--sp-fs-small);
  }
  .regional dt {
    color: var(--sp-text-muted);
  }
  .regional dd {
    margin: 0;
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
  .ok {
    color: var(--sp-success);
    font-size: var(--sp-fs-small);
  }
  .error {
    color: var(--sp-danger);
    font-size: var(--sp-fs-small);
  }
</style>
