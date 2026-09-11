import { t } from "../lib/i18n";
import { useSyncExternalStore } from "react";

import { shortcutRows } from "../lib/keys";
import { legendRevision, subscribeLegends } from "../lib/layout";
import { useStore } from "../lib/store";

export default function ShortcutOverlay() {
  const setShortcutsOpen = useStore((s) => s.setShortcutsOpen);
  // Re-render when the real key legends become known: on macOS they arrive
  // only as the user presses the keys, so a card opened early would otherwise
  // keep claiming `[` on a keyboard that has no `[`.
  useSyncExternalStore(subscribeLegends, legendRevision, legendRevision);
  const editing = useStore(s => s.editorActive);
  const rows = editing ? [
    { keys: 'Space', action: 'Play / pause' }, { keys: 'R / T', action: 'Zoom out / in' },
    { keys: 'Ctrl + wheel', action: 'Zoom at pointer' }, { keys: 'Ctrl+T', action: 'Trim to selection' },
    { keys: 'Delete', action: 'Delete selection' }, { keys: 'Ctrl+Z', action: 'Undo' },
    { keys: 'Ctrl+Shift+Z', action: 'Redo' }, { keys: 'Ctrl+S', action: 'Overwrite original' },
    { keys: 'Ctrl+O', action: 'Open files' },
  ] : shortcutRows();

  return (
    <div className="scrim" onClick={() => setShortcutsOpen(false)}>
      <div className="shortcut-card" onClick={(e) => e.stopPropagation()}>
        <h2>{t("Keyboard")}</h2>
        <div className="shortcut-grid">
          {t(rows.map((row, i) => (
            <div className="shortcut-row" key={`${row.action}-${i}`}>
              <span className="keys num">{t(row.keys)}</span>
              <span className="act">{t(row.action)}</span>
            </div>
          )))}
        </div>
        <div className="shortcut-foot">{t("Esc or ? to close")}</div>
      </div>
    </div>
  );
}
