/**
 * The three window controls Windows and Linux do not draw for us.
 *
 * `tauri.conf.json` turns the native decorations off so the app can draw its
 * own 36 px title bar (SPEC §4). On macOS the traffic lights are kept — see
 * `tauri.macos.conf.json` — but on Windows and Linux "no decorations" means
 * exactly that: without these buttons the only ways out of the window are
 * Alt+F4 and the taskbar.
 *
 * The API is imported lazily and every call is best-effort: the mock preview
 * (`npm run dev:mock`) runs in an ordinary browser tab where there is no
 * window to control, and a failure here must never break the title bar.
 */

import * as api from "./api";
import { logWarn } from "./log";

type WindowAction = "minimize" | "toggleMaximize" | "close";

async function act(action: WindowAction): Promise<void> {
  if (api.MOCK) return;
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const win = getCurrentWindow();
    if (action === "minimize") await win.minimize();
    else if (action === "toggleMaximize") await win.toggleMaximize();
    else await win.close();
  } catch (err) {
    logWarn(`window ${action} failed`, err);
  }
}

export const minimizeWindow = (): void => void act("minimize");
export const toggleMaximizeWindow = (): void => void act("toggleMaximize");
export const closeWindow = (): void => void act("close");
