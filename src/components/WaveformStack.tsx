import { t } from "../lib/i18n";
import { useCallback, useEffect, useRef } from "react";
import * as api from "../lib/api";
import { alignRef, setOffset } from "../lib/align";
import { audibleDeck, useAudibleDeck } from "../lib/audible";
import { carriesEntry, carriesFiles, dropFilesOnDeck, entryIdFromTransfer } from "../lib/drop";
import { frameRef } from "../lib/frame";
import { blindLocked, useStore } from "../lib/store";
import type { Deck } from "../lib/types";
import MeterCluster from "./MeterCluster";
import WaveformLane, { type LaneInteraction } from "./WaveformLane";

type DragMode = "seek" | "loop" | "align" | null;

export default function WaveformStack() {
  const snapshot = useStore((s) => s.snapshot);
  const waveforms = useStore((s) => s.waveforms);
  const pushToast = useStore((s) => s.pushToast);
  const setSnapshot = useStore((s) => s.setSnapshot);
  /* The lane a drag is currently over, so it can light up as a drop target.
     In the store rather than in this component because inside Tauri a *file*
     drag never reaches the DOM: `App.tsx` hit-tests the one native event and
     lights the lane from there (SPEC §2.8). */
  const dropDeck = useStore((s) => s.dropDeck);
  const setDropDeck = useStore((s) => s.setDropDeck);

  const interaction = useRef<LaneInteraction>({
    hoverRatio: null,
    hoverDeck: null,
    dragLoop: null,
  });
  const dragRef = useRef<{
    mode: DragMode;
    rect: DOMRect | null;
    start: number;
    startX: number;
    startFrames: number;
  }>({ mode: null, rect: null, start: 0, startX: 0, startFrames: 0 });
  /** one error per scrub, not one per pointer event */
  const seekFailed = useRef(false);

  const abEnabled = snapshot?.ab.enabled ?? false;
  const blindActive = snapshot?.blind.active ?? false;
  /* The audible deck comes off the frame stream, not the snapshot: `ab_select`
     emits no state event, so a lane click switched the engine and left this
     value — and with it the AUDIBLE badge and the lane's active styling — on
     the previous deck (`lib/audible.ts`). */
  const activeDeck = useAudibleDeck();
  const deckA = snapshot?.deckA ?? null;
  const deckB = snapshot?.deckB ?? null;
  const shared = Math.max(deckA?.durationSecs ?? 0, abEnabled ? deckB?.durationSecs ?? 0 : 0, 0.001);

  const ratioFromEvent = (clientX: number, rect: DOMRect): number =>
    Math.max(0, Math.min(1, (clientX - rect.left) / Math.max(1, rect.width)));

  /**
   * The lanes, the playhead and every hit test share one timeline: deck A is
   * the reference (SPEC §11) and the longer deck sets the span. v1 mixed
   * this with `transport.durationSecs`, so with two decks of different lengths
   * the playhead sat at the wrong place on the drawn material.
   */
  const duration = useCallback(() => shared, [shared]);

  const onPointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>, deck: Deck) => {
      if (e.button !== 0) return;
      // One drag record is shared by both lanes. A second pointer (a touch, a
      // pen) pressing the other lane mid-drag used to overwrite it, and an
      // Alt-drag replaced that way never cleared `alignRef.dragging` — which
      // blocks every later engine offset sync for the rest of the session.
      if (dragRef.current.mode) return;
      const el = e.currentTarget;
      const rect = el.getBoundingClientRect();
      const ratio = ratioFromEvent(e.clientX, rect);
      const secs = ratio * duration();
      el.setPointerCapture(e.pointerId);
      seekFailed.current = false;
      interaction.current.hoverRatio = ratio;
      interaction.current.hoverDeck = deck;

      /* Alt on lane B slides the A/B offset instead of seeking (SPEC §11).
         Checked before anything else so it never also seeks or switches deck. */
      if (e.altKey && deck === "b" && abEnabled && !blindActive) {
        dragRef.current = {
          mode: "align",
          rect,
          start: secs,
          startX: e.clientX,
          startFrames: alignRef.frames,
        };
        alignRef.dragging = true;
        alignRef.dragX = e.clientX - rect.left;
        return;
      }

      // Clicking a lane makes that deck audible — but never during a blind
      // test, where the lane is a masked stand-in and the engine ignores the
      // command anyway. The comparison is against the *engine's* deck
      // (`audibleDeck()`), not a rendered one: a switch does not push a
      // snapshot, so a second click on the other lane used to compare against
      // a deck that had already moved and quietly did nothing at all.
      if (abEnabled && !blindActive && deck !== audibleDeck()) {
        void api.abSelect(deck).catch((err) => pushToast("error", api.errorMessage(err)));
      }

      if (e.shiftKey) {
        dragRef.current = { mode: "loop", rect, start: secs, startX: e.clientX, startFrames: 0 };
        interaction.current.dragLoop = [secs, secs];
      } else {
        dragRef.current = { mode: "seek", rect, start: secs, startX: e.clientX, startFrames: 0 };
        void api.transportSeek(secs).catch((err) => pushToast("error", api.errorMessage(err)));
      }
    },
    [abEnabled, blindActive, duration, pushToast],
  );

  const finishDrag = useCallback(() => {
    const mode = dragRef.current.mode;
    const region = interaction.current.dragLoop;
    dragRef.current = { mode: null, rect: null, start: 0, startX: 0, startFrames: 0 };

    if (mode === "align") {
      alignRef.dragging = false;
      alignRef.dragX = null;
      return;
    }
    if (mode !== "loop") return;
    interaction.current.dragLoop = null;
    if (!region) return;
    const from = Math.min(region[0], region[1]);
    const to = Math.max(region[0], region[1]);
    if (to - from < 0.05) {
      void api.setLoopRegion(null).catch((err) => pushToast("error", api.errorMessage(err)));
      return;
    }
    void api.setLoopRegion([from, to]).catch((err) => pushToast("error", api.errorMessage(err)));
  }, [pushToast]);

  const onPointerMove = useCallback(
    (e: React.PointerEvent<HTMLDivElement>, deck: Deck) => {
      // A drag whose pointerup went to another window (Cmd-Tab, a system dialog)
      // would otherwise still own the pointer: hovering the lane afterwards
      // kept seeking, or kept sliding the A/B offset, with no button held.
      if (dragRef.current.mode && e.buttons === 0) finishDrag();

      const rect = dragRef.current.rect ?? e.currentTarget.getBoundingClientRect();
      const ratio = ratioFromEvent(e.clientX, rect);
      interaction.current.hoverRatio = ratio;
      if (!dragRef.current.mode) interaction.current.hoverDeck = deck;
      const mode = dragRef.current.mode;
      if (!mode) return;

      if (mode === "align") {
        const rate = frameRef.current?.transport.engineSampleRate ?? 48000;
        const dxSecs = ((e.clientX - dragRef.current.startX) / Math.max(1, rect.width)) * duration();
        // dragging the material right means B starts later: a negative offset
        alignRef.dragX = e.clientX - rect.left;
        setOffset(dragRef.current.startFrames - Math.round(dxSecs * rate), rate);
        return;
      }

      const secs = ratio * duration();
      if (mode === "loop") {
        interaction.current.dragLoop = [dragRef.current.start, secs];
      } else {
        // A scrub fires a seek per pointer event; toasting each rejection would
        // bury the screen, but swallowing them all hid a dead transport, so the
        // first failure of a drag is reported and the rest are dropped.
        void api.transportSeek(secs).catch((err) => {
          if (seekFailed.current) return;
          seekFailed.current = true;
          pushToast("error", api.errorMessage(err));
        });
      }
    },
    [duration, finishDrag, pushToast],
  );

  useEffect(() => {
    const up = (): void => finishDrag();
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
    // losing focus mid-drag never delivers a pointerup; without this an Alt
    // drag stays armed and the loop/align ghost stays painted
    window.addEventListener("blur", up);
    return () => {
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
      window.removeEventListener("blur", up);
    };
  }, [finishDrag]);

  const onPointerLeave = useCallback(() => {
    if (!dragRef.current.mode) {
      interaction.current.hoverRatio = null;
      interaction.current.hoverDeck = null;
    }
  }, []);

  /* ── drop a track onto a lane = assign it to that deck (SPEC §2.8) ──────
     The gesture an engineer reaches for first, and until now the lanes had no
     drop handling at all: the drag fell through to the window, which appends
     to the playlist, so dragging a track onto lane B silently did nothing that
     looked like an assignment. Two payloads are accepted — a playlist row
     (`ENTRY_MIME`) and OS files — and both end at the same `ab_assign`. */

  const laneAccepts = useCallback(
    (e: React.DragEvent<HTMLDivElement>): boolean => {
      if (blindActive) return false; // the lane is a masked stand-in (SPEC §7)
      return carriesEntry(e.dataTransfer) || carriesFiles(e.dataTransfer);
    },
    [blindActive],
  );

  const onLaneDragOver = useCallback(
    (e: React.DragEvent<HTMLDivElement>, deck: Deck) => {
      if (!laneAccepts(e)) return;
      // Claim the drag: `preventDefault` makes this a drop target at all, and
      // stopping propagation keeps the window-level "drop = append" handler
      // out of a gesture that has a more specific meaning here.
      e.preventDefault();
      e.stopPropagation();
      e.dataTransfer.dropEffect = carriesFiles(e.dataTransfer) ? "copy" : "move";
      // `dragover` fires continuously; only a change is worth a render
      setDropDeck(deck);
    },
    [laneAccepts],
  );

  const onLaneDragLeave = useCallback((e: React.DragEvent<HTMLDivElement>, deck: Deck) => {
    // `dragleave` also fires when the pointer crosses into a child of the lane
    const next = e.relatedTarget;
    if (next instanceof Node && e.currentTarget.contains(next)) return;
    if (useStore.getState().dropDeck === deck) setDropDeck(null);
  }, [setDropDeck]);

  const onLaneDrop = useCallback(
    (e: React.DragEvent<HTMLDivElement>, deck: Deck) => {
      if (!laneAccepts(e)) return;
      e.preventDefault();
      e.stopPropagation();
      setDropDeck(null);
      const fail = (err: unknown): void => pushToast("error", api.errorMessage(err));
      if (blindLocked("Assigning a deck")) return;

      const id = entryIdFromTransfer(e.dataTransfer);
      if (id != null) {
        void api.abAssign(deck, id).then(setSnapshot).catch(fail);
        return;
      }
      // A file straight off the desktop: append it, then put it on this deck.
      // In Tauri this branch is unreachable — the native drag-drop handler
      // takes files before the DOM sees them, so `App.tsx` hit-tests the drop
      // position instead — but it is the whole gesture in the browser preview.
      const paths = Array.from(e.dataTransfer.files ?? []).map((f) => f.name);
      if (!paths.length) return;
      void dropFilesOnDeck(paths, deck).then(setSnapshot).catch(fail);
    },
    [laneAccepts, pushToast, setDropDeck, setSnapshot],
  );

  /* A drag that ends anywhere else (Esc, or a drop on another window) never
     reaches the lane's own handlers, and a lane left lit would claim to be a
     target for a gesture that is over. */
  useEffect(() => {
    if (dropDeck == null) return;
    const clear = (): void => setDropDeck(null);
    window.addEventListener("dragend", clear);
    window.addEventListener("drop", clear);
    return () => {
      window.removeEventListener("dragend", clear);
      window.removeEventListener("drop", clear);
    };
  }, [dropDeck, setDropDeck]);

  const anyLoaded = (deckA?.loaded ?? false) || (deckB?.loaded ?? false);
  /* How many lanes are drawn, which is what their height depends on — one lane
     gets the whole region, two share it. It is an attribute rather than a
     number passed down because the height is a layout decision that changes
     with the window width (`app.css`, "Narrow windows"), and CSS is where the
     breakpoints live. */
  const lanes = !abEnabled || blindActive ? 1 : 2;

  return (
    <section className="wave-stack" data-ab={abEnabled} data-lanes={lanes}>
      {/* The lanes and the meters are one region: the meters read the audible
          deck the lanes are drawing, and they share the region's height. */}
      <div className="wave-lanes">
        {t(!anyLoaded && <div className="wave-empty">{t("drop audio to begin")}</div>)}
        {t(anyLoaded && blindActive && (
          // One anonymous lane, and it is *always* deck A's material — never the
          // audible deck's. Painting the active deck meant the picture changed
          // the instant you switched slots, and in ABX the X lane was a
          // pixel-for-pixel match of whichever of A/B it was: the answer, drawn
          // on screen. `data-deck` is pinned for the same reason; the attribute
          // alone was enough to read the mapping out of the DOM.
          <WaveformLane
            key="masked"
            deck="a"
            state={deckA}
            waveform={waveforms.a}
            audible
            masked
            abEnabled={abEnabled}
            spanRatio={1}
            timelineSecs={shared}
            interaction={interaction}
            onPointerDown={onPointerDown}
            onPointerMove={onPointerMove}
            onPointerLeave={onPointerLeave}
          />
        ))}
        {t(anyLoaded && !blindActive && (
          <>
            <WaveformLane
              deck="a"
              state={deckA}
              waveform={waveforms.a}
              audible={activeDeck === "a"}
              abEnabled={abEnabled}
              spanRatio={Math.min(1, (deckA?.durationSecs ?? 0) / shared || 1)}
              timelineSecs={shared}
              interaction={interaction}
              dropTarget={dropDeck === "a"}
              onPointerDown={onPointerDown}
              onPointerMove={onPointerMove}
              onPointerLeave={onPointerLeave}
              onLaneDragOver={onLaneDragOver}
              onLaneDragLeave={onLaneDragLeave}
              onLaneDrop={onLaneDrop}
            />
            {t(abEnabled && (
              <WaveformLane
                deck="b"
                state={deckB}
                waveform={waveforms.b}
                audible={activeDeck === "b"}
                abEnabled={abEnabled}
                spanRatio={Math.min(1, (deckB?.durationSecs ?? 0) / shared || 1)}
                timelineSecs={shared}
                offsetAware
                interaction={interaction}
                dropTarget={dropDeck === "b"}
                onPointerDown={onPointerDown}
                onPointerMove={onPointerMove}
                onPointerLeave={onPointerLeave}
                onLaneDragOver={onLaneDragOver}
                onLaneDragLeave={onLaneDragLeave}
                onLaneDrop={onLaneDrop}
              />
            ))}
          </>
        ))}
      </div>

      <MeterCluster />
    </section>
  );
}
