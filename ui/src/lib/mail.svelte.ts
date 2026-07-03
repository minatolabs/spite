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
}
export type Message = {
  summary: MessageSummary
  conversation_id: string | null
  body_html: string | null
  body_content_type: string | null
}
export type SyncState = {
  folder_id: string
  delta_link: string | null
  last_synced_at: number | null
}
export type SyncReport = { initial: boolean; pages: number; upserted: number; removed: number }
export type MessageBody = { body: string; content_type: string }

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
})

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

/** Offline-first: paint from SQLite immediately, then reconcile via delta. */
export async function selectFolder(id: string) {
  mail.folderId = id
  mail.selectedId = null
  mail.messages = []
  await paintMessages() // local paint — never blocked by the network
  await paintSyncState()
  void syncNow() // reconcile in the background
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
    mail.syncError = String(e)
  } finally {
    mail.syncing = false
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
  } catch {
    // Offline: the locally cached folder list is all we need.
  }
}
