# Debug Session: Tauri Window Flash Close

**Session ID**: `tauri-window-flash-close`
**Status**: [OPEN]
**OS**: Windows
**App**: Miku Tunes (Tauri + Leptos)
**Symptom**: Window opens then immediately closes, no taskbar icon persists

---

## Hypotheses

1. **WASM Runtime Panic**: The Leptos/WASM app panics during initialization (e.g., canvas context, localStorage access), causing WebView to terminate.
2. **WebView2 Dev URL Unreachable**: `devUrl: http://localhost:1420` is not ready when the window opens, causing a blank page error and WebView crash.
3. **Trunk Rebuild Loop**: `trunk serve`'s watch mode re-triggers a build while the window is loading, corrupting the WASM fetch.
4. **console_error_panic_hook not showing**: The Rust WASM panic is caught by `console_error_panic_hook` but the error is swallowed by WebView's silent mode.
5. **Three.js importmap issue**: The importmap for `three` loads before WASM init but may cause a runtime module resolution failure.

---

## Logs

(To be filled after instrumentation)
