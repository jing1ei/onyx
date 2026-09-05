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
  const rows = shortcutRows();

  return (
    <div className="scrim" onClick={() => setShortcutsOpen(false)}>
      <div className="shortcut-card" onClick={(e) => e.stopPropagation()}>
        <h2>Keyboard</h2>
        <div className="shortcut-grid">
          {rows.map((row, i) => (
            <div className="shortcut-row" key={`${row.action}-${i}`}>
              <span className="keys num">{row.keys}</span>
              <span className="act">{row.action}</span>
            </div>
          ))}
        </div>
        <div className="shortcut-foot">Esc or ? to close</div>
      </div>
    </div>
  );
}
