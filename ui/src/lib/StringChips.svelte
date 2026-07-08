<script lang="ts">
  // Controlled chip input for plain string lists (subjectContains etc.) —
  // the AddressField chip idiom without autocomplete. Fully controlled:
  // renders `values`, reports every change via `onchange`.
  let {
    values = [] as string[],
    placeholder = 'text…',
    onchange,
  }: {
    values?: string[]
    placeholder?: string
    onchange: (values: string[]) => void
  } = $props()

  let input = $state('')

  function commit() {
    const v = input.trim().replace(/[,;]$/, '')
    if (v && !values.includes(v)) onchange([...values, v])
    input = ''
  }
</script>

<div class="chips">
  {#each values as v, i (v)}
    <span class="chip">
      {v}
      <button class="x" onclick={() => onchange(values.toSpliced(i, 1))} title="Remove">×</button>
    </span>
  {/each}
  <input
    bind:value={input}
    onkeydown={(e) => {
      if (e.key === 'Enter' || e.key === ',' || e.key === ';') {
        e.preventDefault()
        commit()
      } else if (e.key === 'Backspace' && !input && values.length) {
        onchange(values.slice(0, -1))
      }
    }}
    onblur={commit}
    placeholder={values.length ? '' : placeholder}
  />
</div>

<style>
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
    min-width: 120px;
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
</style>
