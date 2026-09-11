import { t } from "../lib/i18n";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as api from "../lib/api";
import { setAttr, setStyle, setText } from "../lib/dom";
import { useFrameEffect } from "../lib/frame";
import { isComposingReact } from "../lib/ime";
import { useStore } from "../lib/store";
import { formatBytes, formatSampleRate } from "../lib/format";
import {
  applyAppearance,
  currentAppearance,
  DEFAULT_APPEARANCE,
  normaliseAppearance,
  parseHex,
  type Appearance,
  type SizeScale,
  type ThemeSetting,
} from "../lib/theme";
import { resetAppearanceEverywhere } from "../lib/themeio";
import type { AudioSourceState, CacheStats, DeviceInfo, SoundFontState } from "../lib/types";
import { IconClose } from "./Icons";
import ThemeEditor from "./ThemeEditor";
import "../styles/settings.css";

/**
 * The settings surface: appearance (SPEC §15), the engine source
 * (SPEC §16), the General MIDI bank (SPEC §18) and the loudness cache
 * (SPEC §8), in that order — the two the user came here for first, the two
 * that are consulted rarely last.
 *
 * It is dense on purpose: this is one 360 px column, not a preferences window
 * with tabs, and a mastering engineer changing a buffer size should not have
 * to hunt. What keeps it legible is that every group is one titled block of
 * label/control rows with a hairline between them, nothing is nested twice,
 * and anything that is *not* a choice — latency, engine rate, underruns — is a
 * read-out in the numeric font on the right-hand rail.
 *
 * Nothing here writes a colour or a font stack: appearance is handed to
 * `lib/theme.ts`, which owns the token layer, and the panel only ever passes
 * around the tokens it offers.
 *
 * Appearance has two faces (SPEC §20). **Theme code** is the default: the
 * document, in the same editor the theme window runs, because the workflow the
 * feature exists for is copy → hand to a model → paste back, and burying that
 * behind a tab would be burying the feature. **Simple** is the six pickers that
 * were here before — theme, accent, fonts, size — which still work, still
 * persist, and are still the fastest way to change one thing. A document in
 * force wins over both: the pickers set `appearance`, and `appearance` is what
 * the document's own `appearance` block overrides.
 */

/**
 * Elide the *head* of the cache path: the file name at the end is the useful
 * part. Done in characters rather than with `text-overflow`, which left a
 * ragged half-character of slack between the ellipsis and the file name; the
 * read-out is monospace and the field is capped at `PATH_CHARS`ch, so a
 * character budget and the box agree exactly.
 */
const PATH_CHARS = 36;

function elidePath(path: string): string {
  return path.length <= PATH_CHARS ? path : `\u2026${path.slice(-(PATH_CHARS - 1))}`;
}

/** stands in for a read-out that would identify the audible deck (SPEC §7) */
const HIDDEN = "\u00B7\u00B7\u00B7";

/* ── the curated lists (§15) ──────────────────────────────────────────────
   Tokens, not family names: `src/styles/tokens.css` maps each one to a stack
   made only of faces the platform already ships, so nothing is ever fetched —
   the CSP forbids remote origins and this is an offline tool. Adding an option
   here means adding the matching `:root[data-ui-font="…"]` block there. */

const ACCENTS: { hex: string; name: string }[] = [
  { hex: "#c9a227", name: "Champagne" },
  { hex: "#b0652a", name: "Bronze" },
  { hex: "#b1503c", name: "Terracotta" },
  { hex: "#7f9a6b", name: "Sage" },
  { hex: "#3f8f8a", name: "Verdigris" },
  { hex: "#7a72c9", name: "Iris" },
];

const UI_FONTS: { token: string; label: string }[] = [
  { token: "system", label: "System UI" },
  { token: "grotesk", label: "Grotesk \u00B7 Helvetica" },
  { token: "humanist", label: "Humanist \u00B7 Avenir" },
  { token: "neutral", label: "Neutral \u00B7 Inter" },
];

/** Monospaced only: the read-outs are tabular and must stay column-stable. */
const NUM_FONTS: { token: string; label: string }[] = [
  { token: "system-mono", label: "System mono" },
  { token: "sf-mono", label: "SF Mono" },
  { token: "menlo", label: "Menlo" },
  { token: "consolas", label: "Consolas" },
  { token: "courier", label: "Courier" },
];

const THEMES: { value: ThemeSetting; label: string }[] = [
  { value: "dark", label: "Dark" },
  { value: "light", label: "Light" },
  { value: "system", label: "Auto" },
];

const SCALES: { value: SizeScale; label: string }[] = [
  { value: "compact", label: "Compact" },
  { value: "normal", label: "Normal" },
  { value: "large", label: "Large" },
];

const sameAppearance = (a: Appearance, b: Appearance): boolean =>
  a.theme === b.theme &&
  a.accent === b.accent &&
  a.uiFont === b.uiFont &&
  a.numericFont === b.numericFont &&
  a.sizeScale === b.sizeScale;

/** `256 frames` at 96 kHz is `2.7 ms`. */
const latencyOf = (frames: number, rate: number): number => (frames / Math.max(1, rate)) * 1000;
const formatMs = (ms: number | null): string => (ms == null ? "\u2014" : `${ms.toFixed(1)} ms`);

export default function SettingsPanel() {
  const snapshot = useStore((s) => s.snapshot);
  const setSettingsOpen = useStore((s) => s.setSettingsOpen);
  const setSnapshot = useStore((s) => s.setSnapshot);
  const pushToast = useStore((s) => s.pushToast);

  const [cache, setCache] = useState<CacheStats | null>(null);
  const [cacheError, setCacheError] = useState<string | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);
  const [clearing, setClearing] = useState(false);
  const rateEl = useRef<HTMLSpanElement | null>(null);
  const underEl = useRef<HTMLSpanElement | null>(null);
  const transEl = useRef<HTMLSpanElement | null>(null);
  /** read from the frame loop, which sees this render's value (`useFrameEffect`) */
  const blindActive = snapshot?.blind.active ?? false;

  const fail = useCallback((err: unknown) => pushToast("error", api.errorMessage(err)), [pushToast]);

  /* ── appearance (§15) ─────────────────────────────────────────────── */

  const snapAppearance = snapshot?.appearance;
  const [look, setLook] = useState<Appearance>(() =>
    normaliseAppearance(snapAppearance ?? currentAppearance()),
  );
  const [hex, setHex] = useState<string>(() => look.accent);
  const [hexBad, setHexBad] = useState(false);

  // The engine is the authority: it validates, persists and broadcasts, and
  // the EQ window is listening to the same broadcast (SPEC §14).
  useEffect(() => {
    if (!snapAppearance) return;
    const next = normaliseAppearance(snapAppearance);
    setLook(next);
    setHex((prev) => (parseHex(prev) === next.accent ? prev : next.accent));
  }, [snapAppearance]);

  const modified = !sameAppearance(look, DEFAULT_APPEARANCE);

  /**
   * Apply live, then persist. The live half is `applyAppearance`, which is the
   * whole of theming as far as this panel is concerned; the persist half can
   * still refuse (a hand-typed hex that the front end let through), and if it
   * does the previous appearance is put back rather than left on screen as a
   * setting that will not survive a restart.
   */
  const change = useCallback(
    (patch: Partial<Appearance>) => {
      const previous = look;
      const next = normaliseAppearance({ ...look, ...patch });
      if (sameAppearance(previous, next)) return;
      setLook(next);
      applyAppearance(next);
      api
        .setAppearance(next)
        .then((confirmed) => {
          setLook(confirmed);
          applyAppearance(confirmed);
        })
        .catch((err) => {
          setLook(previous);
          applyAppearance(previous);
          setHex(previous.accent);
          fail(err);
        });
    },
    [fail, look],
  );

  /** A free hex is rejected *visibly*: the field goes red and nothing moves. */
  const commitHex = useCallback(
    (raw: string) => {
      const parsed = parseHex(raw);
      if (!parsed) {
        setHexBad(true);
        return;
      }
      setHexBad(false);
      setHex(parsed);
      change({ accent: parsed });
    },
    [change],
  );

  /* ── which face of Appearance (§20) ───────────────────────────────
     The code editor is the default because it is the feature; the pickers are
     one click away and unchanged. The choice is component state on purpose —
     it is a view preference for this panel, not something worth a settings-file
     migration and a broadcast. */

  const [lookTab, setLookTab] = useState<"code" | "simple">("code");
  const themeDoc = snapshot?.themeDoc ?? null;

  /** The editor in its own window: bigger, and survives an unreadable theme. */
  const openThemeWindow = useCallback(() => {
    api.themeWindowOpen().catch(fail);
  }, [fail]);

  /* ── engine source (§16) ──────────────────────────────────────────── */

  const [audio, setAudio] = useState<AudioSourceState | null>(null);
  const [audioError, setAudioError] = useState<string | null>(null);
  /** the host being *looked at*, which may not be the one in use */
  const [hostSel, setHostSel] = useState<string | null>(null);
  const [hostDevices, setHostDevices] = useState<DeviceInfo[] | null>(null);
  const [busy, setBusy] = useState(false);

  const loadAudio = useCallback(() => {
    api
      .audioSource()
      .then((state) => {
        setAudio(state);
        setAudioError(null);
        setHostSel(state.source?.hostId ?? state.hosts.find((h) => h.isDefault)?.id ?? null);
        setHostDevices(null);
      })
      .catch((err) => {
        // Enumeration that comes back empty is not a failure and is handled
        // below; this is the case where the call itself could not answer.
        setAudio(null);
        setAudioError(api.errorMessage(err));
      });
  }, []);

  useEffect(loadAudio, [loadAudio]);

  const hosts = audio?.hosts ?? [];
  const source = audio?.source ?? null;
  const inUseHost = source?.hostId ?? null;
  /** devices for the host on screen: the fetched list wins while browsing */
  const devices = hostDevices ?? (hostSel === inUseHost ? (audio?.devices ?? []) : []);
  const device = useMemo(
    () => devices.find((d) => d.name === (snapshot?.device.current ?? source?.deviceName)) ?? null,
    [devices, snapshot?.device.current, source?.deviceName],
  );

  /** One atomic change, one stream rebuild; the snapshot describes the result. */
  const applySource = useCallback(
    (change: Parameters<typeof api.audioSourceSet>[0]) => {
      setBusy(true);
      api
        .audioSourceSet(change)
        .then((snap) => {
          setSnapshot(snap);
          loadAudio();
        })
        .catch(fail)
        .finally(() => setBusy(false));
    },
    [fail, loadAudio, setSnapshot],
  );

  /**
   * Changing the host is two steps, because a host with nothing on it must not
   * cost the user the stream they are listening to: enumerate first, and only
   * rebuild if the new API has something to open.
   */
  const chooseHost = useCallback(
    (hostId: string) => {
      setHostSel(hostId);
      setHostDevices(null);
      if (hostId === inUseHost) return;
      api
        .audioDevices(hostId)
        .then((list) => {
          setHostDevices(list);
          if (list.length > 0) applySource({ hostId, systemDefaultDevice: true });
        })
        .catch((err) => {
          setHostDevices([]);
          fail(err);
        });
    },
    [applySource, fail, inUseHost],
  );

  /* ── the General MIDI bank (§18) ──────────────────────────────────── */

  const [bank, setBank] = useState<SoundFontState | null>(null);
  const [bankError, setBankError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api
      .soundfontGet()
      .then((state) => live && setBank(state))
      .catch((err) => live && setBankError(api.errorMessage(err)));
    return () => {
      live = false;
    };
  }, [snapshot?.soundfont]);

  const chooseBank = useCallback(() => {
    api
      .pickSoundfont()
      .then((picked) => {
        // `null` is a cancelled dialog, which is not a change and not an error.
        if (picked) {
          setBank(picked);
          setBankError(null);
        }
      })
      .catch((err) => {
        setBankError(api.errorMessage(err));
        fail(err);
      });
  }, [fail]);

  const useBundledBank = useCallback(() => {
    api
      .soundfontSet(null)
      .then((state) => {
        setBank(state);
        setBankError(null);
      })
      .catch(fail);
  }, [fail]);

  /* ── loudness cache (SPEC §8) ─────────────────────────────────────── */

  const loadCache = useCallback((live: () => boolean) => {
    api
      .cacheStats()
      .then((stats) => {
        if (!live()) return;
        setCache(stats);
        setCacheError(null);
      })
      .catch((err) => {
        if (!live()) return;
        setCache(null);
        setCacheError(api.errorMessage(err));
      });
  }, []);

  useEffect(() => {
    let live = true;
    loadCache(() => live);
    return () => {
      live = false;
    };
  }, [loadCache]);

  /* the confirm step disarms itself so a stale "confirm" can't be hit later */
  useEffect(() => {
    if (!confirmClear) return;
    const id = window.setTimeout(() => setConfirmClear(false), 5000);
    return () => window.clearTimeout(id);
  }, [confirmClear]);

  useFrameEffect((frame) => {
    const t = frame?.transport;
    // Same leak as the meter cluster's foot line: engine rate and bit-transparency
    // track the audible deck, and this panel can be open during a test.
    const hide = blindActive;
    setText(rateEl.current, hide ? HIDDEN : formatSampleRate(t?.engineSampleRate ?? 0));
    setText(underEl.current, String(t?.outputUnderruns ?? 0));
    setText(transEl.current, hide ? HIDDEN : t?.bitTransparent ? "yes" : "no");
    setStyle(
      transEl.current,
      "color",
      !hide && t?.bitTransparent ? "var(--m-safe)" : "var(--text-mid)",
    );
    setAttr(underEl.current, "data-tone", (t?.outputUnderruns ?? 0) > 0 ? "hot" : "");
  });

  const clearCache = useCallback(() => {
    setClearing(true);
    api
      .cacheClear()
      .then((stats) => {
        setCache(stats);
        setCacheError(null);
        pushToast("info", "Loudness cache cleared");
      })
      .catch((err) => {
        setCacheError(api.errorMessage(err));
        fail(err);
      })
      .finally(() => {
        setClearing(false);
        setConfirmClear(false);
      });
  }, [fail, pushToast]);

  const dev = snapshot?.device;
  const rate = dev?.engineSampleRate ?? source?.sampleRate ?? 0;
  const rates = device?.sampleRates ?? [];
  const buffers = device?.bufferFrames?.options ?? [];
  const following = dev?.followSourceRate ?? source?.followSourceRate ?? true;

  return (
    <div className="float-panel right settings">
      <div className="panel-head">
        <span className="panel-title">{t("Settings")}</span>
        <span className="spacer" />
        <button className="close-btn" onClick={() => setSettingsOpen(false)} aria-label={t("Close")}>
          <IconClose />
        </button>
      </div>

      {/* ── appearance (SPEC §15/§20) ─────────────────────────────── */}
      <div className="set-sec">
        <div className="set-sec-head">
          <span className="set-sec-title">{t("Appearance")}</span>
          {t((modified || themeDoc) && (
            <span className="set-chip" title={t("These settings differ from the defaults")}>{t("modified")}</span>
          ))}
          <span className="spacer" />
          <button
            className="ghost-btn"
            disabled={!modified && !themeDoc}
            // Not `change(DEFAULT_APPEARANCE)`: with a document in force that
            // would reset the six fields and leave the document painting over
            // them. This is the same escape hatch the menu item and
            // Ctrl/Cmd+Alt+Shift+R call, and it clears both.
            onClick={() => {
              setHexBad(false);
              setHex(DEFAULT_APPEARANCE.accent);
              resetAppearanceEverywhere();
            }}
            title={t("Back to obsidian, champagne, system fonts, normal size and no theme document")}
          >{t("Reset")}</button>
        </div>

        {/* Its own row: two labels and a Reset do not fit on the title line in
            a 360 px column, and a tab strip that wraps is worse than one that
            has a line to itself. */}
        <div className="set-tabs seg" role="tablist" aria-label={t("How to change the appearance")}>
          <button
            role="tab"
            aria-selected={lookTab === "code"}
            data-on={lookTab === "code"}
            onClick={() => setLookTab("code")}
            title={t("The theme document — copy it, hand it to an agent, paste the reply back")}
          >{t("Theme code")}</button>
          <button
            role="tab"
            aria-selected={lookTab === "simple"}
            data-on={lookTab === "simple"}
            onClick={() => setLookTab("simple")}
            title={t("Theme, accent, fonts and size, one control each")}
          >{t("Simple")}</button>
        </div>

        {t(lookTab === "code" ? (
          <div className="field col">
            <div className="set-row">
              <div>
                <div className="k">{t("Theme document")}</div>
                <div className="sub">{t("Copy default &rarr; paste into a chat &rarr; paste the reply back")}</div>
              </div>
              <button
                className="ghost-btn"
                onClick={openThemeWindow}
                title={t("Open the editor in its own window, which keeps working if a theme makes this one unreadable")}
              >{t("Open window")}</button>
            </div>
            <ThemeEditor compact />
          </div>
        ) : (
          <>
            <div className="field">
              <span className="k">{t("Theme")}</span>
              <div className="seg">
                {t(THEMES.map((theme) => (
                  <button
                    key={theme.value}
                    data-on={look.theme === theme.value}
                    onClick={() => change({ theme: theme.value })}
                  >
                    {t(theme.label)}
                  </button>
                )))}
              </div>
            </div>

            <div className="field col">
              <div className="set-row">
                <div>
                  <div className="k">{t("Accent")}</div>
                  <div className="sub">{t("Hover, pressed and dim states are derived from it")}</div>
                </div>
                <span className="set-swatch big" style={{ background: look.accent }} />
              </div>
              <div className="set-swatches">
                {t(ACCENTS.map((a) => (
                  <button
                    key={a.hex}
                    className="set-swatch"
                    style={{ background: a.hex }}
                    data-on={look.accent === a.hex}
                    title={t(`${a.name} \u00B7 ${a.hex}`)}
                    aria-label={t(a.name)}
                    aria-pressed={look.accent === a.hex}
                    onClick={() => {
                      setHexBad(false);
                      setHex(a.hex);
                      change({ accent: a.hex });
                    }}
                  />
                )))}
                {/* The free field. `maxLength` is generous on purpose: it has to
                    hold whatever was pasted by mistake so the rejection can quote
                    it back rather than quote a truncation of it. */}
                <input
                  className="set-hex num"
                  value={hex}
                  spellCheck={false}
                  maxLength={16}
                  aria-label={t("Accent colour, as hex")}
                  aria-invalid={hexBad}
                  data-bad={hexBad}
                  onChange={(e) => {
                    setHex(e.target.value);
                    if (hexBad && parseHex(e.target.value)) setHexBad(false);
                  }}
                  onKeyDown={(e) => {
                    /* Not while an IME is composing: that Enter commits the
                       candidate, and reading `value` then would judge a
                       half-composed string — "#a" — and reject it in a toast
                       the user never asked for. See `lib/ime.ts`. */
                    if (isComposingReact(e)) return;
                    if (e.key === "Enter") commitHex((e.target as HTMLInputElement).value);
                  }}
                  onBlur={(e) => commitHex(e.target.value)}
                />
              </div>
              {t(hexBad && (
                <div className="set-error">{t("&ldquo;")}{t(hex)}{t("&rdquo; is not a colour &mdash; use #rrggbb")}</div>
              ))}
            </div>

            <div className="field">
              <span className="k">{t("Interface font")}</span>
              <select
                className="select"
                data-set="ui-font"
                value={look.uiFont}
                onChange={(e) => change({ uiFont: e.target.value })}
              >
                {t(UI_FONTS.map((f) => (
                  <option key={f.token} value={f.token}>
                    {t(f.label)}
                  </option>
                )))}
              </select>
            </div>

            <div className="field col">
              <div className="set-row">
                <div>
                  <div className="k">{t("Read-out font")}</div>
                  <div className="sub">{t("Monospaced only &mdash; LUFS, timecode and dB are tabular")}</div>
                </div>
                <select
                  className="select"
                  data-set="num-font"
                  value={look.numericFont}
                  onChange={(e) => change({ numericFont: e.target.value })}
                >
                  {t(NUM_FONTS.map((f) => (
                    <option key={f.token} value={f.token}>
                      {t(f.label)}
                    </option>
                  )))}
                </select>
              </div>
              {/* Two rows of the same width: a proportional face would visibly
                  stagger them, which is the whole argument for this restriction. */}
              <div className="set-sample num">
                <span>{t("\u221214.2 LUFS")}</span>
                <span>0:41.70</span>
                <span>{t("96.0 kHz")}</span>
              </div>
              <div className="set-sample num">
                <span>{t("\u22128.7 LUFS")}</span>
                <span>1:08.05</span>
                <span>{t("44.1 kHz")}</span>
              </div>
            </div>

            <div className="field">
              <span className="k">{t("Size")}</span>
              <div className="seg">
                {t(SCALES.map((s) => (
                  <button
                    key={s.value}
                    data-on={look.sizeScale === s.value}
                    onClick={() => change({ sizeScale: s.value })}
                  >
                    {t(s.label)}
                  </button>
                )))}
              </div>
            </div>

            {/* A document in force overrides these controls wherever the two
                overlap, and saying so is cheaper than a user wondering why the
                accent swatch does nothing. */}
            {t(themeDoc && (
              <div className="set-note">{t("A theme document is in force. It overrides these where they overlap &mdash; edit it under")}<b>{t("Theme code")}</b>{t(", or Reset to clear it.")}</div>
            ))}
          </>
        ))}
      </div>

      {/* ── engine source (SPEC §16) ──────────────────────────────── */}
      <div className="set-sec">
        <div className="set-sec-head">
          <span className="set-sec-title">{t("Engine source")}</span>
          <span className="spacer" />
          {t(busy && <span className="set-chip quiet">{t("rebuilding")}</span>)}
        </div>

        {t(audioError && <div className="set-error">{t("Could not read the audio source: ")}{t(audioError)}</div>)}

        <div className="field">
          <div>
            <div className="k">{t("Audio API")}</div>
            <div className="sub">
              {t(hosts.length === 0 ? "no audio API available" : `${hosts.length} available`)}
            </div>
          </div>
          <select
            className="select"
            data-set="host"
            value={hostSel ?? ""}
            disabled={hosts.length === 0}
            onChange={(e) => chooseHost(e.target.value)}
          >
            {t(hosts.length === 0 && <option value="">{t("None")}</option>)}
            {t(hosts.map((h) => (
              <option key={h.id} value={h.id} disabled={!h.available}>
                {t(h.name)}
                {t(h.available ? "" : " (unavailable)")}
              </option>
            )))}
          </select>
        </div>

        <div className="field">
          <div>
            <div className="k">{t("Output device")}</div>
            <div className="sub">
              {t(devices.length === 0
                ? hostSel
                  ? "no output devices on this API"
                  : "nothing to enumerate"
                : dev?.followingSystemDefault
                  ? `system default \u00B7 ${dev.current ?? "\u2014"}`
                  : `${devices.length} available`)}
            </div>
          </div>
          <select
            className="select"
            data-set="device"
            value={dev?.followingSystemDefault ? "" : (dev?.current ?? "")}
            disabled={devices.length === 0 || hostSel !== inUseHost}
            onChange={(e) =>
              applySource(
                e.target.value === ""
                  ? { systemDefaultDevice: true }
                  : { deviceName: e.target.value },
              )
            }
          >
            <option value="">{t("System default")}</option>
            {t(devices.map((d) => (
              <option key={d.name} value={d.name}>
                {t(d.name)}
                {t(d.isDefault ? " (default)" : "")}
              </option>
            )))}
          </select>
        </div>

        <div className="field">
          <div>
            <div className="k">{t("Sample rate")}</div>
            <div className="sub">
              {t(following ? "follows deck A \u00B7 stays bit-transparent" : "fixed \u00B7 resamples")}
            </div>
          </div>
          <select
            className="select"
            data-set="rate"
            value={following ? "follow" : String(rate)}
            disabled={devices.length === 0}
            onChange={(e) =>
              applySource(
                e.target.value === "follow"
                  ? { followSourceRate: true }
                  : { followSourceRate: false, sampleRate: Number(e.target.value) },
              )
            }
          >
            <option value="follow">{t("Follow source")}</option>
            {t(rates.map((r) => (
              <option key={r} value={r}>
                {t(formatSampleRate(r))}
              </option>
            )))}
          </select>
        </div>

        <div className="field">
          <div>
            <div className="k">{t("Buffer size")}</div>
            <div className="sub">
              {t(devices.length === 0
                ? "no device to ask"
                : buffers.length === 0
                  ? "the driver chooses its own"
                  : `${buffers[0]}\u2013${buffers[buffers.length - 1]} frames on this device`)}
            </div>
          </div>
          {t(buffers.length === 0 ? (
            <span className="v num">{t(dev?.bufferFrames ?? "\u2014")}</span>
          ) : (
            <select
              className="select"
              data-set="buffer"
              value={String(dev?.bufferFrames ?? "")}
              onChange={(e) => applySource({ bufferFrames: Number(e.target.value) })}
            >
              {t(dev?.bufferFrames == null && <option value="">{t("Driver default")}</option>)}
              {t(buffers.map((n) => (
                <option key={n} value={n}>
                  {t(`${n} \u00B7 ${latencyOf(n, rate).toFixed(1)} ms`)}
                </option>
              )))}
            </select>
          ))}
        </div>

        {/* The number the buffer size is *for*: frames are what the driver
            takes, milliseconds are what a player hears, and this one follows
            the engine's rate rather than the device's nominal one. */}
        <div className="field">
          <span className="k">{t("Output latency")}</span>
          <span className="v num">{t(formatMs(dev?.latencyMs ?? null))}</span>
        </div>

        <div className="field">
          <span className="k">{t("Engine sample rate")}</span>
          <span className="v num" ref={rateEl}>{t("&mdash;")}</span>
        </div>

        <div className="field">
          <span className="k">{t("Output underruns")}</span>
          <span className="v num" ref={underEl} data-tone="">
            0
          </span>
        </div>

        <div className="field">
          <span className="k">{t("Bit-transparent")}</span>
          <span className="v num" ref={transEl}>{t("&mdash;")}</span>
        </div>

        <div className="set-note">{t("Bit-transparent means engine rate equals the source rate with EQ bypassed, unity volume and no match trim applied.")}</div>
      </div>

      {/* ── the General MIDI bank (SPEC §18) ──────────────────────── */}
      <div className="set-sec">
        <div className="set-sec-head">
          <span className="set-sec-title">{t("MIDI")}</span>
          <span className="spacer" />
          <span className="set-chip quiet">{t(bank?.bundled === false ? "user bank" : "bundled")}</span>
        </div>

        <div className="field col">
          <div className="set-row">
            <div>
              <div className="k">{t("General MIDI bank")}</div>
              <div className="sub">{t(".mid files are rendered through this SoundFont")}</div>
            </div>
          </div>
          <div className="set-bank">
            <span className="set-bank-name num" title={t(bank?.path ?? "bundled with Onyx")}>
              {t(bank ? bank.name : "\u2014")}
            </span>
            <button className="ghost-btn" onClick={chooseBank}>{t("Choose .sf2")}</button>
            <button className="ghost-btn" disabled={bank?.bundled !== false} onClick={useBundledBank}>{t("Bundled")}</button>
          </div>
          {t(bankError && <div className="set-error">{t(bankError)}</div>)}
        </div>
      </div>

      {/* ── loudness cache (SPEC §8) ─────────────────────────────────── */}
      <div className="set-sec">
        <div className="set-sec-head">
          <span className="set-sec-title">{t("Loudness cache")}</span>
          <span className="spacer" />
        </div>

        <div className="field">
          <span className="k">{t("Entries")}</span>
          <span className="v num">{t(cache ? cache.entries.toLocaleString() : "\u2014")}</span>
        </div>
        <div className="field">
          <span className="k">{t("On disk")}</span>
          <span className="v num">{t(cache ? formatBytes(cache.bytes) : "\u2014")}</span>
        </div>
        <div className="field cache-path">
          <span className="k">{t("File")}</span>
          <span className="v num path" title={t(cache?.path ?? "")}>
            {t(cache ? elidePath(cache.path) : "\u2014")}
          </span>
        </div>

        {t(cacheError && (
          <div className="set-error">{t("Cache unavailable: ")}{t(cacheError)}</div>
        ))}

        <div className="cache-actions">
          <span className="set-note">{t("Integrated LUFS, LRA and true peak for files you have already played. Waveform peaks are not cached.")}</span>
          {t(confirmClear ? (
            <div className="confirm-pair">
              <button className="ghost-btn" onClick={() => setConfirmClear(false)}>{t("Keep")}</button>
              <button
                className="ghost-btn danger"
                disabled={clearing}
                onClick={clearCache}
                title={t("Every entry is discarded; loudness is re-measured on the next play")}
              >
                {t(clearing ? "Clearing" : "Confirm")}
              </button>
            </div>
          ) : (
            <button
              className="ghost-btn"
              disabled={cache == null || cache.entries === 0}
              onClick={() => setConfirmClear(true)}
            >{t("Clear")}</button>
          ))}
        </div>
      </div>
    </div>
  );
}
