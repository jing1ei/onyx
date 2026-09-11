import { t } from "../lib/i18n";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as api from "../lib/api";
import { useAudibleDeck } from "../lib/audible";
import { IS_MAC } from "../lib/dom";
import { ENTRY_MIME } from "../lib/drop";
import { blindLocked, useStore } from "../lib/store";
import { formatTime } from "../lib/format";
import type { Deck, PlaylistEntry } from "../lib/types";
import { IconPlus } from "./Icons";
// the archive and MIDI marks below (SPEC §18/§19) are styled there
import "../styles/settings.css";

/** The two decks, in the order the chips are drawn. */
const DECKS: Deck[] = ["a", "b"];

interface CtxState {
  x: number;
  y: number;
  entry: PlaylistEntry;
}

export default function Playlist({ onOpen }: { onOpen?: (path: string) => void } = {}) {
  const snapshot = useStore((s) => s.snapshot);
  const selectedId = useStore((s) => s.selectedId);
  const pendingPlayId = useStore((s) => s.pendingPlayId);
  const setSelectedId = useStore((s) => s.setSelectedId);
  const setPendingPlayId = useStore((s) => s.setPendingPlayId);
  const setSnapshot = useStore((s) => s.setSnapshot);
  const pushToast = useStore((s) => s.pushToast);

  const [ctx, setCtx] = useState<CtxState | null>(null);
  const [dragIndex, setDragIndex] = useState<number | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);
  const rowsRef = useRef<HTMLDivElement | null>(null);

  const playlist = snapshot?.playlist ?? [];
  const playingIdA = snapshot?.deckA.entryId ?? null;
  const playingIdB = snapshot?.deckB.entryId ?? null;
  /**
   * What the row highlight means: *this is what you are hearing*.
   *
   * It used to mean "some deck holds this file", which with A/B switched off
   * still lit two rows — deck B keeps its material when the comparison is
   * turned off, and the engine forces the audible deck back to A
   * (`crates/onyx-core/src/engine.rs` `set_ab_enabled`). Two active rows for
   * one audible deck. With A/B on, both loaded rows stay marked, but only the
   * audible one gets `data-playing`; the silent deck reads as `data-loaded`,
   * in its own deck accent.
   *
   * Which deck that is comes from the frame stream (`lib/audible.ts`): read
   * from the snapshot it was stale for exactly as long as nothing else pushed
   * one, so switching deck from a lane or the A|B buttons left the highlight
   * behind on the deck you had stopped hearing.
   */
  const abEnabled = snapshot?.ab.enabled ?? false;
  const activeDeck = useAudibleDeck();
  const blindActive = snapshot?.blind.active ?? false;
  const audibleId = activeDeck === "a" ? playingIdA : playingIdB;

  const total = useMemo(
    () => playlist.reduce((acc, e) => acc + (e.durationSecs || 0), 0),
    [playlist],
  );

  /** The archives this playlist was opened from, in first-seen order (§19). */
  const archives = useMemo(
    () => [...new Set(playlist.map((e) => e.archive).filter((a): a is string => !!a))],
    [playlist],
  );
  /**
   * A whole playlist out of one zip says so once, in the head. The row mark is
   * for the case where it actually distinguishes rows — two archives, or an
   * archive mixed with files off the disk — otherwise it is the same chip
   * repeated down the column, competing with every title for the eye.
   */
  const markRows = archives.length > 1 || (archives.length === 1 && playlist.some((e) => !e.archive));

  const fail = useCallback(
    (err: unknown) => pushToast("error", api.errorMessage(err)),
    [pushToast],
  );

  /** One tap = play. Highlight instantly, resolve the snapshot afterwards. */
  const play = useCallback(
    (entry: PlaylistEntry) => {
      // loading a row replaces deck A, which would invalidate a running test
      if (blindLocked("Loading a track")) return;
      setSelectedId(entry.id);
      if (onOpen) { onOpen(entry.path); return; }
      setPendingPlayId(entry.id);
      api
        .playlistPlayEntry(entry.id)
        .then((snap) => setSnapshot(snap))
        .catch((err) => {
          setPendingPlayId(null);
          fail(err);
        });
    },
    [fail, onOpen, setPendingPlayId, setSelectedId, setSnapshot],
  );

  /**
   * Put a row on a deck. Reached from four places (SPEC §2.8): the `A` / `B`
   * chips on the row, `⇧A` / `⇧B`, a drag onto a waveform lane, and the row
   * context menu. Assigning also selects the row, so the chips stay visible
   * afterwards and the keyboard route keeps working on the track you just
   * touched.
   */
  const assign = useCallback(
    (entry: PlaylistEntry, deck: Deck) => {
      if (onOpen) return;
      if (blindLocked("Assigning a deck")) return;
      setSelectedId(entry.id);
      api
        .abAssign(deck, entry.id)
        .then((snap) => setSnapshot(snap))
        .catch(fail);
    },
    [fail, onOpen, setSelectedId, setSnapshot],
  );

  const remove = useCallback(
    (entry: PlaylistEntry) => {
      if (blindLocked("Removing a track")) return;
      api
        .playlistRemove(entry.id)
        .then((snap) => setSnapshot(snap))
        .catch(fail);
    },
    [fail, setSnapshot],
  );

  useEffect(() => {
    if (!ctx) return;
    const close = (): void => setCtx(null);
    window.addEventListener("pointerdown", close, { capture: true });
    window.addEventListener("blur", close);
    return () => {
      window.removeEventListener("pointerdown", close, { capture: true });
      window.removeEventListener("blur", close);
    };
  }, [ctx]);

  const onDrop = useCallback(
    (to: number) => {
      const from = dragIndex;
      setDragIndex(null);
      setDropIndex(null);
      if (from == null || from === to) return;
      api
        .playlistMove(from, from < to ? to - 1 : to)
        .then((snap) => setSnapshot(snap))
        .catch(fail);
    },
    [dragIndex, fail, setSnapshot],
  );

  if (playlist.length === 0) {
    return (
      <section className="playlist">
        <div className="pl-head">
          <span className="label">{t("Playlist")}</span>
        </div>
        <div />
        <div className="pl-empty">
          <strong>{t("Nothing queued")}</strong>
          <span>{t("Drop audio files anywhere in the window, or open a folder of masters.")}</span>
          <button
            className="ghost-btn"
            onClick={() => api.pickAndOpenFiles(true).then(setSnapshot).catch(fail)}
          >
            <IconPlus size={10} />{t("Open files")}</button>
        </div>
      </section>
    );
  }

  return (
    <section className="playlist">
      <div className="pl-head">
        <span className="label">{t("Playlist")}</span>
        <span className="pl-count num">
          {playlist.length} {t(playlist.length === 1 ? 'track' : 'tracks')} · {formatTime(total)}
        </span>
        {t(archives.length > 0 && (
          <i className="pl-tag arc" title={t(`Opened from ${archives.join(", ")}`)}>
            {t(archives.length === 1 ? archives[0] : `${archives.length} archives`)}
          </i>
        ))}
        <div className="pl-actions">
          <button
            className="ghost-btn"
            onClick={() => api.pickAndOpenFiles(false).then(setSnapshot).catch(fail)}
            title={t("Add files (⌘⇧O)")}
          >{t("Add")}</button>
          <button
            className="ghost-btn"
            onClick={() => {
              if (blindLocked("Clearing the playlist")) return;
              api.playlistClear().then(setSnapshot).catch(fail);
            }}
            title={t("Clear playlist (⌘K)")}
          >{t("Clear")}</button>
        </div>
      </div>

      {/* Five columns, and the header cells carry the same class as the body
          cells they head: alignment is described once, and a column that a
          narrow window drops (`app.css`, "Narrow windows") disappears from the
          header and the rows with one rule. */}
      <div className="pl-cols pl-grid">
        <span className="pl-i">#</span>
        <span className="pl-title">{t("Title")}</span>
        <span className="pl-artist">{t("Artist")}</span>
        <span className="pl-deck">{t("Deck")}</span>
        <span className="pl-dur">{t("Time")}</span>
      </div>

      <div className="pl-rows" ref={rowsRef}>
        {t(playlist.map((entry, index) => {
          // Only decks that are part of what you hear get a badge: with A/B off
          // a stale "B" on another row is the same false "two decks are live"
          // claim as the old double highlight.
          const onDeckA = entry.id === playingIdA;
          const onDeckB = abEnabled && entry.id === playingIdB;
          const deck = onDeckA ? "a" : onDeckB ? "b" : null;
          // A blind test must not say which slot is audible (SPEC §7), so while
          // one runs both loaded rows are marked identically.
          const audible = blindActive ? onDeckA || onDeckB : entry.id === audibleId;
          const loaded = (onDeckA || onDeckB) && !audible;
          return (
            <div
              key={entry.id}
              className="pl-row pl-grid"
              data-playing={audible}
              data-loaded={loaded}
              data-deck={deck ?? undefined}
              data-selected={entry.id === selectedId}
              data-pending={entry.id === pendingPlayId}
              data-missing={entry.missing}
              data-dragging={dragIndex === index}
              data-drop={dropIndex === index ? "above" : dropIndex === index + 1 ? "below" : undefined}
              draggable
              onDragStart={(e) => {
                setDragIndex(index);
                // Two consumers, two effects: inside the list this is a reorder
                // (`move`), and on a waveform lane it is an assignment to that
                // deck (SPEC §2.8). The lane can only see `types` during
                // `dragover`, so the entry id travels as its own MIME type —
                // `text/plain` carries it too, but nothing keys off that.
                e.dataTransfer.effectAllowed = "copyMove";
                e.dataTransfer.setData(ENTRY_MIME, String(entry.id));
                e.dataTransfer.setData("text/plain", String(entry.id));
              }}
              onDragOver={(e) => {
                if (dragIndex == null) return;
                e.preventDefault();
                const rect = e.currentTarget.getBoundingClientRect();
                const below = e.clientY - rect.top > rect.height / 2;
                setDropIndex(index + (below ? 1 : 0));
              }}
              onDragEnd={() => {
                setDragIndex(null);
                setDropIndex(null);
              }}
              onDrop={(e) => {
                e.preventDefault();
                onDrop(dropIndex ?? index);
              }}
              onClick={() => play(entry)}
              onContextMenu={(e) => {
                e.preventDefault();
                setSelectedId(entry.id);
                setCtx({ x: e.clientX, y: e.clientY, entry });
              }}
            >
              <span className="pl-i num">{t(index + 1)}</span>
              <span className="pl-title">
                {entry.title ?? entry.fileName}
                {/* Where the row came from and what it is, in the order a
                    reader needs them: a zip entry's path on disk is a
                    temporary extraction directory and says nothing, so the
                    archive it was opened from is named instead (SPEC §19),
                    and a .mid row says up front that what you will hear is a
                    synthesised General MIDI rendering, not a recording
                    (SPEC §18). */}
                {t(entry.synthBank && (
                  <i className="pl-tag" title={t(`Rendered through ${entry.synthBank}`)}>{t("MIDI · GM")}</i>
                ))}
                {t(entry.archive && markRows && (
                  <i className="pl-tag arc" title={t(`From the archive ${entry.archive}`)}>
                    {t(entry.archive)}
                  </i>
                ))}
                {entry.title && <span className="pl-sub">{entry.fileName}</span>}
              </span>
              <span className="pl-artist">{entry.artist ?? "\u2014"}</span>
              {/* The deck column is the assignment control, not a read-out.
                  A right-click menu was the only route to deck B, and a menu
                  nothing hints at is a feature that does not exist: a user on
                  the shipped build reported deck B as unassignable. So both
                  chips live on every row — the one that holds the row is
                  always lit, the other appears on hover, on the selected row
                  and whenever either chip has keyboard focus. Never
                  hover-only: a trackpad is not the only way into this app. */}
              <span className="pl-deck">
                <span className="pl-chips" role="group" aria-label={t("Assign this track to a deck")}>
                  {t(DECKS.map((d) => {
                    const on = deck === d;
                    const label = d.toUpperCase();
                    return (
                      <button
                        key={d}
                        className="deck-chip"
                        disabled={!!onOpen}
                        data-deck={d}
                        data-on={on}
                        aria-pressed={on}
                        draggable={false}
                        title={t(on
                            ? `On deck ${label} \u00B7 click to reload it (\u21E7${label})`
                            : `Assign to deck ${label} (\u21E7${label})`)}
                        onPointerDown={(e) => e.stopPropagation()}
                        onClick={(e) => {
                          // the row underneath means "play now" (SPEC §2.3)
                          e.stopPropagation();
                          assign(entry, d);
                        }}
                      >
                        {t(label)}
                      </button>
                    );
                  }))}
                </span>
              </span>
              <span className="pl-dur num">{t(formatTime(entry.durationSecs))}</span>
            </div>
          );
        }))}
      </div>

      {t(ctx && (
        <div
          className="ctx-menu"
          style={{ left: Math.min(ctx.x, window.innerWidth - 224), top: Math.min(ctx.y, window.innerHeight - 210) }}
          onPointerDown={(e) => e.stopPropagation()}
        >
          <button className="ctx-item" onClick={() => { play(ctx.entry); setCtx(null); }}>{t(onOpen ? "打开音频" : "Play now")}<small>{t("Click")}</small>
          </button>
          <div className="ctx-sep" />
          <button className="ctx-item" disabled={!!onOpen} onClick={() => { assign(ctx.entry, "a"); setCtx(null); }}>{t("Assign to deck A")}<small>{t("\u21E7A")}</small>
          </button>
          <button className="ctx-item" disabled={!!onOpen} onClick={() => { assign(ctx.entry, "b"); setCtx(null); }}>{t("Assign to deck B")}<small>{t("\u21E7B")}</small>
          </button>
          <div className="ctx-sep" />
          <button
            className="ctx-item"
            onClick={() => {
              api.revealInFinder(ctx.entry.path).catch(fail);
              setCtx(null);
            }}
          >
            {t(IS_MAC ? "Reveal in Finder" : "Show in Explorer")}
          </button>
          <button
            className="ctx-item"
            data-danger="true"
            onClick={() => { remove(ctx.entry); setCtx(null); }}
          >{t("Remove")}<small>{t("Del")}</small>
          </button>
        </div>
      ))}
    </section>
  );
}
