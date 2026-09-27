import { useState } from "react";
import * as api from "../lib/api";
import { useAudibleDeck } from "../lib/audible";
import { IS_MAC } from "../lib/dom";
import { toggleEqWindow } from "../lib/eqwindow";
import { useStore } from "../lib/store";
import { formatBadge, formatLufs } from "../lib/format";
import { closeWindow, minimizeWindow, toggleMaximizeWindow } from "../lib/window";
import { IconClose, IconEq, IconGear, IconKeys, IconMaximize, IconMinimize } from "./Icons";
// the MIDI badge below (SPEC §18) is styled there
import "../styles/settings.css";

export default function TitleBar() {
  const snapshot = useStore((s) => s.snapshot);
  // Rust's answer, not a local guess: the EQ window can be closed with its own
  // close button, and this button has to know (SPEC §12).
  const eqOpen = useStore((s) => s.eqWindow.open);
  const [eqBusy, setEqBusy] = useState(false);
  const eqEnabled = snapshot?.eq.enabled ?? false;
  const hasEqBands = (snapshot?.eq.bands.length ?? 0) > 0;
  const eqActive = !!snapshot?.eq.enabled && snapshot.eq.bands.some((band) => band.enabled);
  const settingsOpen = useStore((s) => s.settingsOpen);
  const shortcutsOpen = useStore((s) => s.shortcutsOpen);
  const toggleSettings = useStore((s) => s.toggleSettings);
  const toggleShortcuts = useStore((s) => s.toggleShortcuts);

  const blind = snapshot?.blind ?? null;
  const blinded = blind?.active ?? false;
  /* The title bar names the track you are *hearing*, so it follows the frame
     stream's deck rather than the snapshot's (`lib/audible.ts`). */
  const audible = useAudibleDeck();
  const deck = audible === "b" ? snapshot?.deckB ?? null : snapshot?.deckA ?? null;
  const info = blinded ? null : deck?.info ?? null;
  const lufs = blinded ? null : deck?.analysis?.integratedLufs ?? null;

  return (
    <header className="titlebar" data-tauri-drag-region>
      <div className="tb-left" data-tauri-drag-region>
        <span className="wordmark">
          ONYX<em>.</em>
        </span>
      </div>

      <div className="tb-center" data-tauri-drag-region>
        {info ? (
          <>
            <span className="tb-file" title={info.path}>
              {info.title ?? info.fileName}
            </span>
            <span className="dot">·</span>
            {/* A rendered MIDI file is the one case where the badge names a
                *choice* rather than a fact about the file: the bank decides
                what you hear (SPEC §18). So it is a button, and it goes
                where the bank is chosen. Everything else stays plain text —
                nothing about a 24-bit FLAC is actionable. */}
            {info.synthBank ? (
              <button
                className="tb-badge as-link"
                onClick={() => !settingsOpen && toggleSettings()}
                title={`Rendered through ${info.synthBank} \u00B7 choose another .sf2 in Settings`}
              >
                {formatBadge(info)}
              </button>
            ) : (
              <span className="tb-badge">{formatBadge(info)}</span>
            )}
            {lufs != null && (
              <>
                <span className="dot">·</span>
                <span className="tb-lufs num">{formatLufs(lufs)} LUFS</span>
              </>
            )}
          </>
        ) : blinded ? (
          <span className="tb-idle shimmer">
            {/* Narrow windows drop this line from the outside in (`app.css`,
                "Narrow windows"): "identity hidden" is what the masked meters
                and the single unlabelled lane are already saying, and "blind
                test" is what the lit Blind button says. The trial number is the
                one part of it that is written nowhere else, so it is the part
                that survives to 420 px. */}
            <span className="tb-blind-word">blind test · </span>trial {blind?.trial} /{" "}
            {blind?.trials}
            <span className="tb-blind-tail"> · identity hidden</span>
          </span>
        ) : (
          <span className="tb-idle">no track loaded</span>
        )}
      </div>

      <div className="tb-right">
        {hasEqBands && (
          <button
            className="tb-btn eq-power"
            data-eq-active={eqActive}
            disabled={eqBusy}
            aria-label="EQ processing"
            aria-pressed={eqEnabled}
            title={eqEnabled ? "Turn EQ off (Shift+E)" : "Turn EQ on (Shift+E)"}
            onClick={async () => {
              const config = useStore.getState().snapshot?.eq;
              if (!config || eqBusy) return;
              setEqBusy(true);
              try {
                await api.setEq({ ...config, enabled: !config.enabled });
              } catch (err) {
                useStore.getState().pushToast("error", api.errorMessage(err));
              } finally {
                setEqBusy(false);
              }
            }}
          >
            {eqEnabled ? "EQ ON" : "EQ OFF"}
          </button>
        )}
        <button
          className="tb-btn"
          data-on={eqOpen}
          onClick={toggleEqWindow}
          title="Open / close EQ editor (E)"
          aria-label="Equaliser"
          aria-expanded={eqOpen}
        >
          <IconEq size={13} />
          {!hasEqBands && "EQ"}
        </button>
        <button
          className="tb-btn"
          data-on={settingsOpen}
          onClick={toggleSettings}
          title="Open settings — theme, colors, and audio output"
          aria-label="Settings"
          aria-expanded={settingsOpen}
        >
          <IconGear size={15} />
        </button>
        <button
          className="tb-btn"
          data-on={shortcutsOpen}
          onClick={toggleShortcuts}
          title="Keyboard shortcuts (?)"
          aria-label="Shortcuts"
          aria-expanded={shortcutsOpen}
        >
          <IconKeys size={13} />
        </button>

        {/* macOS draws its own traffic lights (the title bar leaves 78 px for
            them); on Windows and Linux the window has no decorations at all,
            so without these three it can only be closed with Alt+F4. */}
        {!IS_MAC && (
          <div className="tb-winctl">
            <button
              className="tb-win"
              onClick={minimizeWindow}
              title="Minimise"
              aria-label="Minimise"
            >
              <IconMinimize size={12} />
            </button>
            <button
              className="tb-win"
              onClick={toggleMaximizeWindow}
              title="Maximise / restore"
              aria-label="Maximise or restore"
            >
              <IconMaximize size={12} />
            </button>
            <button
              className="tb-win danger"
              onClick={closeWindow}
              title="Close"
              aria-label="Close"
            >
              <IconClose size={12} />
            </button>
          </div>
        )}
      </div>
    </header>
  );
}
