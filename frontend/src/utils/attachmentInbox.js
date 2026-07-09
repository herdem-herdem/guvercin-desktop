// Bridges OS-level `guvercin://attach-file` deep links to the compose UI.
//
// When a user selects "Send with Guvercin" from the context menu, the OS
// calls the app with a guvercin://attach-file?path=<path> URI. This module
// listens for these URIs and queues file attachment requests for the compose UI.

import { invoke } from '@tauri-apps/api/core'

const queue = []
const subscribers = new Set()
let initialized = false

function isTauri() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

async function attachFile(filePath) {
  if (!filePath || typeof filePath !== 'string') return

  try {
    await invoke('attach_file_to_compose', { filePath })
  } catch (error) {
    console.error('Failed to attach file:', error)
  }
}

function dispatch(attachment) {
  if (subscribers.size === 0) {
    queue.push(attachment)
    return
  }
  for (const cb of subscribers) {
    try {
      cb(attachment)
    } catch (error) {
      console.error('attachment subscriber failed:', error)
    }
  }
}

function parseAttachmentUri(uri) {
  if (typeof uri !== 'string') return null

  // Parse guvercin://attach-file?path=<encoded_path>
  const match = uri.match(/^guvercin:\/\/attach-file\?path=(.+)$/)
  if (!match) return null

  try {
    // Decode the path
    const decodedPath = decodeURIComponent(match[1])
    return { filePath: decodedPath }
  } catch {
    console.error('Failed to parse attachment URI:', uri)
    return null
  }
}

function handleUrls(urls) {
  if (!Array.isArray(urls)) return
  for (const url of urls) {
    if (typeof url === 'string' && url.startsWith('guvercin://attach-file')) {
      const attachment = parseAttachmentUri(url)
      if (attachment) {
        attachFile(attachment.filePath)
      }
    }
  }
}

// Installs the deep-link listeners. Safe to call multiple times.
export async function initAttachmentInbox() {
  if (initialized || !isTauri()) return
  initialized = true
  try {
    const { onOpenUrl, getCurrent } = await import('@tauri-apps/plugin-deep-link')
    // URIs the app was launched with (cold start).
    try {
      const current = await getCurrent()
      handleUrls(current)
    } catch {
      // getCurrent is unavailable on some platforms; ignore.
    }
    // URIs delivered while the app is running (hot).
    await onOpenUrl(handleUrls)
  } catch (error) {
    console.error('Failed to initialize attachment deep-link handling:', error)
  }
}
