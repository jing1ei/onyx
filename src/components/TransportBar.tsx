import { t } from "../lib/i18n";
import { useCallback, useEffect, useRef } from "react";
import * as api from "../lib/api";
import { setAttr, setStyle, setText } from "../lib/dom";
import { frameRef, useFrameEffect } from "../lib/frame";
import { blindLocked, useStore } from "../lib/store";
import { formatSignedDb, formatTime, formatVolumeDb } from "../lib/format";
import { IconBlind, IconLoop, IconNext, IconPause, IconPlay, IconPrev, IconVolume } from "./Icons";
import { MonitorControl } from "./MonitorMatrix";

const CROSSFADES = [0, 8, 25, 50];

export default function TransportBar() {
  const snapshot = useStore((s) => s.snapshot);
  const setSnapshot = useStore((s) => s.setSnapshot);
  const pushToast = useStore((s) => s.pushToast);
  const blindOpen = useStore((s) => s.blindOpen);
  const setBlindOpen = useStore((s) => s.setBlindOpen);

  const playBtn = useRef<HTMLButtonElement | null>(null);
  const posEl = useRef<HTMLSpanElement | null>(null);
  const durEl = useRef<HTMLSpanElement | null>(null);
  const loopBtn = useRef<HTMLButtonElement | null>(null);
  const muteBtn = useRef<HTMLButtonElement | null>(null);
  const volFill = useRef<HTMLDivElement | null>(null);
  const volKnob = useRef<HTMLDivElement | null>(null);
  const volDb = useRef<HTMLSpanElement | null>(null);
  const deckABtn = useRef<HTMLButtonElement | null>(null);
  const deckBBtn = useRef<HTMLButtonElement | null>(null);
  const trimEl = useRef<HTMLSpanElement | null>(null);
  const volDrag = useRef(false);
  const volTrack = useRef<HTMLDivElement | null>(null);

  const fail = useCallback((err: unknown) => pushToast("error", api.errorMessage(err)), [pushToast]);

  const ab = snapshot?.ab;
  const abEnabled = ab?.enabled ?? false;
  const blind = snapshot?.blind ?? null;
  const blindActive = blind?.active ?? false;
  const blindSlots = blindActive && blind ? blind.slots : [];
  const bothLoaded = (snapshot?.deckA.loaded ?? false) && (snapshot?.deckB.loaded ?? false);
  // level matching and time alignment live in the A/B rail above the transport
  const trims = {
    a: ab?.levelMatch.enabled ? ab.levelMatch.trimDbA : 0,
    b: ab?.levelMatch.enabled ? ab.levelMatch.trimDbB : 0,
  };
  const showTrim = (ab?.levelMatch.enabled ?? false) && bothLoaded && !blindActive;

  /* everything transport-shaped is painted from the frame ref, never state */
  useFrameEffect((frame) => {
    const t = frame?.transport;
    if (!t) return;
    setAttr(playBtn.current, "data-playing", String(t.playing));
    setText(posEl.current, formatTime(t.positionSecs));
    setText(durEl.current, formatTime(t.durationSecs));
    setAttr(loopBtn.current, "data-on", String(t.loopEnabled));
    setAttr(muteBtn.current, "data-muted", String(t.muted));
    setAttr(deckABtn.current, "data-on", String(t.activeDeck === "a"));
    setAttr(deckBBtn.current, "data-on", String(t.activeDeck === "b"));
    // The match trim follows the audible deck, so it belongs on the frame
    // path too — reading `frameRef` during render left it a frame stale.
    // While blind, the *text* has to go as well: `hidden` only hides it from
    // the eye, and −1.8 dB vs 0.0 dB names the audible deck to anyone who
    // opens the inspector.
    setText(
      trimEl.current,
      showTrim ? `${formatSignedDb(t.activeDeck === "b" ? trims.b : trims.a)} dB` : "",
    );
    if (!volDrag.current) {
      const pct = `${(t.volume * 100).toFixed(2)}%`;
      setStyle(volFill.current, "width", pct);
      setStyle(volKnob.current, "left", pct);
      setText(volDb.current, `${formatVolumeDb(t.muted ? 0 : t.volume)} dB`);
    }
  });

  const setVolumeFromEvent = useCallback(
    (clientX: number) => {
      const el = volTrack.current;
      if (!el) return;
      const rect = el.getBoundingClientRect();
      const v = Math.max(0, Math.min(1, (clientX - rect.left) / Math.max(1, rect.width)));
      const pct = `${(v * 100).toFixed(2)}%`;
      setStyle(volFill.current, "width", pct);
      setStyle(volKnob.current, "left", pct);
      setText(volDb.current, `${formatVolumeDb(v)} dB`);
      void api.setVolume(v).catch((err) => {
        // release the optimistic hold so the next frame repaints the truth
        volDrag.current = false;
        fail(err);
      });
    },
    [fail],
  );

  useEffect(() => {
    const move = (e: PointerEvent): void => {
      if (!volDrag.current) return;
      // No button down means the pointerup was delivered somewhere else (the
      // window lost focus mid-drag). Without this, every later mouse move over
      // the window set the volume from the cursor's x position.
      if (e.buttons === 0) {
        volDrag.current = false;
        return;
      }
      setVolumeFromEvent(e.clientX);
    };
    const up = (): void => {
      volDrag.current = false;
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
    window.addEventListener("blur", up);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
      window.removeEventListener("blur", up);
    };
  }, [setVolumeFromEvent]);

  return (
    <footer className="transport">
      <div className="tr-group">
        <button
          className="tr-btn"
          onClick={() => {
            // prev/next replace deck A, which is half of what the test compares
            if (blindLocked("Changing track")) return;
            api.playlistPrev().then(setSnapshot).catch(fail);
          }}
          title={t("Previous")}
          aria-label={t("Previous track")}
        >
          <IconPrev />
        </button>
        <button
          className="tr-btn primary"
          ref={playBtn}
          data-playing="false"
          onClick={() => api.transportToggle().catch(fail)}
          title={t("Play / pause (Space)")}
          aria-label={t("Play or pause")}
        >
          <span className="i-play">
            <IconPlay size={15} />
          </span>
          <span className="i-pause">
            <IconPause size={15} />
          </span>
        </button>
        <button
          className="tr-btn"
          onClick={() => {
            if (blindLocked("Changing track")) return;
            api.playlistNext().then(setSnapshot).catch(fail);
          }}
          title={t("Next")}
          aria-label={t("Next track")}
        >
          <IconNext />
        </button>
      </div>

      <div className="tr-time">
        <span className="tr-pos num" ref={posEl}>
          0:00
        </span>
        <span className="tr-sep">/</span>
        <span className="tr-dur num" ref={durEl}>
          0:00
        </span>
      </div>

      <button
        className="tr-toggle"
        ref={loopBtn}
        data-on="false"
        onClick={() => {
          const on = frameRef.current?.transport.loopEnabled ?? false;
          void api.setLoopEnabled(!on).catch(fail);
        }}
        title={t("Loop (L) — shift-drag the waveform to set a region")}
      >
        <IconLoop size={12} />
        {/* the word goes before the icon does when the window gets narrow */}
        <span className="tr-word">{t("Loop")}</span>
      </button>

      <MonitorControl />

      <div className="tr-spacer" />

      <div className="vol">
        <button
          className="vol-btn"
          ref={muteBtn}
          data-muted="false"
          onClick={() => {
            const muted = frameRef.current?.transport.muted ?? false;
            void api.setMuted(!muted).catch(fail);
          }}
          title={t("Mute (M)")}
          aria-label={t("Mute")}
        >
          <IconVolume size={14} />
        </button>
        <div
          className="vol-track"
          ref={volTrack}
          onPointerDown={(e) => {
            volDrag.current = true;
            setVolumeFromEvent(e.clientX);
          }}
          title={t("Volume (↑ / ↓)")}
        >
          <div className="vol-fill" ref={volFill} style={{ width: "80%" }} />
          <div className="vol-knob" ref={volKnob} style={{ left: "80%" }} />
        </div>
        <span className="vol-db num" ref={volDb}>{t("0.0 dB")}</span>
      </div>

      <div className="ab-cluster">
        <button
          className="tr-toggle"
          data-on={abEnabled}
          disabled={blindActive}
          onClick={() => api.abSetEnabled(!abEnabled).then(setSnapshot).catch(fail)}
          title={t(blindActive ? "Finish or abort the blind test first" : "Enable A/B comparison")}
        >{t("A/B")}</button>

        {t(blindActive ? (
          // identity stays hidden: these are blind slots, never decks, and they
          // are rendered in `blind.slots` order so the DOM leaks nothing either
          <div className="seg" data-blind="true">
            {t(blindSlots.map((slot) => (
              <button
                key={slot}
                data-on={blind?.currentSlot === slot}
                onClick={() => api.blindSwitch(slot).catch(fail)}
                title={t(`Slot ${slot.toUpperCase()}`)}
              >
                {t(slot.toUpperCase())}
              </button>
            )))}
          </div>
        ) : (
          <div className="seg">
            <button
              ref={deckABtn}
              data-deck="a"
              data-on="true"
              disabled={!abEnabled}
              onClick={() => api.abSelect("a").catch(fail)}
              title={t("Deck A (A)")}
            >{t("A")}</button>
            <button
              ref={deckBBtn}
              data-deck="b"
              data-on="false"
              disabled={!abEnabled}
              onClick={() => api.abSelect("b").catch(fail)}
              title={t("Deck B (B)")}
            >{t("B")}</button>
          </div>
        ))}

        <span className="tr-trim num" ref={trimEl} hidden={!showTrim} title={t("Level-match trim on the audible deck")} />

        <div className="xfade">
          <span className="label">{t("Xfade")}</span>
          <div className="xfade-opts">
            {t(CROSSFADES.map((ms) => (
              <button
                key={ms}
                className="num"
                data-on={Math.round(ab?.crossfadeMs ?? 0) === ms}
                disabled={!abEnabled || blindActive}
                onClick={() => api.abSetCrossfadeMs(ms).catch(fail)}
              >
                {t(ms)}
              </button>
            )))}
          </div>
        </div>

        <button
          className="tr-toggle blind-btn"
          data-tone="b"
          data-on={blindActive || blindOpen}
          disabled={!abEnabled || !bothLoaded}
          onClick={() => setBlindOpen(true)}
          title={t("Blind A/B or ABX test")}
        >
          <IconBlind size={12} />
          <span className="tr-word">{t("Blind")}</span>
        </button>
      </div>
    </footer>
  );
}
