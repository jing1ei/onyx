/** AbortSignal listener options are unavailable in Big Sur's original WebKit. */
export function listenUntilAbort<K extends keyof WindowEventMap>(
  target: Window,
  type: K,
  listener: (event: WindowEventMap[K]) => void,
  signal: AbortSignal,
  capture = false,
): void {
  if (signal.aborted) return;
  target.addEventListener(type, listener, capture);
  signal.addEventListener("abort", () => target.removeEventListener(type, listener, capture), { once: true });
}
