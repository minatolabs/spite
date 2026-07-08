import { invoke } from '@tauri-apps/api/core'

// Wire shapes mirror core/src/rules.rs. Index signatures matter: unknown
// fields from Graph ride through IPC as plain JS keys, and the round-trip
// invariant is that we NEVER rebuild these objects from form state — the
// builders mutate known keys in place and everything else survives untouched.

export type RuleEmailAddress = { name?: string; address?: string; [k: string]: unknown }
export type RuleRecipient = { emailAddress?: RuleEmailAddress; [k: string]: unknown }
export type RulePredicates = { [k: string]: unknown }
export type RuleActions = { [k: string]: unknown }
export type MessageRule = {
  id?: string
  displayName?: string
  sequence?: number
  isEnabled?: boolean
  isReadOnly?: boolean
  hasError?: boolean
  conditions?: RulePredicates | null
  exceptions?: RulePredicates | null
  actions?: RuleActions | null
  [k: string]: unknown
}

export const rules = $state({
  list: [] as MessageRule[],
  loaded: false,
  loading: false,
  error: '',
  /** Domain of the signed-in UPN — the boundary for "external" forwards. */
  accountDomain: '',
})

export function setAccountDomain(upn: string) {
  rules.accountDomain = (upn.split('@')[1] ?? '').toLowerCase()
}

export async function loadRules() {
  rules.loading = true
  rules.error = ''
  try {
    rules.list = await invoke<MessageRule[]>('list_message_rules')
    rules.loaded = true
  } catch (e) {
    rules.error = String(e)
  } finally {
    rules.loading = false
  }
}

/** Load once (idempotent) — used by the SettingsPane standing indicator. */
export async function ensureRules() {
  if (rules.loaded) return
  try {
    rules.list = await invoke<MessageRule[]>('list_message_rules')
    rules.loaded = true
  } catch {
    /* offline and nothing cached — indicator stays absent */
  }
}

/** Optimistic enable/disable with snapshot restore (the Phase 7 discipline). */
export async function toggleRule(rule: MessageRule) {
  if (rule.isReadOnly || !rule.id) return
  rules.error = ''
  const snapshot = rules.list
  const flipped = { ...rule, isEnabled: !rule.isEnabled }
  rules.list = snapshot.map((r) => (r.id === rule.id ? flipped : r))
  try {
    rules.list = await invoke<MessageRule[]>('update_message_rule', { rule: flipped })
  } catch (e) {
    rules.error = String(e)
    rules.list = snapshot
  }
}

export async function removeRule(id: string) {
  rules.error = ''
  const snapshot = rules.list
  rules.list = snapshot.filter((r) => r.id !== id)
  try {
    rules.list = await invoke<MessageRule[]>('delete_message_rule', { id })
  } catch (e) {
    rules.error = String(e)
    rules.list = snapshot
  }
}

export async function reorderRules(orderedIds: string[]) {
  rules.error = ''
  const snapshot = rules.list
  const byId = new Map(snapshot.map((r) => [r.id, r]))
  rules.list = orderedIds.map((id) => byId.get(id)).filter(Boolean) as MessageRule[]
  try {
    rules.list = await invoke<MessageRule[]>('reorder_message_rules', { orderedIds })
  } catch (e) {
    rules.error = String(e)
    // The shell re-fetched the authoritative order before erroring; repaint
    // from the server rather than trusting either local order.
    await loadRules()
  }
}

/** Editor save (create or update). Returns an error string or null. */
export async function saveRule(rule: MessageRule): Promise<string | null> {
  try {
    rules.list = rule.id
      ? await invoke<MessageRule[]>('update_message_rule', { rule })
      : await invoke<MessageRule[]>('create_message_rule', { rule })
    return null
  } catch (e) {
    return String(e)
  }
}

export async function duplicateRule(rule: MessageRule) {
  rules.error = ''
  // structuredClone keeps unknown keys; strip the server-owned fields.
  const copy = structuredClone(rule) as MessageRule
  delete copy.id
  delete copy.sequence
  delete copy.hasError
  delete copy.isReadOnly
  copy.displayName = `${rule.displayName ?? 'Rule'} (copy)`
  copy.isEnabled = false // a duplicated forward/delete rule shouldn't fire until reviewed
  const err = await saveRule(copy)
  if (err) rules.error = err
}

// ---------------------------------------------------------------------------
// Summary generation. SECURITY INVARIANT: the summary is computed by
// enumerating the keys ACTUALLY PRESENT on the raw objects, not by rendering
// a known-field list — so a rule can under-describe an action's meaning but
// can never omit its existence. Present-but-unrendered keys surface as an
// explicit "unrecognized — preserved" warning segment.
// ---------------------------------------------------------------------------

export type Segment = { text: string; kind: 'plain' | 'warn' | 'danger' }

const plain = (text: string): Segment => ({ text, kind: 'plain' })
const warn = (text: string): Segment => ({ text, kind: 'warn' })
const danger = (text: string): Segment => ({ text, kind: 'danger' })

function addresses(recipients: unknown): string[] {
  if (!Array.isArray(recipients)) return []
  return recipients.map((r: RuleRecipient) => r?.emailAddress?.address || '(unrecognized recipient — preserved)')
}

function names(list: unknown): string {
  return Array.isArray(list) ? list.map((s) => `"${s}"`).join(', ') : ''
}

export function isExternal(address: string): boolean {
  const domain = address.split('@')[1]?.toLowerCase()
  // Unparseable/unrecognized recipients count as external — fail loud.
  if (!domain) return true
  return rules.accountDomain !== '' && domain !== rules.accountDomain
}

/** All forward-family recipient addresses on a rule that cross the account
 *  domain boundary. Drives the save-time confirm and the standing indicator. */
export function externalForwardAddresses(rule: MessageRule): string[] {
  const a = rule.actions ?? {}
  const all = [
    ...addresses(a.forwardTo),
    ...addresses(a.forwardAsAttachmentTo),
    ...addresses(a.redirectTo),
  ]
  return all.filter(isExternal)
}

export function externalForwardRuleCount(): number {
  return rules.list.filter((r) => externalForwardAddresses(r).length > 0).length
}

/** Predicate keys the summary knows how to describe. Anything present on the
 *  object but not in this map is counted and surfaced, never hidden. */
const PREDICATE_TEXT: Record<string, (v: unknown) => string> = {
  fromAddresses: (v) => `from ${addresses(v).join(', ')}`,
  sentToAddresses: (v) => `sent to ${addresses(v).join(', ')}`,
  sentToMe: (v) => (v ? 'sent to me' : 'not addressed to me directly'),
  sentOnlyToMe: (v) => (v ? 'sent only to me' : 'not sent only to me'),
  sentCcMe: (v) => (v ? "I'm Cc'd" : "I'm not Cc'd"),
  sentToOrCcMe: (v) => (v ? 'sent to or Cc me' : 'neither to nor Cc me'),
  notSentToMe: (v) => (v ? 'not sent to me' : 'sent to me'),
  recipientContains: (v) => `recipient contains ${names(v)}`,
  senderContains: (v) => `sender contains ${names(v)}`,
  subjectContains: (v) => `subject contains ${names(v)}`,
  bodyContains: (v) => `body contains ${names(v)}`,
  bodyOrSubjectContains: (v) => `subject or body contains ${names(v)}`,
  headerContains: (v) => `header contains ${names(v)}`,
  importance: (v) => `importance ${v}`,
  sensitivity: (v) => `sensitivity ${v}`,
  messageActionFlag: (v) => `action-flagged ${v}`,
  isApprovalRequest: (v) => (v ? 'is an approval request' : 'not an approval request'),
  isAutomaticForward: (v) => (v ? 'is an automatic forward' : 'not an automatic forward'),
  isAutomaticReply: (v) => (v ? 'is an automatic reply' : 'not an automatic reply'),
  isEncrypted: (v) => (v ? 'is encrypted' : 'not encrypted'),
  isMeetingRequest: (v) => (v ? 'is a meeting request' : 'not a meeting request'),
  isMeetingResponse: (v) => (v ? 'is a meeting response' : 'not a meeting response'),
  isNonDeliveryReport: (v) => (v ? 'is a non-delivery report' : 'not a non-delivery report'),
  isPermissionControlled: (v) => (v ? 'is permission-controlled' : 'not permission-controlled'),
  isReadReceipt: (v) => (v ? 'is a read receipt' : 'not a read receipt'),
  isSigned: (v) => (v ? 'is signed' : 'not signed'),
  isVoicemail: (v) => (v ? 'is a voicemail' : 'not a voicemail'),
  withinSizeRange: (v) => {
    const r = v as { minimumSize?: number; maximumSize?: number }
    return `size ${r?.minimumSize ?? 0}–${r?.maximumSize ?? '∞'} KB`
  },
  hasAttachments: (v) => (v ? 'has attachments' : 'has no attachments'),
  categories: (v) => `categorized ${names(v)}`,
}

function predicateSegments(preds: RulePredicates | null | undefined): {
  segments: Segment[]
  unknown: number
} {
  const segments: Segment[] = []
  let unknown = 0
  for (const key of Object.keys(preds ?? {})) {
    const value = (preds as RulePredicates)[key]
    if (value === null || value === undefined) continue
    const render = PREDICATE_TEXT[key]
    if (render) segments.push(plain(render(value)))
    else unknown++
  }
  return { segments, unknown }
}

/** Action keys the summary renders. The set difference against the keys
 *  actually present is what surfaces smuggled/unknown actions. */
export const RENDERED_ACTION_KEYS = new Set([
  'moveToFolder',
  'copyToFolder',
  'delete',
  'permanentDelete',
  'forwardTo',
  'forwardAsAttachmentTo',
  'redirectTo',
  'assignCategories',
  'markAsRead',
  'markImportance',
  'stopProcessingRules',
])

function forwardSegment(verb: string, recipients: unknown): Segment[] {
  const out: Segment[] = []
  const addrs = addresses(recipients)
  if (!addrs.length) return out
  const external = addrs.filter(isExternal)
  if (external.length) {
    out.push(danger(`${verb} externally to ${addrs.join(', ')}`))
  } else {
    out.push(warn(`${verb} to ${addrs.join(', ')}`))
  }
  return out
}

function actionSegments(
  actions: RuleActions | null | undefined,
  folderName: (id: string) => string,
): { segments: Segment[]; unknown: number } {
  const segments: Segment[] = []
  let unknown = 0
  const a = actions ?? {}
  for (const key of Object.keys(a)) {
    const value = a[key]
    if (value === null || value === undefined) continue
    switch (key) {
      case 'moveToFolder':
        segments.push(plain(`move to ${folderName(String(value))}`))
        break
      case 'copyToFolder':
        segments.push(plain(`copy to ${folderName(String(value))}`))
        break
      case 'delete':
        if (value) segments.push(plain('delete (to Deleted Items)'))
        break
      case 'permanentDelete':
        if (value) segments.push(danger('permanently delete (unrecoverable)'))
        break
      case 'forwardTo':
        segments.push(...forwardSegment('forward', value))
        break
      case 'forwardAsAttachmentTo':
        segments.push(...forwardSegment('forward as attachment', value))
        break
      case 'redirectTo':
        segments.push(...forwardSegment('redirect', value))
        break
      case 'assignCategories':
        segments.push(plain(`categorize ${names(value)}`))
        break
      case 'markAsRead':
        if (value) segments.push(plain('mark read'))
        break
      case 'markImportance':
        segments.push(plain(`mark importance ${value}`))
        break
      case 'stopProcessingRules':
        if (value) segments.push(plain('stop processing more rules'))
        break
      default:
        unknown++
    }
  }
  return { segments, unknown }
}

/** Human one-liner as styled segments. Enumerates every key present on the
 *  rule's conditions/exceptions/actions — unrecognized keys become explicit
 *  "preserved" warnings, so nothing a rule does is ever invisible here. */
export function ruleSummary(rule: MessageRule, folderName: (id: string) => string): Segment[] {
  const out: Segment[] = []
  const conds = predicateSegments(rule.conditions)
  if (conds.segments.length) {
    out.push(plain('If '))
    conds.segments.forEach((s, i) => {
      if (i > 0) out.push(plain(' and '))
      out.push(s)
    })
  } else {
    out.push(plain('For all incoming mail'))
  }
  if (conds.unknown > 0) {
    out.push(warn(` +${conds.unknown} unrecognized condition(s) — preserved`))
  }
  const excs = predicateSegments(rule.exceptions)
  if (excs.segments.length || excs.unknown > 0) {
    out.push(plain(' except '))
    excs.segments.forEach((s, i) => {
      if (i > 0) out.push(plain(' and '))
      out.push(s)
    })
    if (excs.unknown > 0) {
      out.push(warn(` +${excs.unknown} unrecognized exception(s) — preserved`))
    }
  }
  out.push(plain(' → '))
  const acts = actionSegments(rule.actions, folderName)
  if (acts.segments.length) {
    acts.segments.forEach((s, i) => {
      if (i > 0) out.push(plain(', '))
      out.push(s)
    })
  } else if (acts.unknown === 0) {
    out.push(warn('no actions'))
  }
  if (acts.unknown > 0) {
    out.push(warn(` ⚠ +${acts.unknown} unrecognized action(s) — preserved`))
  }
  return out
}
