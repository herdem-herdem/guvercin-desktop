// Helpers for making Guvercin the OS default `mailto:` handler.
// Backed by macOS LaunchServices via Tauri commands; safe no-ops elsewhere.

const PROMPT_FLAG = 'default_mail_prompt_shown'

function isTauri() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

export async function isDefaultMailClient() {
  if (!isTauri()) return false
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    return Boolean(await invoke('is_default_mail_client'))
  } catch (error) {
    console.error('is_default_mail_client failed:', error)
    return false
  }
}

export async function setAsDefaultMailClient() {
  if (!isTauri()) return false
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    await invoke('set_as_default_mail_client')
    return true
  } catch (error) {
    console.error('set_as_default_mail_client failed:', error)
    return false
  }
}

// Whether the first-launch prompt has already been shown (and answered) once.
export function hasShownDefaultPrompt() {
  try {
    return localStorage.getItem(PROMPT_FLAG) === '1'
  } catch {
    return false
  }
}

export function markDefaultPromptShown() {
  try {
    localStorage.setItem(PROMPT_FLAG, '1')
  } catch {
    // ignore storage failures
  }
}
