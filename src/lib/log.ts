/**
 * Front-end diagnostics — the one place anything is allowed to report a
 * failure that is not already visible to the user.
 *
 * Policy, deliberately narrow:
 *
 *  1. **No `console.*` anywhere in app code.** A packaged Tauri build has no
 *     devtools, so a `console.warn` in the field is written to a stream nobody
 *     will ever read: it is not "logging", it is deleting the message with
 *     extra steps.
 *  2. **Anything a user must act on is a toast** (`pushToast`), exactly as
 *     before. That path is unchanged and is still the primary channel.
 *  3. **Anything that is only useful in a bug report goes to the Rust file
 *     log** over the `onyx://client-log` event, which is the same sink the Rust
 *     side writes its own `log::warn!` / `log::error!` lines to. It is an event
 *     rather than a command on purpose: SPEC §3.1 fixes the command surface at
 *     46, and a log line must never be able to fail loudly or block.
 *  4. **The console is a dev-only mirror.** In `vite dev` the same records are
 *     echoed so a developer sees them immediately; in any built bundle they are
 *     not.
 *
 * Nothing in here may throw, and nothing in here may report its own failure
 * through itself.
 */

import { emit } from "@tauri-apps/api/event";

type LogLevel = "warn" | "error";

/** `emit` only works inside a Tauri webview; the mock preview runs in a tab. */
const IN_TAURI = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
const MIRROR_TO_CONSOLE = import.meta.env.DEV;

/** Best-effort one-line rendering of whatever was thrown. */
function detailOf(cause: unknown): string | null {
  if (cause == null) return null;
  if (typeof cause === "string") return cause;
  if (cause instanceof Error) return cause.stack ?? `${cause.name}: ${cause.message}`;
  try {
    return JSON.stringify(cause) ?? String(cause);
  } catch {
    return String(cause);
  }
}

function record(level: LogLevel, message: string, cause?: unknown): void {
  const detail = detailOf(cause);
  if (IN_TAURI) {
    // Fire and forget. A rejection here means the log line itself could not be
    // delivered; re-reporting it would be an infinite regress, and there is no
    // second channel to report it on.
    void emit("onyx://client-log", { level, message, detail }).catch(() => undefined);
  }
  if (MIRROR_TO_CONSOLE) {
    // The one sanctioned console call in the app, and it is compiled out of
    // every build that is not `vite dev`.
    console[level](`onyx: ${message}`, cause ?? "");
  }
}

/** A degraded feature the user does not need to act on. */
export function logWarn(message: string, cause?: unknown): void {
  record("warn", message, cause);
}

/** A failure that should appear in a bug report. */
export function logError(message: string, cause?: unknown): void {
  record("error", message, cause);
}

/**
 * Catch what no component catches: promises nobody awaited and exceptions that
 * escaped an event handler. Without this, both vanish completely in a packaged
 * build. Returns the uninstaller.
 */
export function installDiagnostics(): () => void {
  const onRejection = (e: PromiseRejectionEvent): void => {
    logError("unhandled promise rejection", e.reason);
  };
  const onError = (e: ErrorEvent): void => {
    logError(`uncaught error: ${e.message} (${e.filename}:${e.lineno})`, e.error);
  };
  window.addEventListener("unhandledrejection", onRejection);
  window.addEventListener("error", onError);
  return () => {
    window.removeEventListener("unhandledrejection", onRejection);
    window.removeEventListener("error", onError);
  };
}
