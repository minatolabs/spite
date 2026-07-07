import { invoke } from '@tauri-apps/api/core'

// Wire shapes mirror core/src/settings.rs (serde rename_all = "camelCase").

export type DateTimeTimeZone = { dateTime: string; timeZone: string }

export type AutomaticReplies = {
  /** 'Disabled' | 'AlwaysEnabled' | 'Scheduled' */
  status: string
  /** 'None' | 'ContactsOnly' | 'All' */
  externalAudience: string
  scheduledStartDateTime: DateTimeTimeZone | null
  scheduledEndDateTime: DateTimeTimeZone | null
  internalReplyMessage: string
  externalReplyMessage: string
}

export type WorkingHours = {
  daysOfWeek: string[]
  startTime: string
  endTime: string
  timeZone: { name: string } | null
}

export type MailboxSettings = {
  automaticRepliesSetting: AutomaticReplies
  timeZone: string
  dateFormat: string
  timeFormat: string
  workingHours: WorkingHours | null
  language: { locale: string; displayName: string } | null
}

export type MasterCategory = { id: string; displayName: string; color: string }

/** Mirror of core/src/settings.rs OFFERED_PRESETS: the curated brass/verdigris
 *  swatches the picker offers. Kept in lockstep with the Rust side (which
 *  validates writes) so a category never lands on a color outside this set. */
export type Swatch = { preset: string; cssVar: string; label: string }
export const CATEGORY_SWATCHES: Swatch[] = [
  { preset: 'preset1', cssVar: '--sp-cat-brass', label: 'Brass' },
  { preset: 'preset3', cssVar: '--sp-cat-gold', label: 'Old gold' },
  { preset: 'preset2', cssVar: '--sp-cat-bronze', label: 'Bronze' },
  { preset: 'preset5', cssVar: '--sp-cat-verd', label: 'Verdigris' },
  { preset: 'preset4', cssVar: '--sp-cat-patina', label: 'Patina' },
  { preset: 'preset20', cssVar: '--sp-cat-deepverd', label: 'Deep verdigris' },
]

/** The CSS custom property for a Graph preset. Anything outside the curated
 *  set (a red/cranberry set elsewhere, or `None`) → neutral, never oxblood. */
export function presetCssVar(preset: string): string {
  return CATEGORY_SWATCHES.find((s) => s.preset === preset)?.cssVar ?? '--sp-cat-neutral'
}

export const settings = $state({
  mailbox: null as MailboxSettings | null,
  mailboxLoaded: false,
  categories: [] as MasterCategory[],
  categoriesLoaded: false,
  loading: false,
  error: '',
})

/** Load the master-category list once (idempotent). Used by the assign picker,
 *  which must have the list even if the settings pane was never opened. Failure
 *  is swallowed — the picker degrades to plain create/free-text. */
export async function ensureCategories() {
  if (settings.categoriesLoaded) return
  try {
    settings.categories = await invoke<MasterCategory[]>('list_master_categories')
    settings.categoriesLoaded = true
  } catch {
    /* offline and nothing cached — leave unloaded */
  }
}

async function refreshCategories() {
  settings.categories = await invoke<MasterCategory[]>('list_master_categories')
  settings.categoriesLoaded = true
}

/** Full load for the settings pane: mailbox settings + master categories. */
export async function loadMailboxSettings() {
  settings.loading = true
  settings.error = ''
  try {
    settings.mailbox = await invoke<MailboxSettings>('get_mailbox_settings')
    settings.mailboxLoaded = true
    await refreshCategories()
  } catch (e) {
    settings.error = String(e)
  } finally {
    settings.loading = false
  }
}

/** Optimistic save of the out-of-office settings: apply locally, commit to
 *  Graph, roll back on failure (the Phase 7 discipline, on settings state). */
export async function saveAutomaticReplies(replies: AutomaticReplies): Promise<boolean> {
  settings.error = ''
  const prev = settings.mailbox?.automaticRepliesSetting
  if (settings.mailbox) settings.mailbox.automaticRepliesSetting = replies
  try {
    await invoke('set_automatic_replies', { replies })
    return true
  } catch (e) {
    settings.error = String(e)
    if (settings.mailbox && prev) settings.mailbox.automaticRepliesSetting = prev
    return false
  }
}

export async function createCategory(displayName: string, color: string): Promise<boolean> {
  settings.error = ''
  const snapshot = settings.categories
  settings.categories = [...snapshot, { id: `pending-${displayName}`, displayName, color }]
  try {
    // The command returns the fresh authoritative list (server ids included).
    settings.categories = await invoke<MasterCategory[]>('create_master_category', {
      displayName,
      color,
    })
    return true
  } catch (e) {
    settings.error = String(e)
    settings.categories = snapshot
    return false
  }
}

export async function recolorCategory(id: string, color: string): Promise<boolean> {
  settings.error = ''
  const snapshot = settings.categories
  settings.categories = snapshot.map((c) => (c.id === id ? { ...c, color } : c))
  try {
    settings.categories = await invoke<MasterCategory[]>('set_master_category_color', { id, color })
    return true
  } catch (e) {
    settings.error = String(e)
    settings.categories = snapshot
    return false
  }
}

export async function deleteCategory(id: string): Promise<boolean> {
  settings.error = ''
  const snapshot = settings.categories
  settings.categories = snapshot.filter((c) => c.id !== id)
  try {
    settings.categories = await invoke<MasterCategory[]>('delete_master_category', { id })
    return true
  } catch (e) {
    settings.error = String(e)
    settings.categories = snapshot
    return false
  }
}
