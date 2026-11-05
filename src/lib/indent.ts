/**
 * Indent and dedent, as pure text edits — the theme editor's `Tab` key.
 *
 * A theme document is 172 tokens across ~420 lines (SPEC §20.2), and it is
 * edited in a plain `<textarea>`: no editor component, because one would bring
 * its own tokenizer, its own idea of what CSS is and a second implementation of
 * the grammar the app already has. What that costs is exactly this — `Tab` has
 * to be implemented — and the version of it that existed did the worst half:
 * the key handler *claimed* in a comment that `Tab` inserted a tab and that
 * `Shift+Tab` "still got out", while handling neither, so `Tab` left the field
 * mid-edit and a block of tokens could only be re-indented a line at a time.
 *
 * The logic lives here rather than in the component so it can be tested without
 * a DOM (`scripts/check-theme.mjs`): a caret arithmetic bug in an editor is the
 * kind of thing that silently eats a character, and a keystroke is not something
 * a screenshot can check.
 *
 * `INDENT` is two spaces because that is what `themedoc.ts` exports, and an
 * editor whose `Tab` disagrees with the document it is editing re-indents the
 * file the first time someone touches it.
 */

export const INDENT = "  ";

/** A textarea's whole state after an edit: the value and where the caret is. */
export interface TextEdit {
  text: string;
  selectionStart: number;
  selectionEnd: number;
}

/** The [start, end] character offsets of every line the selection touches. */
function touchedLines(text: string, start: number, end: number): { from: number; to: number } {
  const from = text.lastIndexOf("\n", start - 1) + 1;
  let to = text.indexOf("\n", end);
  if (to < 0) to = text.length;
  return { from, to };
}

/** How many leading whitespace characters one dedent should take off a line. */
function dedentWidth(line: string): number {
  if (line.startsWith("\t")) return 1;
  let n = 0;
  while (n < INDENT.length && line[n] === " ") n += 1;
  return n;
}

/**
 * `Tab`. A caret inserts one indent; a selection indents every line it touches
 * and stays around them, so the key can be held down.
 */
export function indentSelection(text: string, start: number, end: number): TextEdit {
  if (start === end) {
    return {
      text: text.slice(0, start) + INDENT + text.slice(start),
      selectionStart: start + INDENT.length,
      selectionEnd: start + INDENT.length,
    };
  }
  const { from, to } = touchedLines(text, start, end);
  const block = text.slice(from, to);
  // A blank line gets nothing: trailing whitespace is not indentation, and
  // adding it would leave a document that no longer matches its own export.
  const lines = block.split("\n").map((l) => (l.trim() === "" ? l : INDENT + l));
  const next = lines.join("\n");
  return {
    text: text.slice(0, from) + next + text.slice(to),
    selectionStart: from,
    selectionEnd: from + next.length,
  };
}

/**
 * `Shift+Tab`. Removes one indent from every line the selection touches, and
 * from the caret's own line when there is no selection — never more than the
 * line actually has, so it cannot eat a token.
 */
export function dedentSelection(text: string, start: number, end: number): TextEdit {
  const { from, to } = touchedLines(text, start, end);
  const block = text.slice(from, to);
  let firstLineCut = 0;
  let cutTotal = 0;
  const lines = block.split("\n").map((line, i) => {
    const cut = dedentWidth(line);
    if (i === 0) firstLineCut = cut;
    cutTotal += cut;
    return line.slice(cut);
  });
  const next = lines.join("\n");
  if (cutTotal === 0) return { text, selectionStart: start, selectionEnd: end };
  const out = text.slice(0, from) + next + text.slice(to);
  if (start === end) {
    // Keep the caret where it was in the line, not where it was in the file.
    const at = Math.max(from, start - firstLineCut);
    return { text: out, selectionStart: at, selectionEnd: at };
  }
  return { text: out, selectionStart: from, selectionEnd: from + next.length };
}
