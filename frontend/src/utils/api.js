// Dynamically resolved base URL.
//
// Strategy:
//   1. On module load we immediately start resolving the backend port via the
//      Tauri `get_backend_port` command (retrying every 100 ms for up to 5 s).
//   2. While resolving, _resolvedBase starts as the env-var / legacy fallback so
//      calls during startup still work.
//   3. Once the real port is known, _resolvedBase is updated — all future calls
//      pick it up automatically.
//   4. `apiReady()` returns a Promise that resolves once the real port is known,
//      so critical initialisation code can await it before the first fetch.

const _fallback = import.meta.env.VITE_API_URL || 'http://127.0.0.1:5000';

// Start with the fallback; replaced as soon as the real port arrives.
let _resolvedBase = _fallback;
let _readyResolve;
export const apiReady = new Promise(res => { _readyResolve = res; });

(async () => {
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    for (let i = 0; i < 50; i++) {
      const port = await invoke('get_backend_port');
      if (port) {
        _resolvedBase = `http://127.0.0.1:${port}`;
        break;
      }
      await new Promise(r => setTimeout(r, 100));
    }
  } catch {
    // Not inside Tauri — keep the fallback.
  }
  _readyResolve();
})();

// Synchronous — safe to call from non-async contexts.
export const apiUrl = (path) => {
  if (path.startsWith('http')) return path;
  return `${_resolvedBase}${path}`;
};

// Backward-compat export (some files may import this directly).
export const getApiBaseUrl = () => _resolvedBase;
