import { t } from "./lib/i18n";
import { useEffect } from "react";
import { useLanguage } from './lib/i18n';
import * as api from "./lib/api";
import { IS_MAC } from "./lib/dom";
import { carriesFiles, deckAtPoint, dropFilesOnDeck } from "./lib/drop";
import { startEqWindowSync } from "./lib/eqwindow";
import { installFocusHygiene } from "./lib/focus";
import { startBridge } from "./lib/frame";
import { installKeyboard } from "./lib/keys";
import { loadKeyboardLayout } from "./lib/layout";
import { logWarn } from "./lib/log";
import { useStore } from "./lib/store";
import { onThemeDocProblem, startAppearanceSync, takeThemeDocProblem } from "./lib/theme";
import AbRail from "./components/AbRail";
import BadgeRail from "./components/Badges";
import TitleBar from "./components/TitleBar";
import WaveformStack from "./components/WaveformStack";
import Playlist from "./components/Playlist";
import TransportBar from "./components/TransportBar";
import BlindTest from "./components/BlindTest";
import SettingsPanel from "./components/SettingsPanel";
import Toasts from "./components/Toasts";
import ShortcutOverlay from "./components/ShortcutOverlay";
import DropOverlay from "./components/DropOverlay";
import Editor from "./editor/Editor";
import "./editor/unified.css";

export default function App() {
  const language = useLanguage();
  useEffect(() => { void api.editorIo({ action: 'language', language }).catch(e => logWarn('Language sync failed', e)); }, [language]);
  const editorActive = useStore(s => s.editorActive);
  const modeSwitching = useStore(s => s.modeSwitching);
  const modeError = useStore(s => s.modeError);
  useEffect(() => {
    if (editorActive) void api.transportPause().catch(e => useStore.getState().pushToast("error", api.errorMessage(e)));
  }, [editorActive]);
  useEffect(() => {
    let disposed = false; let off: (() => void) | undefined;
    void api.listenEvent("onyx://editor-open", () => useStore.getState().setEditorActive(true)).then(un => { if (disposed) un(); else off = un; });
    return () => { disposed = true; off?.(); };
  }, []);
  const snapshot = useStore((s) => s.snapshot);
  const settingsOpen = useStore((s) => s.settingsOpen);
  const shortcutsOpen = useStore((s) => s.shortcutsOpen);
  const blindOpen = useStore((s) => s.blindOpen);
  const dropActive = useStore((s) => s.dropActive);
  const abEnabled = snapshot?.ab.enabled ?? false;
  const blindActive = snapshot?.blind.active ?? false;
  const setBlindOpen = useStore((s) => s.setBlindOpen);
  const setDropActive = useStore((s) => s.setDropActive);
  const setSnapshot = useStore((s) => s.setSnapshot);
  const pushToast = useStore((s) => s.pushToast);

  useEffect(() => {
    const stopBridge = startBridge();
    // The EQ is a window of its own (SPEC §12); this only mirrors Rust's
    // `onyx://eq-window` into the store so the EQ button tells the truth.
    const stopEqWindow = startEqWindowSync();
    const stopKeys = installKeyboard();
    // a mouse click must not leave a control looking engaged (lib/focus.ts)
    const stopFocus = installFocusHygiene();
    // theme, accent, fonts and size scale, mirrored from the snapshot; the EQ
    // window runs the same subscription of its own (SPEC §14)
    const stopAppearance = startAppearanceSync();
    // Best effort, and deliberately not awaited: it only affects what the
    // shortcut overlay prints, never what the keys do.
    void loadKeyboardLayout();
    return () => {
      stopBridge();
      stopEqWindow();
      stopKeys();
      stopFocus();
      stopAppearance();
    };
  }, []);

  /* A saved theme document that could not be read is a *notice*, not a white
     screen (SPEC §20): the window comes up in the designed appearance and says
     why, with the line number, so the document can be fixed rather than
     silently lost. The first one happens before React exists — `initAppearance`
     runs at module scope — so it is collected; later ones arrive with a
     snapshot and need the listener. */
  useEffect(() => {
    const first = takeThemeDocProblem();
    if (first) pushToast("error", first);
    return onThemeDocProblem((why) => pushToast("error", why));
  }, [pushToast]);

  /* a running blind test always keeps its panel reachable */
  useEffect(() => {
    if (snapshot?.blind.active) setBlindOpen(true);
  }, [snapshot?.blind.active, setBlindOpen]);

  /* real file paths from the webview — Tauri only */
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;

    void (async () => {
      try {
        const { getCurrentWebview } = await import("@tauri-apps/api/webview");
        const un = await getCurrentWebview().onDragDropEvent((event) => {
          if (useStore.getState().editorActive) return;
          const payload = event.payload;
          if (payload.type === "over") {
            /* Which affordance to show. A file held over a waveform lane is
               about to become *that deck's* material (SPEC §2.8), so the lane
               lights instead of the window-wide "drop to append" overlay —
               otherwise the packaged app promises an append and performs an
               assignment. The DOM never sees this drag, so the position from
               the native event is hit-tested by hand; it is physical pixels
               and `elementFromPoint` wants CSS pixels. */
            const dpr = window.devicePixelRatio || 1;
            const over = payload.position
              ? deckAtPoint(payload.position.x / dpr, payload.position.y / dpr)
              : null;
            useStore.getState().setDropDeck(over);
            setDropActive(!over);
            return;
          }
          if (payload.type === "drop") {
            setDropActive(false);
            useStore.getState().setDropDeck(null);
            const paths = payload.paths ?? [];
            if (paths.length === 0) return;
            const fail = (err: unknown): void =>
              pushToast("error", api.errorMessage(err));
            /* A file dropped *on a waveform lane* means that deck (SPEC §2.8).
               The webview never sees a DOM drop for files inside Tauri — the
               native handler takes them first and reports one window event —
               so the lane cannot claim it by stopping propagation, and this
               handler hit-tests the reported position instead. The position is
               physical pixels; `elementFromPoint` wants CSS pixels. */
            const dpr = window.devicePixelRatio || 1;
            const at = payload.position;
            const deck = at ? deckAtPoint(at.x / dpr, at.y / dpr) : null;
            if (deck) {
              dropFilesOnDeck(paths, deck).then(setSnapshot).catch(fail);
              return;
            }
            // drop = append; only replace when there is nothing to append to
            const empty = (useStore.getState().snapshot?.playlist.length ?? 0) === 0;
            api.openFiles(paths, empty).then(setSnapshot).catch(fail);
            return;
          }
          setDropActive(false);
          useStore.getState().setDropDeck(null);
        });
        if (disposed) un();
        else unlisten = un;
      } catch (err) {
        // Expected in the mock preview, which runs in a plain tab. Inside
        // Tauri it means dropping files silently does nothing at all, with no
        // error anywhere — exactly the failure a bug report needs to name.
        if (!api.MOCK) logWarn("drag-drop listener unavailable", err);
      }
    })();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [pushToast, setDropActive, setSnapshot]);

  /* browser fallback so the drop affordance is testable outside Tauri */
  useEffect(() => {
    if (!api.MOCK) return;
    let depth = 0;
    // Reordering a playlist row is a drag too, and it used to raise the
    // full-window "drop audio" overlay over the list being reordered.
    const over = (e: DragEvent): void => {
      if (!carriesFiles(e.dataTransfer)) return;
      e.preventDefault();
      depth += 1;
      // Over a lane, the lane's own highlight is the affordance and it means
      // something else: assign to that deck, not append (SPEC §2.8).
      setDropActive(!deckAtPoint(e.clientX, e.clientY));
    };
    const leave = (e: DragEvent): void => {
      if (depth === 0) return;
      e.preventDefault();
      depth = Math.max(0, depth - 1);
      if (depth === 0) setDropActive(false);
    };
    const drop = (e: DragEvent): void => {
      if (!carriesFiles(e.dataTransfer)) return;
      // A lane took it: it is an assignment to that deck, not an append. The
      // lane stops propagation, so this listener normally does not run at all
      // — the check is the belt to that braces, and the one that would still
      // hold if a future lane forgot.
      if (e.defaultPrevented || deckAtPoint(e.clientX, e.clientY)) {
        depth = 0;
        setDropActive(false);
        return;
      }
      e.preventDefault();
      depth = 0;
      setDropActive(false);
      const paths = Array.from(e.dataTransfer?.files ?? []).map((f) => f.name);
      if (!paths.length) return;
      const empty = (useStore.getState().snapshot?.playlist.length ?? 0) === 0;
      api
        .openFiles(paths, empty)
        .then(setSnapshot)
        .catch((err) => pushToast("error", api.errorMessage(err)));
    };
    // this one used to be added with an inline arrow and never removed
    const allow = (e: DragEvent): void => {
      if (!carriesFiles(e.dataTransfer)) return;
      e.preventDefault();
      // crossing from the window onto a lane hands the drag over to the lane
      if (deckAtPoint(e.clientX, e.clientY)) setDropActive(false);
    };
    window.addEventListener("dragenter", over);
    window.addEventListener("dragover", allow);
    window.addEventListener("dragleave", leave);
    window.addEventListener("drop", drop);
    return () => {
      window.removeEventListener("dragenter", over);
      window.removeEventListener("dragover", allow);
      window.removeEventListener("dragleave", leave);
      window.removeEventListener("drop", drop);
    };
  }, [pushToast, setDropActive, setSnapshot]);

  return (
    <div className={"app" + (editorActive ? " editing" : "")} data-platform={IS_MAC ? "mac" : "win"}>
      <TitleBar />

      <div className="editor-tab-panel" role="tabpanel" aria-label={t("剪辑")} hidden={!editorActive}>
        <Editor active={editorActive} />
      </div>
      <div className="stage" style={editorActive ? { display: "none" } : undefined}>
        <WaveformStack />
        <Playlist />
      </div>

      {t(!editorActive && abEnabled && !blindActive && <AbRail />)}

      {t(!editorActive && <TransportBar />)}
      {t(!editorActive && <BadgeRail />)}

      {t(settingsOpen && <SettingsPanel />)}
      {t(blindOpen && <BlindTest />)}
      {t(shortcutsOpen && <ShortcutOverlay />)}
      {t(dropActive && <DropOverlay />)}
      <Toasts />
      {modeSwitching && <div className="mode-feedback" role="status">{t('正在准备音频，请稍候…')}</div>}
      {!modeSwitching && modeError && <div className="mode-feedback error" role="alert"><span>{t('无法完成操作：')}{t(modeError)}</span><button aria-label={t('Close')} onClick={() => useStore.getState().setModeError(null)}>×</button></div>}

      {t(!snapshot && <div className="connecting">{t("connecting to engine")}</div>)}
    </div>
  );
}
