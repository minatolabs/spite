<script lang="ts">
  import AddressField from './AddressField.svelte'
  import { isExternal, type RuleRecipient } from './rules.svelte'

  // Maps a raw Graph recipient array ⇄ AddressField chips WITHOUT losing
  // round-trip data: every change is reconciled against the ORIGINAL
  // recipient objects by address, so an untouched recipient (which may carry
  // unknown fields) is reported back verbatim, not rebuilt. A recipient with
  // no plain address can't become a chip — it renders as a preserved warning
  // and always survives the sync-back. The parent remounts this per rule
  // (keyed), so seeding chips once from the prop is safe.
  let {
    label,
    recipients = [] as RuleRecipient[],
    warnExternal = false,
    onchange,
  }: {
    label: string
    recipients?: RuleRecipient[]
    warnExternal?: boolean
    onchange: (recipients: RuleRecipient[]) => void
  } = $props()

  // Deliberate initial-value capture: the field seeds once from the prop and
  // the parent remounts it per rule (keyed), so live prop tracking is not
  // wanted — the originals map is the reconciliation baseline.
  // svelte-ignore state_referenced_locally
  const initial = recipients
  const originals = new Map<string, RuleRecipient>()
  const unmappable: RuleRecipient[] = []
  for (const r of initial) {
    const addr = r?.emailAddress?.address
    if (addr) originals.set(addr.toLowerCase(), r)
    else unmappable.push(r)
  }

  let chips = $state(
    initial
      .map((r) => r?.emailAddress)
      .filter((e): e is { name?: string; address?: string } => !!e?.address)
      .map((e) => ({ name: e.name ?? '', address: e.address as string })),
  )

  function syncBack() {
    onchange([
      ...chips.map(
        (c) =>
          originals.get(c.address.toLowerCase()) ?? {
            emailAddress: { name: c.name, address: c.address },
          },
      ),
      ...unmappable,
    ])
  }

  const externalChips = $derived(warnExternal ? chips.filter((c) => isExternal(c.address)) : [])
</script>

<div class="recipients" class:warn={warnExternal}>
  <AddressField {label} bind:chips onchange={syncBack} />
  {#if unmappable.length}
    <p class="preserved">
      {unmappable.length} unrecognized recipient(s) from another client — preserved
    </p>
  {/if}
  {#if externalChips.length}
    <p class="external">
      ⚠ Sends mail outside your organization: {externalChips.map((c) => c.address).join(', ')}
    </p>
  {/if}
</div>

<style>
  .recipients.warn :global(.chip) {
    border-color: var(--sp-flag);
  }

  .preserved,
  .external {
    margin: var(--sp-1) 0 0;
    font-size: var(--sp-fs-caption);
    color: var(--sp-flag);
  }

  .external {
    color: var(--sp-danger);
  }
</style>
