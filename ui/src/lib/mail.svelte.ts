import { invoke } from '@tauri-apps/api/core'

export type Folder = {
  id: string
  display_name: string
  well_known_name: string | null
  parent_id: string | null
}
export type MessageSummary = {
  id: string
  folder_id: string
  subject: string
  from_name: string
  from_address: string
  received_at: number
  preview: string
  is_read: boolean
  has_attachments: boolean
  flag_status: string
  inference_classification: string
  is_draft: boolean
}
export type Message = {
  summary: MessageSummary
  conversation_id: string | null
  body_html: string | null
  body_content_type: string | null
  categories: string[]
}
export type SyncState = {
  folder_id: string
  delta_link: string | null
  last_synced_at: number | null
}
export type SyncReport = { initial: boolean; pages: number; upserted: number; removed: number }
export type MessageBody = { body: string; content_type: string }
export type SearchFilters = {
  folder_id: string | null
  unread_only: boolean
  has_attachments: boolean
  flagged_only: boolean
  from: string | null
  date_from: number | null
  date_to: number | null
}
export type SearchHit = {
  entity_type: string
  entity_id: string
  title: string
  snippet: string
  ts: number
  summary: MessageSummary | null
}
export type ServerHit = { summary: MessageSummary; internet_message_id: string | null }
export type SavedSearch = { id: number; name: string; query: string; filters: string }

export const PAGE_SIZE = 50

export const mail = $state({
  folders: [] as Folder[],
  unread: {} as Record<string, number>,
  folderId: null as string | null,
  messages: [] as MessageSummary[],
  hasMore: false,
  selectedId: null as string | null,
  syncState: null as SyncState | null,
  syncing: false,
  syncError: '',
  // Search state
  query: '',
  scopeAll: false,
  chips: {
    unread_only: false,
    has_attachments: false,
    flagged_only: false,
    from: '' as string,
    days: 0 as number, // 0 = any time
  },
  /// Focused-inbox tab in the Inbox: 'focused' | 'other' | 'all'.
  focusTab: 'all' as 'focused' | 'other' | 'all',
  hits: [] as SearchHit[],
  serverHits: [] as ServerHit[],
  serverSearching: false,
  serverSearched: false,
  searchError: '',
  savedSearches: [] as SavedSearch[],
  /// Summary of a selected server-only hit (not in the local store).
  serverSelected: null as MessageSummary | null,
  /// Transient status-bar flash (e.g. stubbed keyboard verbs).
  flash: '',
  /// A failed mail-management action (surfaced as a dismissable banner) —
  /// distinct from `syncError` so write failures aren't mislabeled "offline".
  actionError: '',
  /// Multi-select: ids in the current selection + the range anchor.
  selection: new Set<string>() as Set<string>,
  anchorId: null as string | null,
  /// True while a never-synced folder is doing its first load.
  folderLoading: false,
})

let flashTimer: ReturnType<typeof setTimeout> | undefined
export function flash(text: string) {
  mail.flash = text
  clearTimeout(flashTimer)
  flashTimer = setTimeout(() => (mail.flash = ''), 2500)
}

export function chipsActive(): boolean {
  const c = mail.chips
  return (
    c.unread_only || c.has_attachments || c.flagged_only || c.from.trim() !== '' || c.days > 0
  )
}

export function searchActive(): boolean {
  return mail.query.trim() !== '' || chipsActive()
}

function currentFilters(): SearchFilters {
  const c = mail.chips
  return {
    folder_id: mail.scopeAll ? null : mail.folderId,
    unread_only: c.unread_only,
    has_attachments: c.has_attachments,
    flagged_only: c.flagged_only,
    from: c.from.trim() || null,
    date_from: c.days > 0 ? Math.floor(Date.now() / 1000) - c.days * 86400 : null,
    date_to: null,
  }
}

/** Instant local search (or filtered browse when the query is empty). */
export async function runSearch() {
  mail.serverHits = []
  mail.serverSearched = false
  mail.searchError = ''
  if (!searchActive()) {
    mail.hits = []
    return
  }
  try {
    mail.hits = await invoke<SearchHit[]>('search_local', {
      query: mail.query,
      filters: currentFilters(),
      limit: 100,
    })
  } catch (e) {
    mail.searchError = String(e)
  }
}

/** Deep server search, deduped against local in the shell. */
export async function searchEverywhere() {
  if (mail.serverSearching || !mail.query.trim()) return
  mail.serverSearching = true
  mail.searchError = ''
  try {
    mail.serverHits = await invoke<ServerHit[]>('search_server', {
      query: mail.query,
      filters: currentFilters(),
    })
    mail.serverSearched = true
  } catch (e) {
    mail.searchError = `server search failed: ${e}`
  } finally {
    mail.serverSearching = false
  }
}

export function clearSearch() {
  mail.query = ''
  mail.chips = { unread_only: false, has_attachments: false, flagged_only: false, from: '', days: 0 }
  mail.hits = []
  mail.serverHits = []
  mail.serverSearched = false
  mail.searchError = ''
  mail.serverSelected = null
}

export async function loadSavedSearches() {
  try {
    mail.savedSearches = await invoke<SavedSearch[]>('list_saved_searches')
  } catch {
    // non-fatal
  }
}

export async function saveCurrentSearch(name: string) {
  await invoke('save_search', { name, query: mail.query, filters: currentFilters() })
  await loadSavedSearches()
}

export async function deleteSavedSearch(id: number) {
  await invoke('delete_saved_search', { id })
  await loadSavedSearches()
}

export async function applySavedSearch(saved: SavedSearch) {
  mail.query = saved.query
  try {
    const f = JSON.parse(saved.filters) as Partial<SearchFilters>
    mail.scopeAll = !f.folder_id
    mail.chips = {
      unread_only: !!f.unread_only,
      has_attachments: !!f.has_attachments,
      flagged_only: !!f.flagged_only,
      from: f.from ?? '',
      days: f.date_from ? Math.max(1, Math.round((Date.now() / 1000 - f.date_from) / 86400)) : 0,
    }
  } catch {
    // filters unreadable → query-only search
  }
  await runSearch()
}

export function selectedFolder(): Folder | null {
  return mail.folders.find((f) => f.id === mail.folderId) ?? null
}

async function paintFolders() {
  mail.folders = await invoke<Folder[]>('list_folders')
  const counts = await invoke<[string, number][]>('unread_counts')
  mail.unread = Object.fromEntries(counts)
}

async function paintMessages(reset = true) {
  if (!mail.folderId) return
  const offset = reset ? 0 : mail.messages.length
  const page = await invoke<MessageSummary[]>('list_messages', {
    folderId: mail.folderId,
    limit: PAGE_SIZE,
    offset,
  })
  mail.messages = reset ? page : [...mail.messages, ...page]
  mail.hasMore = page.length === PAGE_SIZE
}

async function paintSyncState() {
  if (!mail.folderId) return
  mail.syncState = await invoke<SyncState | null>('get_sync_status', {
    folderId: mail.folderId,
  })
}

export async function loadMore() {
  await paintMessages(false)
}

/** Offline-first: paint from SQLite immediately, then reconcile via delta.
 *  Any folder (Inbox, Archive, Sent, Junk, Deleted, user folders) lazy-syncs
 *  here via its own per-folder cursor. */
export async function selectFolder(id: string) {
  mail.folderId = id
  mail.selectedId = null
  mail.messages = []
  mail.focusTab = 'all'
  clearSelection()
  await paintMessages() // local paint — never blocked by the network
  await paintSyncState()
  // First open of a never-synced folder: show a loading state until the
  // background sync fills it, so the user sees progress instead of "empty".
  mail.folderLoading = mail.messages.length === 0 && !mail.syncState?.last_synced_at
  void syncNow()
}

export async function syncNow() {
  if (!mail.folderId || mail.syncing) return
  mail.syncing = true
  mail.syncError = ''
  try {
    await invoke<SyncReport>('sync_folder', { folderId: mail.folderId })
    await paintMessages()
    await paintFolders()
  } catch (e) {
    // A folder-open sync failure is a real error, not "offline reading".
    mail.actionError = `Couldn't load this folder: ${e}`
  } finally {
    mail.syncing = false
    mail.folderLoading = false
    await paintSyncState()
  }
}

/** Initial load: local state first, then refresh the folder list from Graph. */
export async function initMail() {
  await paintFolders()
  const inbox = mail.folders.find((f) => f.well_known_name === 'inbox')
  if (inbox) await selectFolder(inbox.id)
  try {
    await invoke<Folder[]>('sync_folders')
    await paintFolders()
    if (!mail.folderId) {
      const i = mail.folders.find((f) => f.well_known_name === 'inbox')
      if (i) await selectFolder(i.id)
    }
    // One-shot repair of rows an earlier partial delta event blanked
    // (empty sender/subject); re-fetches their summaries from Graph.
    const repaired = await invoke<number>('repair_summaries')
    if (repaired > 0) await paintMessages()
  } catch {
    // Offline: the locally cached folder list is all we need.
  }
}

export function wellKnownFolderId(name: string): string | null {
  return mail.folders.find((f) => f.well_known_name === name)?.id ?? null
}

function patchLocalSummary(id: string, patch: Partial<MessageSummary>) {
  mail.messages = mail.messages.map((m) => (m.id === id ? { ...m, ...patch } : m))
  mail.hits = mail.hits.map((h) =>
    h.entity_id === id && h.summary ? { ...h, summary: { ...h.summary, ...patch } } : h,
  )
}

function removeLocal(id: string) {
  mail.messages = mail.messages.filter((m) => m.id !== id)
  mail.hits = mail.hits.filter((h) => h.entity_id !== id)
  if (mail.selectedId === id) mail.selectedId = null
}

/** Immediate optimistic op (read/flag/categories/inference). The svelte list
 *  updates instantly; if the shell (store + Graph) rejects it, we repaint
 *  from the authoritative store, which already rolled back. */
async function immediateOp(op: object, optimistic: () => void) {
  mail.actionError = ''
  optimistic()
  try {
    await invoke<string>('apply_op', { op })
  } catch (e) {
    // Surface the failure loudly — the store already rolled back, so repaint
    // to match, and tell the user rather than swallowing it.
    mail.actionError = String(e)
    await paintMessages()
    await refreshCounts()
  }
}

export function toggleRead(m: MessageSummary) {
  const next = !m.is_read
  void immediateOp({ kind: 'setRead', id: m.id, isRead: next }, () =>
    patchLocalSummary(m.id, { is_read: next }),
  ).then(() => refreshCounts())
}

/** Mark an unread message read via the optimistic path (no-op if already
 *  read). Used by auto-mark-read-on-dwell; manual toggle stays separate. */
export function markRead(m: MessageSummary) {
  if (m.is_read) return
  void immediateOp({ kind: 'setRead', id: m.id, isRead: true }, () =>
    patchLocalSummary(m.id, { is_read: true }),
  ).then(() => refreshCounts())
}

export function toggleFlag(m: MessageSummary) {
  const next = m.flag_status === 'flagged' ? 'notFlagged' : 'flagged'
  void immediateOp({ kind: 'setFlag', id: m.id, flagged: next === 'flagged' }, () =>
    patchLocalSummary(m.id, { flag_status: next }),
  )
}

export function setFocused(m: MessageSummary, focused: boolean) {
  void immediateOp({ kind: 'setInference', id: m.id, focused }, () =>
    patchLocalSummary(m.id, { inference_classification: focused ? 'focused' : 'other' }),
  )
}

export async function setCategories(id: string, categories: string[]) {
  mail.actionError = ''
  try {
    await invoke('apply_op', { op: { kind: 'setCategories', id, categories } })
  } catch (e) {
    mail.actionError = String(e)
    await paintMessages()
  }
}

async function refreshCounts() {
  const counts = await invoke<[string, number][]>('unread_counts')
  mail.unread = Object.fromEntries(counts)
}

/** Undoable op (archive/delete/move): the row leaves the list immediately;
 *  the shell shows the undo toast and fires Graph on lapse. */
export async function queueMove(id: string, destFolderId: string, label: string) {
  mail.actionError = ''
  removeLocal(id)
  try {
    await invoke('queue_op', {
      op: { kind: 'move', id, destFolderId },
      label,
    })
  } catch (e) {
    mail.actionError = String(e)
    await paintMessages()
  }
}

export async function archive(id: string) {
  const dest = wellKnownFolderId('archive')
  if (!dest) return
  await queueMove(id, dest, 'archive')
}

export async function softDelete(id: string) {
  const dest = wellKnownFolderId('deleteditems')
  if (!dest) return
  await queueMove(id, dest, 'delete')
}

export async function hardDelete(id: string) {
  mail.actionError = ''
  removeLocal(id)
  try {
    await invoke('queue_op', { op: { kind: 'hardDelete', id }, label: 'delete' })
  } catch (e) {
    mail.actionError = String(e)
    await paintMessages()
  }
}

/** Repaint the current view from the authoritative store — used after an
 *  undone op restores a row, or a queued op fails and rolls back. */
export async function refreshList() {
  await paintMessages()
  await refreshCounts()
  if (searchActive()) await runSearch()
}

// ---- General move (item 1): reuses the move op, so move-back is free. ----

export async function moveToFolder(id: string, destFolderId: string) {
  if (!destFolderId) return
  await queueMove(id, destFolderId, 'move')
}

// ---- Multi-select model (item 4). ----

/** The visible row order, used for Shift-range selection. */
export function visibleRows(): MessageSummary[] {
  if (searchActive()) {
    return mail.hits.filter((h) => h.summary).map((h) => h.summary as MessageSummary)
  }
  if (mail.focusTab === 'all') return mail.messages
  return mail.messages.filter((m) => m.inference_classification === mail.focusTab)
}

export function isSelected(id: string): boolean {
  return mail.selection.has(id)
}

export function clearSelection() {
  if (mail.selection.size) mail.selection = new Set()
  mail.anchorId = null
}

/** Click semantics: plain = single-select + open; Ctrl/Cmd = toggle; Shift =
 *  range from the anchor over the visible order. */
export function clickSelect(id: string, opts: { shift?: boolean; ctrl?: boolean }) {
  const next = new Set(mail.selection)
  if (opts.shift && mail.anchorId) {
    const rows = visibleRows().map((m) => m.id)
    const a = rows.indexOf(mail.anchorId)
    const b = rows.indexOf(id)
    if (a !== -1 && b !== -1) {
      const [lo, hi] = a < b ? [a, b] : [b, a]
      for (let i = lo; i <= hi; i++) next.add(rows[i])
    }
  } else if (opts.ctrl) {
    if (next.has(id)) next.delete(id)
    else next.add(id)
    mail.anchorId = id
  } else {
    // Plain click: single-select + open in the reading pane.
    next.clear()
    next.add(id)
    mail.anchorId = id
    mail.selectedId = id
    mail.serverSelected = null
  }
  mail.selection = next
}

export function selectAll() {
  mail.selection = new Set(visibleRows().map((m) => m.id))
}

// ---- Bulk actions (item 4): each maps the selection to a MailOp and fires
// one queue_bulk_op, which shows a single undo toast and one $batch on lapse.

function selectedIds(): string[] {
  return [...mail.selection]
}

async function queueBulk(ops: object[], label: string, optimistic: () => void) {
  if (!ops.length) return
  mail.actionError = ''
  optimistic()
  clearSelection()
  try {
    await invoke('queue_bulk_op', { ops, label })
  } catch (e) {
    mail.actionError = String(e)
    await paintMessages()
  }
}

export async function bulkArchive() {
  const dest = wellKnownFolderId('archive')
  if (!dest) return
  const ids = selectedIds()
  await queueBulk(
    ids.map((id) => ({ kind: 'move', id, destFolderId: dest })),
    'archive',
    () => ids.forEach(removeLocal),
  )
}

export async function bulkDelete() {
  const dest = wellKnownFolderId('deleteditems')
  if (!dest) return
  const ids = selectedIds()
  await queueBulk(
    ids.map((id) => ({ kind: 'move', id, destFolderId: dest })),
    'delete',
    () => ids.forEach(removeLocal),
  )
}

export async function bulkMove(destFolderId: string) {
  const ids = selectedIds()
  await queueBulk(
    ids.map((id) => ({ kind: 'move', id, destFolderId })),
    'move',
    () => ids.forEach(removeLocal),
  )
}

export async function bulkMarkRead(isRead: boolean) {
  const ids = selectedIds()
  await queueBulk(
    ids.map((id) => ({ kind: 'setRead', id, isRead })),
    isRead ? 'mark read' : 'mark unread',
    () => ids.forEach((id) => patchLocalSummary(id, { is_read: isRead })),
  )
  void refreshCounts()
}

export async function bulkFlag(flagged: boolean) {
  const ids = selectedIds()
  await queueBulk(
    ids.map((id) => ({ kind: 'setFlag', id, flagged })),
    flagged ? 'flag' : 'unflag',
    () =>
      ids.forEach((id) =>
        patchLocalSummary(id, { flag_status: flagged ? 'flagged' : 'notFlagged' }),
      ),
  )
}
