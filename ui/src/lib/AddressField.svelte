<script lang="ts">
  import { invoke } from '@tauri-apps/api/core'

  type EmailAddress = { name: string; address: string }

  let {
    label,
    chips = $bindable([] as EmailAddress[]),
    onchange,
  }: {
    label: string
    chips: EmailAddress[]
    onchange?: () => void
  } = $props()

  let input = $state('')
  let suggestions: [string, string][] = $state([])
  let highlighted = $state(0)
  let focused = $state(false)
  let debounce: ReturnType<typeof setTimeout> | undefined

  function commit(address: string, name = '') {
    const addr = address.trim().replace(/[,;]$/, '')
    if (!addr) return
    if (!chips.some((c) => c.address.toLowerCase() === addr.toLowerCase())) {
      chips = [...chips, { name, address: addr }]
      onchange?.()
    }
    input = ''
    suggestions = []
  }

  function onInput() {
    clearTimeout(debounce)
    const q = input.trim()
    if (!q) {
      suggestions = []
      return
    }
    debounce = setTimeout(async () => {
      try {
        suggestions = await invoke<[string, string][]>('autocomplete_recipients', { query: q })
        highlighted = 0
      } catch {
        suggestions = []
      }
    }, 120)
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'ArrowDown' && suggestions.length) {
      e.preventDefault()
      highlighted = (highlighted + 1) % suggestions.length
    } else if (e.key === 'ArrowUp' && suggestions.length) {
      e.preventDefault()
      highlighted = (highlighted - 1 + suggestions.length) % suggestions.length
    } else if (e.key === 'Enter' || e.key === ',' || e.key === ';') {
      if (input.trim() || suggestions.length) e.preventDefault()
      if (suggestions.length) {
        const [address, name] = suggestions[highlighted]
        commit(address, name)
      } else {
        commit(input)
      }
    } else if (e.key === 'Backspace' && !input && chips.length) {
      chips = chips.slice(0, -1)
      onchange?.()
    } else if (e.key === 'Escape') {
      suggestions = []
    }
  }

  function remove(i: number) {
    chips = chips.toSpliced(i, 1)
    onchange?.()
  }
</script>

<div class="field">
  <span class="label">{label}</span>
  <div class="chips">
    {#each chips as chip, i (chip.address)}
      <span class="chip" title={chip.address}>
        {chip.name || chip.address}
        <button class="x" onclick={() => remove(i)} title="Remove">×</button>
      </span>
    {/each}
    <input
      bind:value={input}
      oninput={onInput}
      onkeydown={onKeydown}
      onfocus={() => (focused = true)}
      onblur={() => {
        focused = false
        setTimeout(() => commit(input), 150)
      }}
      placeholder={chips.length ? '' : 'address@example.com'}
    />
  </div>
  {#if focused && suggestions.length}
    <ul class="dropdown">
      {#each suggestions as [address, name], i (address)}
        <li>
          <button
            class:active={i === highlighted}
            onmousedown={(e) => {
              e.preventDefault()
              commit(address, name)
            }}
          >
            {#if name}<span class="name">{name}</span>{/if}
            <span class="addr">{address}</span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .field {
    position: relative;
    display: flex;
    align-items: flex-start;
    gap: var(--sp-2);
    padding: var(--sp-1) 0;
    border-bottom: var(--sp-stitch);
  }

  .label {
    flex: none;
    width: 42px;
    padding-top: 5px;
    font-size: var(--sp-fs-caption);
    text-transform: uppercase;
    letter-spacing: var(--sp-track-label);
    color: var(--sp-text-muted);
  }

  .chips {
    flex: 1;
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-1);
    align-items: center;
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
    box-shadow: var(--sp-bevel);
  }

  .x {
    border: none;
    background: none;
    color: var(--sp-text-tertiary);
    cursor: pointer;
    font-size: var(--sp-fs-body);
    line-height: 1;
    padding: 0 2px;
  }

  .x:hover {
    color: var(--sp-danger);
  }

  input {
    flex: 1;
    min-width: 140px;
    border: none;
    background: transparent;
    color: var(--sp-text-primary);
    font: inherit;
    font-size: var(--sp-fs-small);
    padding: 4px 0;
    outline: none;
  }

  input::placeholder {
    color: var(--sp-text-placeholder);
  }

  .dropdown {
    position: absolute;
    top: 100%;
    left: 50px;
    right: 0;
    z-index: var(--sp-z-dropdown);
    margin: 0;
    padding: var(--sp-1);
    list-style: none;
    background: var(--ink-700);
    border: 1px solid var(--sp-border-hard);
    border-radius: var(--sp-r-control);
    box-shadow: var(--sp-lift);
  }

  .dropdown button {
    display: flex;
    gap: var(--sp-2);
    width: 100%;
    padding: 5px var(--sp-2);
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--sp-text-primary);
    font: inherit;
    font-size: var(--sp-fs-small);
    text-align: left;
    cursor: pointer;
  }

  .dropdown button.active,
  .dropdown button:hover {
    background: var(--sp-selected-fill);
  }

  .name {
    color: var(--sp-text-primary);
  }

  .addr {
    color: var(--sp-text-tertiary);
  }
</style>
