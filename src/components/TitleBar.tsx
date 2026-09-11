import { t } from "../lib/i18n";
import { useAudibleDeck } from "../lib/audible";
import { switchMode } from '../lib/documentMode';
import { useLanguage, setLanguage } from '../lib/i18n';
import { IS_MAC } from "../lib/dom";
import { toggleEqWindow } from "../lib/eqwindow";
import { useStore } from "../lib/store";
import { formatBadge, formatLufs } from "../lib/format";
import { closeWindow, minimizeWindow, toggleMaximizeWindow } from "../lib/window";
import { IconClose, IconEq, IconGear, IconKeys, IconMaximize, IconMinimize } from "./Icons";
// the MIDI badge below (SPEC §18) is styled there
import "../styles/settings.css";

export default function TitleBar() {
  const language = useLanguage();
  const editorActive = useStore(s => s.editorActive);
  const modeSwitching = useStore(s => s.modeSwitching);
  const setEditorActive = (value: boolean) => { void switchMode(value); };
  const editorPath = useStore(s => s.editorPath);
  const editorDirty = useStore(s => s.editorDirty);
  const snapshot = useStore((s) => s.snapshot);
  // Rust's answer, not a local guess: the EQ window can be closed with its own
  // close button, and this button has to know (SPEC §12).
  const eqOpen = useStore((s) => s.eqWindow.open);
  const eqBypassed = (snapshot?.eq.bands.length ?? 0) > 0 && !(snapshot?.eq.enabled ?? true);
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
        <span className="wordmark">{t("ONYX")}<em>.</em>
        </span>
        <nav className="mode-tabs" role="tablist" aria-label={t("工作模式")}>
          <button role="tab" aria-selected={!editorActive} disabled={modeSwitching} onClick={() => setEditorActive(false)}>{t("试听")}</button>
          <button role="tab" aria-selected={editorActive} aria-busy={modeSwitching} disabled={modeSwitching || snapshot?.blind.active} title={snapshot?.blind.active ? t('请先结束盲测，再进入剪辑') : undefined} onClick={() => setEditorActive(true)}>{t("剪辑")}</button>
        </nav>
      </div>

      <div className="tb-center" data-tauri-drag-region>
        {t(editorActive ? <span className="tb-file" title={t(editorPath)}>{t(editorPath.split(/[\\/]/).pop() || '未载入音频')}{t(editorDirty ? ' *' : '')}</span> : info ? (
          <>
            <span className="tb-file" title={t(info.path)}>
              {info.title ?? info.fileName}
            </span>
            <span className="dot">·</span>
            {/* A rendered MIDI file is the one case where the badge names a
                *choice* rather than a fact about the file: the bank decides
                what you hear (SPEC §18). So it is a button, and it goes
                where the bank is chosen. Everything else stays plain text —
                nothing about a 24-bit FLAC is actionable. */}
            {t(info.synthBank ? (
              <button
                className="tb-badge as-link"
                onClick={() => !settingsOpen && toggleSettings()}
                title={t(`Rendered through ${info.synthBank} \u00B7 choose another .sf2 in Settings`)}
              >
                {t(formatBadge(info))}
              </button>
            ) : (
              <span className="tb-badge">{t(formatBadge(info))}</span>
            ))}
            {t(lufs != null && (
              <>
                <span className="dot">·</span>
                <span className="tb-lufs num">{t(formatLufs(lufs))}{t(" LUFS")}</span>
              </>
            ))}
          </>
        ) : blinded ? (
          <span className="tb-idle shimmer">
            {/* Narrow windows drop this line from the outside in (`app.css`,
                "Narrow windows"): "identity hidden" is what the masked meters
                and the single unlabelled lane are already saying, and "blind
                test" is what the lit Blind button says. The trial number is the
                one part of it that is written nowhere else, so it is the part
                that survives to 420 px. */}
            <span className="tb-blind-word">{t("blind test · ")}</span>{t("trial ")}{t(blind?.trial)} /{t(" ")}
            {t(blind?.trials)}
            <span className="tb-blind-tail">{t(" · identity hidden")}</span>
          </span>
        ) : (
          <span className="tb-idle">{t("no track loaded")}</span>
        ))}
      </div>

      <div className="tb-right">
        <select className="language-switch" aria-label={t("Language / 语言")} value={language} onChange={e => setLanguage(e.target.value === 'en' ? 'en' : 'zh')}><option value="zh">{t("中文")}</option><option value="en">{t("English")}</option></select>
        <button
          className="tb-btn"
          data-on={eqOpen}
          onClick={toggleEqWindow}
          title={t(eqBypassed
              ? "Equaliser (E) \u00B7 bypassed (\u21E7E)"
              : eqOpen
                ? "Equaliser (E) \u00B7 open in its own window"
                : "Equaliser (E)")}
          aria-label={t("Equaliser")}
          aria-pressed={eqOpen}
        >
          <IconEq size={13} />
          {/* The curve is edited in another window, possibly on another screen:
              this is the only place in the main window that can say the EQ is
              switched off under a curve the user drew. */}
          {t(eqBypassed ? "EQ \u00F8" : "EQ")}
        </button>
        <button
          className="tb-btn"
          data-on={settingsOpen}
          onClick={toggleSettings}
          title={t("Settings · appearance, engine source, MIDI bank")}
          aria-label={t("Settings")}
        >
          <IconGear size={13} />
        </button>
        <button
          className="tb-btn"
          data-on={shortcutsOpen}
          onClick={toggleShortcuts}
          title={t("Keyboard shortcuts (?)")}
          aria-label={t("Shortcuts")}
        >
          <IconKeys size={13} />
        </button>

        {/* macOS draws its own traffic lights (the title bar leaves 78 px for
            them); on Windows and Linux the window has no decorations at all,
            so without these three it can only be closed with Alt+F4. */}
        {t(!IS_MAC && (
          <div className="tb-winctl">
            <button
              className="tb-win"
              onClick={minimizeWindow}
              title={t("Minimise")}
              aria-label={t("Minimise")}
            >
              <IconMinimize size={12} />
            </button>
            <button
              className="tb-win"
              onClick={toggleMaximizeWindow}
              title={t("Maximise / restore")}
              aria-label={t("Maximise or restore")}
            >
              <IconMaximize size={12} />
            </button>
            <button
              className="tb-win danger"
              onClick={closeWindow}
              title={t("Close")}
              aria-label={t("Close")}
            >
              <IconClose size={12} />
            </button>
          </div>
        ))}
      </div>
    </header>
  );
}
