import { t } from "../lib/i18n";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  applyTheme,
  copyWithToast,
  currentThemeSource,
  defaultThemeText,
  resetAppearanceEverywhere,
  revertTheme,
  summarise,
  themeForAgent,
  validateTheme,
} from "../lib/themeio";
import { currentThemeText } from "../lib/theme";
import { dedentSelection, indentSelection } from "../lib/indent";
import { formatRatio } from "../lib/contrast";
import { isComposingReact } from "../lib/ime";
import { useStore } from "../lib/store";
import type { ParseOutcome, Problem } from "../lib/themedoc";
import "../styles/theme-editor.css";

/**
 * The theme editor — SPEC §20.
 *
 * One component, two homes: the editor window (`src/theme/ThemeWindow.tsx`),
 * where it fills the window, and the Appearance section of the settings panel,
 * where it is `compact` and 360 px wide. They are the same code because they
 * are the same feature, and a "quick paste box" that validated differently
 * from the real editor is exactly the kind of second implementation that ends
 * up disagreeing with the first.
 *
 * # What it does, and what it refuses to do
 *
 * It never applies text. It parses text into a document, and hands the
 * *document* to `theme.ts`. Everything the user can see here — the problems
 * with their line numbers, the "did you mean", the contrast findings — comes
 * from the same `parseTheme` call that decides whether Apply is allowed, so the
 * editor cannot show a green light for a theme the applier would refuse, or the
 * reverse.
 *
 * Validation runs on a debounce while typing, and again, from scratch, on
 * Apply. The debounce is for the display; the Apply-time parse is the one that
 * matters.
 */

/** Long enough not to fire mid-paste, short enough to feel like feedback. */
const VALIDATE_DEBOUNCE_MS = 220;

interface Props {
  /** the 360 px settings-panel form; the window uses the full one */
  compact?: boolean;
}

export default function ThemeEditor({ compact = false }: Props) {
  const snapshot = useStore((s) => s.snapshot);
  const pushToast = useStore((s) => s.pushToast);

  /** The text in the box. Never applied directly — see the module note. */
  const [text, setText] = useState<string>(() => currentThemeSource());
  const [outcome, setOutcome] = useState<ParseOutcome | null>(null);
  const [busy, setBusy] = useState(false);
  /** what is actually in force, so "unsaved changes" can be honest */
  const [appliedText, setAppliedText] = useState<string | null>(() => currentThemeText());
  const area = useRef<HTMLTextAreaElement | null>(null);

  /* The document in force can change under this window: another window applied
     one, or the escape hatch reset it. The snapshot is the authority, and
     adopting it here is what keeps the editor from showing a theme that is no
     longer on screen. An *edited* box is left alone — losing someone's work in
     progress to a background event would be unforgivable. */
  const backendText = snapshot?.themeDoc ?? null;
  useEffect(() => {
    setAppliedText((previous) => {
      if (previous === backendText) return previous;
      setText((current) => (current === (previous ?? "") || current.trim() === "" ? "" : current));
      return backendText;
    });
  }, [backendText]);

  // Keep the box in step when it has no edits of its own: an empty box means
  // "show me what is in force".
  useEffect(() => {
    setText((current) => (current.trim() === "" ? currentThemeSource() : current));
  }, [appliedText]);

  /* Validate on a debounce. Pure, cheap (a 400-line document parses in well
     under a millisecond) and never applied — this only drives the display. */
  useEffect(() => {
    const id = window.setTimeout(() => setOutcome(validateTheme(text)), VALIDATE_DEBOUNCE_MS);
    return () => window.clearTimeout(id);
  }, [text]);

  const problems = outcome?.problems ?? [];
  const errors = problems.filter((p) => p.level === "error");
  const warnings = problems.filter((p) => p.level === "warning");
  const summary = useMemo(() => (outcome ? summarise(outcome) : null), [outcome]);
  /* "unapplied edits" means the box differs from what is *on screen*, and with
     no document in force what is on screen is the current appearance as a
     document — which is exactly what the box was filled with. Comparing
     against `""` instead would flag a freshly opened editor as edited, and a
     warning that is always on is a warning nobody reads. */
  const baseline = useMemo(() => appliedText ?? currentThemeSource(), [appliedText]);
  const dirty = text.trim() !== baseline.trim();

  const copy = useCallback(
    (what: string, produce: () => string) => {
      void copyWithToast(produce(), what);
    },
    [],
  );

  const apply = useCallback(() => {
    setBusy(true);
    applyTheme(text)
      .then((result) => {
        setOutcome(result.outcome);
        if (!result.applied) {
          pushToast("error", `That theme was not applied — ${errorsSummary(result.outcome)}`);
          return;
        }
        setAppliedText(text);
        const s = summarise(result.outcome);
        const unreadable = s.unreadable > 0 ? `, ${s.unreadable} contrast warning(s)` : "";
        pushToast("info", `Theme applied — ${s.name}, ${s.tokens} tokens${unreadable}`);
      })
      .catch((err: unknown) => {
        pushToast("error", `Could not save the theme — ${(err as Error).message}`);
      })
      .finally(() => setBusy(false));
  }, [pushToast, text]);

  const revert = useCallback(() => {
    setBusy(true);
    revertTheme()
      .then(() => {
        setAppliedText(null);
        setText(defaultThemeText());
        setOutcome(null);
        pushToast("info", "Back to the designed appearance");
      })
      .catch((err: unknown) => pushToast("error", (err as Error).message))
      .finally(() => setBusy(false));
  }, [pushToast]);

  return (
    <div className="theme-editor" data-compact={compact || undefined}>
      <div className="te-bar">
        <button className="ghost-btn" onClick={() => copy("Default theme", defaultThemeText)}>{t("Copy default")}</button>
        <button className="ghost-btn" onClick={() => copy("Current theme", currentThemeSource)}>{t("Copy current")}</button>
        <button
          className="ghost-btn accent"
          title={t("The theme plus a short brief, so pasting it into any chat is enough")}
          onClick={() => copy("Theme and brief", () => themeForAgent(text || undefined))}
        >{t("Copy for agent")}</button>
        <span className="spacer" />
        <button
          className="ghost-btn"
          onClick={() => setOutcome(validateTheme(text))}
          title={t("Check without applying")}
        >{t("Validate")}</button>
      </div>

      <textarea
        ref={area}
        className="te-code num"
        value={text}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        aria-label={t("Theme document")}
        data-bad={errors.length > 0 || undefined}
        placeholder={t("Paste a theme document here.\n\nCopy default → give it to an agent → paste the reply back.")}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          /* An input method is composing: every key below belongs to the
             candidate window, not to this editor. Returning here — rather than
             calling `preventDefault()` and rewriting `value` — is what keeps a
             Pinyin candidate picked with Tab from losing the characters that
             were pending. See `lib/ime.ts`; the guard is first because both
             branches below are destructive. */
          if (isComposingReact(e)) return;
          // ⌘/Ctrl + Enter applies, the way every editor with a Run button does.
          if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
            e.preventDefault();
            if (errors.length === 0 && !busy) apply();
            return;
          }
          /* Tab indents and Shift+Tab dedents, over a whole selected block:
             this is a 420-line document with three nested token blocks in it,
             and a Tab that moved focus out of the box mid-edit — which is what
             a textarea does by default, and what this handler used to let
             happen while a comment claimed otherwise — is the single most
             annoying thing an editor can do. Escape still leaves the field, so
             the field is not a trap. */
          if (e.key === "Tab" && !e.metaKey && !e.ctrlKey && !e.altKey) {
            e.preventDefault();
            const el = e.currentTarget;
            const edit = e.shiftKey
              ? dedentSelection(el.value, el.selectionStart, el.selectionEnd)
              : indentSelection(el.value, el.selectionStart, el.selectionEnd);
            setText(edit.text);
            /* The value comes back from React on the next render, so the caret
               has to be restored after it — set it now and the browser puts it
               back at the end of the box. */
            requestAnimationFrame(() => {
              el.setSelectionRange(edit.selectionStart, edit.selectionEnd);
            });
          }
        }}
      />

      <div className="te-status">
        {t(summary && errors.length === 0 && (
          <span className="te-ok">
            {summary.name} · {summary.tokens} {t(summary.tokens === 1 ? 'token' : 'tokens')}
          </span>
        ))}
        {t(errors.length > 0 && (
          <span className="te-bad">
            {t(errors.length)}{t(" error")}{t(errors.length === 1 ? "" : "s")}
          </span>
        ))}
        {t(warnings.length > 0 && (
          <span className="te-warn">
            {t(warnings.length)}{t(" warning")}{t(warnings.length === 1 ? "" : "s")}
          </span>
        ))}
        {t(dirty && <span className="te-chip">{t("unapplied edits")}</span>)}
        <span className="spacer" />
        <button className="ghost-btn" disabled={busy || appliedText === null} onClick={revert}>{t("Revert")}</button>
        <button
          className="primary-btn"
          disabled={busy || errors.length > 0}
          onClick={apply}
          title={t(errors.length > 0 ? "Fix the errors first" : "Apply to every window")}
        >{t("Apply")}</button>
      </div>

      {t(problems.length > 0 && (
        <ul className="te-problems" aria-live="polite">
          {t(problems.slice(0, 40).map((p, i) => (
            <li key={`${p.line}:${p.col}:${i}`} data-level={p.level}>
              <button
                className="te-line"
                title={t("Go to this line")}
                onClick={() => focusLine(area.current, p.line)}
              >
                {t(p.line)}
              </button>
              <span className="te-msg">
                {t(p.message)}
                {t(p.suggestion && <em className="te-hint"> — {t(p.suggestion)}</em>)}
              </span>
            </li>
          )))}
          {t(problems.length > 40 && <li className="te-more">{t("…and ")}{t(problems.length - 40)}{t(" more")}</li>)}
        </ul>
      ))}

      {t(outcome && outcome.contrast.some((f) => !f.ok) && (
        <div className="te-contrast" role="status">
          <div className="te-contrast-head">{t("Legibility — measured, not blocked. This theme can still be applied.")}</div>
          <ul>
            {t(outcome.contrast
              .filter((f) => !f.ok)
              .slice(0, 8)
              .map((f) => (
                <li key={`${f.theme}:${f.label}`}>
                  <span className="te-theme">{t(f.theme)}</span>
                  <span className="te-pair">{t(f.label)}</span>
                  <span className="num te-ratio">{t(formatRatio(f.ratio))}</span>
                  <span className="num te-min">{t("needs ")}{t(f.min.toFixed(1))}:1</span>
                </li>
              )))}
          </ul>
        </div>
      ))}

      <div className="te-foot">
        <span>{t("JSON with")}<code>//</code>{t("comments and trailing commas. Keys are fixed; unknown ones are errors with a line number.")}</span>
        <button
          className="ghost-btn danger"
          onClick={resetAppearanceEverywhere}
          title={t("Designed themes, champagne accent, system fonts — from anywhere, even an unreadable window")}
        >{t("Reset appearance")}</button>
      </div>
    </div>
  );
}

const errorsSummary = (outcome: ParseOutcome): string => {
  const first = outcome.problems.find((p: Problem) => p.level === "error");
  return first ? `line ${first.line}: ${first.message}` : "it could not be read";
};

/** Put the caret on a problem's line, and scroll it into view. */
function focusLine(area: HTMLTextAreaElement | null, line: number): void {
  if (!area) return;
  const lines = area.value.split("\n");
  const index = Math.min(Math.max(1, line), lines.length) - 1;
  let start = 0;
  for (let i = 0; i < index; i += 1) start += lines[i].length + 1;
  area.focus();
  area.setSelectionRange(start, start + lines[index].length);
  // Rough but reliable: textareas have no scrollIntoView for a caret.
  const rows = area.clientHeight / (parseFloat(getComputedStyle(area).lineHeight) || 16);
  area.scrollTop = Math.max(0, (index - rows / 2) * (area.scrollHeight / lines.length));
}
