import { useEffect, useRef, useSyncExternalStore } from "react";

import { shortcutRows } from "../lib/keys";
import { legendRevision, subscribeLegends } from "../lib/layout";
import { useStore } from "../lib/store";
import { IconClose } from "./Icons";
import { isComposingReact } from "../lib/ime";

export default function ShortcutOverlay() {
  const setShortcutsOpen = useStore((s) => s.setShortcutsOpen);
  // Re-render when the real key legends become known: on macOS they arrive
  // only as the user presses the keys, so a card opened early would otherwise
  // keep claiming `[` on a keyboard that has no `[`.
  useSyncExternalStore(subscribeLegends, legendRevision, legendRevision);
  const rows = shortcutRows();
  const card = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const panel = card.current;
    if (!panel) return;
    const trigger = document.querySelector<HTMLElement>('[aria-label="Shortcuts"]');
    const siblings = Array.from(panel.parentElement?.parentElement?.children ?? [])
      .filter((el): el is HTMLElement => el instanceof HTMLElement && el !== panel.parentElement);
    const previous = siblings.map(el => el.inert);
    const previousHidden = siblings.map(el => el.getAttribute("aria-hidden"));
    siblings.forEach(el => { el.inert = true; });
    siblings.forEach(el => el.setAttribute("aria-hidden", "true"));
    const keepFocus = (event: FocusEvent) => {
      if (event.target instanceof Node && !panel.contains(event.target)) {
        panel.querySelector<HTMLButtonElement>("button")?.focus();
      }
    };
    document.addEventListener("focusin", keepFocus);
    panel.querySelector<HTMLButtonElement>("button")?.focus();
    return () => {
      siblings.forEach((el, i) => { el.inert = previous[i]; });
      siblings.forEach((el, i) => {
        if (previousHidden[i] == null) el.removeAttribute("aria-hidden");
        else el.setAttribute("aria-hidden", previousHidden[i]!);
      });
      document.removeEventListener("focusin", keepFocus);
      trigger?.focus({ preventScroll: true });
    };
  }, []);

  return (
    <div className="scrim" onClick={() => setShortcutsOpen(false)}>
      <div ref={card} className="shortcut-card" role="dialog" aria-modal="true" aria-labelledby="shortcuts-title"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (isComposingReact(e)) return;
          if (e.key === "Tab") {
            e.preventDefault();
            card.current?.querySelector<HTMLButtonElement>("button")?.focus();
          }
        }}>
        <div className="shortcut-head">
          <h2 id="shortcuts-title">Keyboard</h2>
          <button className="close-btn" aria-label="Close shortcuts" onClick={() => setShortcutsOpen(false)}><IconClose /></button>
        </div>
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
