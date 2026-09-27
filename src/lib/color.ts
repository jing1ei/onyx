/**
 * Colour maths and the one CSS colour parser, with nothing in it that touches
 * the DOM.
 *
 * Four consumers, which is why it is its own module: `lib/theme.ts` derives an
 * accent family in OKLCH *and* paints canvases from resolved custom properties,
 * `lib/cssvalue.ts` validates whatever colour syntax a pasted theme used, and
 * `lib/contrast.ts` measures whether the result is legible. Node runs this file
 * directly (`scripts/check-theme.mjs`), so it may not import anything that
 * expects a browser.
 *
 * **{@link parseCssColor} is the only colour parser in the front end.** There
 * were three: this one, a token-level one in `cssvalue.ts`, and a hex/rgb-only
 * one in `theme.ts` that returned an *opaque* colour for `oklch()` and `hsl()`
 * — the two syntaxes `THEMING.md` recommends — so a themed canvas gradient
 * painted a solid block where it should have faded out. What the editor
 * validates has to be exactly what the canvas paints, and that is only true if
 * one function decides what a colour string means.
 */

export type Rgb = [number, number, number];

export interface Rgba {
  /** 0..255 */
  r: number;
  g: number;
  b: number;
  /** 0..1 */
  a: number;
}

export interface Lch {
  /** 0..1, perceptual */
  L: number;
  C: number;
  /** degrees */
  h: number;
}

export const clamp = (v: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, v));

const round255 = (v: number): number => Math.round(clamp(v, 0, 255));

/** `#rgb` / `#rrggbb`, with or without the `#` — the grammar Rust accepts too. */
export function parseHex(input: string): string | null {
  const s = input.trim().replace(/^#/, "").toLowerCase();
  if (/^[0-9a-f]{3}$/.test(s)) return `#${s[0]}${s[0]}${s[1]}${s[1]}${s[2]}${s[2]}`;
  if (/^[0-9a-f]{6}$/.test(s)) return `#${s}`;
  return null;
}

export function hexToRgb(hex: string): Rgb {
  const s = hex.replace("#", "");
  return [
    parseInt(s.slice(0, 2), 16),
    parseInt(s.slice(2, 4), 16),
    parseInt(s.slice(4, 6), 16),
  ];
}

export function rgbToHex([r, g, b]: Rgb): string {
  const to = (v: number): string => round255(v).toString(16).padStart(2, "0");
  return `#${to(r)}${to(g)}${to(b)}`;
}

/* ── sRGB ↔ OKLab/OKLCH ───────────────────────────────────────────────────
   Lightness in OKLab is perceptual, which is the whole reason for the detour:
   "two shades brighter" done in HSL swings the hue and the saturation of
   anything that is not already a pastel. */

export const srgbToLinear = (c: number): number =>
  c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
export const linearToSrgb = (c: number): number =>
  c <= 0.0031308 ? 12.92 * c : 1.055 * Math.pow(c, 1 / 2.4) - 0.055;

export function rgbToLch([r8, g8, b8]: Rgb): Lch {
  const r = srgbToLinear(r8 / 255);
  const g = srgbToLinear(g8 / 255);
  const b = srgbToLinear(b8 / 255);
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const L = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
  const A = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
  const B = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
  return { L, C: Math.hypot(A, B), h: (Math.atan2(B, A) * 180) / Math.PI };
}

export function lchToRgb({ L, C, h }: Lch): { rgb: Rgb; inGamut: boolean } {
  const rad = (h * Math.PI) / 180;
  const A = C * Math.cos(rad);
  const B = C * Math.sin(rad);
  const l = (L + 0.3963377774 * A + 0.2158037573 * B) ** 3;
  const m = (L - 0.1055613458 * A - 0.0638541728 * B) ** 3;
  const s = (L - 0.0894841775 * A - 1.291485548 * B) ** 3;
  const lin: Rgb = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
  const inGamut = lin.every((v) => v >= -0.0002 && v <= 1.0002);
  const rgb = lin.map((v) => linearToSrgb(clamp(v, 0, 1)) * 255) as Rgb;
  return { rgb, inGamut };
}

/**
 * Out-of-gamut colours lose chroma, never lightness: a custom accent must keep
 * the brightness the state it is used in was tuned for, even where sRGB cannot
 * hold that much colour at that brightness.
 */
export function lchToHex(lch: Lch): string {
  let { C } = lch;
  for (let i = 0; i < 48; i += 1) {
    const { rgb, inGamut } = lchToRgb({ ...lch, C });
    if (inGamut || C < 0.0005) return rgbToHex(rgb);
    C *= 0.94;
  }
  return rgbToHex(lchToRgb({ ...lch, C: 0 }).rgb);
}

/* ── HSL, because a pasted theme will contain some ─────────────────────── */

export function hslToRgb(h: number, s: number, l: number): Rgb {
  const hue = ((h % 360) + 360) % 360;
  const sat = clamp(s, 0, 1);
  const lig = clamp(l, 0, 1);
  const c = (1 - Math.abs(2 * lig - 1)) * sat;
  const x = c * (1 - Math.abs(((hue / 60) % 2) - 1));
  const m = lig - c / 2;
  const [r, g, b] =
    hue < 60
      ? [c, x, 0]
      : hue < 120
        ? [x, c, 0]
        : hue < 180
          ? [0, c, x]
          : hue < 240
            ? [0, x, c]
            : hue < 300
              ? [x, 0, c]
              : [c, 0, x];
  return [(r + m) * 255, (g + m) * 255, (b + m) * 255];
}

/* ── WCAG 2.1 ─────────────────────────────────────────────────────────────
   Contrast is defined on *opaque* colours, and almost every ink in this app is
   a translucent white or near-black over a surface, so the caller composites
   first (`over`) and measures second. */

export function relativeLuminance({ r, g, b }: Rgba): number {
  const lin = (v: number): number => srgbToLinear(clamp(v, 0, 255) / 255);
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

/** `fg` composited over `bg` (source-over), which is what the eye sees. */
export function over(fg: Rgba, bg: Rgba): Rgba {
  const a = clamp(fg.a, 0, 1);
  const ba = clamp(bg.a, 0, 1);
  const outA = a + ba * (1 - a);
  if (outA <= 0) return { r: 0, g: 0, b: 0, a: 0 };
  const mix = (f: number, b: number): number => (f * a + b * ba * (1 - a)) / outA;
  return { r: mix(fg.r, bg.r), g: mix(fg.g, bg.g), b: mix(fg.b, bg.b), a: outA };
}

/** WCAG 2.1 contrast ratio, 1..21. Both colours must already be opaque. */
export function contrastRatio(a: Rgba, b: Rgba): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  const hi = Math.max(la, lb);
  const lo = Math.min(la, lb);
  return (hi + 0.05) / (lo + 0.05);
}

export const formatRgba = ({ r, g, b, a }: Rgba): string =>
  a >= 0.999
    ? rgbToHex([r, g, b])
    : `rgba(${round255(r)}, ${round255(g)}, ${round255(b)}, ${Number(clamp(a, 0, 1).toFixed(4))})`;

/* ── CSS named colours ────────────────────────────────────────────────────
   An LLM writes `crimson`, not `#dc143c`. The whole CSS list, packed, because
   half of it would be the half the user wanted. */

const NAMED_PACKED =
  "aliceblue f0f8ff,antiquewhite faebd7,aqua 00ffff,aquamarine 7fffd4,azure f0ffff,beige f5f5dc," +
  "bisque ffe4c4,black 000000,blanchedalmond ffebcd,blue 0000ff,blueviolet 8a2be2,brown a52a2a," +
  "burlywood deb887,cadetblue 5f9ea0,chartreuse 7fff00,chocolate d2691e,coral ff7f50," +
  "cornflowerblue 6495ed,cornsilk fff8dc,crimson dc143c,cyan 00ffff,darkblue 00008b," +
  "darkcyan 008b8b,darkgoldenrod b8860b,darkgray a9a9a9,darkgreen 006400,darkgrey a9a9a9," +
  "darkkhaki bdb76b,darkmagenta 8b008b,darkolivegreen 556b2f,darkorange ff8c00,darkorchid 9932cc," +
  "darkred 8b0000,darksalmon e9967a,darkseagreen 8fbc8f,darkslateblue 483d8b,darkslategray 2f4f4f," +
  "darkslategrey 2f4f4f,darkturquoise 00ced1,darkviolet 9400d3,deeppink ff1493,deepskyblue 00bfff," +
  "dimgray 696969,dimgrey 696969,dodgerblue 1e90ff,firebrick b22222,floralwhite fffaf0," +
  "forestgreen 228b22,fuchsia ff00ff,gainsboro dcdcdc,ghostwhite f8f8ff,gold ffd700," +
  "goldenrod daa520,gray 808080,green 008000,greenyellow adff2f,grey 808080,honeydew f0fff0," +
  "hotpink ff69b4,indianred cd5c5c,indigo 4b0082,ivory fffff0,khaki f0e68c,lavender e6e6fa," +
  "lavenderblush fff0f5,lawngreen 7cfc00,lemonchiffon fffacd,lightblue add8e6,lightcoral f08080," +
  "lightcyan e0ffff,lightgoldenrodyellow fafad2,lightgray d3d3d3,lightgreen 90ee90," +
  "lightgrey d3d3d3,lightpink ffb6c1,lightsalmon ffa07a,lightseagreen 20b2aa,lightskyblue 87cefa," +
  "lightslategray 778899,lightslategrey 778899,lightsteelblue b0c4de,lightyellow ffffe0," +
  "lime 00ff00,limegreen 32cd32,linen faf0e6,magenta ff00ff,maroon 800000,mediumaquamarine 66cdaa," +
  "mediumblue 0000cd,mediumorchid ba55d3,mediumpurple 9370db,mediumseagreen 3cb371," +
  "mediumslateblue 7b68ee,mediumspringgreen 00fa9a,mediumturquoise 48d1cc,mediumvioletred c71585," +
  "midnightblue 191970,mintcream f5fffa,mistyrose ffe4e1,moccasin ffe4b5,navajowhite ffdead," +
  "navy 000080,oldlace fdf5e6,olive 808000,olivedrab 6b8e23,orange ffa500,orangered ff4500," +
  "orchid da70d6,palegoldenrod eee8aa,palegreen 98fb98,paleturquoise afeeee,palevioletred db7093," +
  "papayawhip ffefd5,peachpuff ffdab9,peru cd853f,pink ffc0cb,plum dda0dd,powderblue b0e0e6," +
  "purple 800080,rebeccapurple 663399,red ff0000,rosybrown bc8f8f,royalblue 4169e1," +
  "saddlebrown 8b4513,salmon fa8072,sandybrown f4a460,seagreen 2e8b57,seashell fff5ee," +
  "sienna a0522d,silver c0c0c0,skyblue 87ceeb,slateblue 6a5acd,slategray 708090,slategrey 708090," +
  "snow fffafa,springgreen 00ff7f,steelblue 4682b4,tan d2b48c,teal 008080,thistle d8bfd8," +
  "tomato ff6347,turquoise 40e0d0,violet ee82ee,wheat f5deb3,white ffffff,whitesmoke f5f5f5," +
  "yellow ffff00,yellowgreen 9acd32";

export const NAMED_COLORS: ReadonlyMap<string, string> = new Map(
  NAMED_PACKED.split(",").map((pair) => {
    const [name, hex] = pair.trim().split(" ");
    return [name, `#${hex}`] as const;
  }),
);

/* ── the one CSS colour parser ─────────────────────────────────────────────
   Every syntax `cssvalue.ts` accepts for a `color` token, parsed back out of a
   string: that is the contract. A computed custom property keeps whichever
   syntax the theme was authored in — the browser hands `oklch(0.78 0.15 85 /
   0.55)` straight back — so the canvas has to understand the same grammar the
   validator does, or a legal theme paints wrong. */

/** A numeric component of a colour function, with its unit. */
interface NumTok {
  v: number;
  unit: string;
}

/** Pull the numbers out of a colour function's argument list, in order. */
function numbers(args: string): NumTok[] {
  const out: NumTok[] = [];
  for (const m of args.matchAll(/([+-]?(?:\d+\.?\d*|\.\d+))(%|deg|turn|rad|grad)?/gi)) {
    const v = parseFloat(m[1]);
    if (!Number.isFinite(v)) continue;
    out.push({ v, unit: (m[2] ?? "").toLowerCase() });
  }
  return out;
}

/** Degrees, from whatever angular unit was written. */
function degrees({ v, unit }: NumTok): number {
  switch (unit) {
    case "turn":
      return v * 360;
    case "rad":
      return (v * 180) / Math.PI;
    case "grad":
      return v * 0.9;
    default:
      return v;
  }
}

const alphaAt = (nums: NumTok[], i: number): number => {
  const a = nums[i];
  if (!a) return 1;
  return clamp(a.unit === "%" ? a.v / 100 : a.v, 0, 1);
};

const channel = (n: NumTok): number => clamp(n.unit === "%" ? (n.v / 100) * 255 : n.v, 0, 255);

/**
 * `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, a CSS colour name, `transparent`,
 * `rgb()`/`rgba()`, `hsl()`/`hsla()` and `oklch()` — in legacy comma form and
 * in the modern space-separated form with a `/ alpha`.
 *
 * `null` for anything else, which includes the syntaxes whose value depends on
 * another token (`rgb(var(--accent-rgb) / 0.4)`, `currentcolor`): those have no
 * literal colour *here*, and the caller — the contrast audit, or the canvas
 * palette reading an already-substituted computed value — is the one that knows
 * how to resolve them.
 */
export function parseCssColor(input: string): Rgba | null {
  const s = input.trim();
  if (s === "") return null;

  if (s.startsWith("#")) {
    const h = s.slice(1).toLowerCase();
    if (!/^[0-9a-f]+$/.test(h)) return null;
    const pair = (i: number): number =>
      h.length <= 4 ? parseInt(h[i] + h[i], 16) : parseInt(h.slice(i * 2, i * 2 + 2), 16);
    if (h.length === 3 || h.length === 4) {
      return { r: pair(0), g: pair(1), b: pair(2), a: h.length === 4 ? pair(3) / 255 : 1 };
    }
    if (h.length === 6 || h.length === 8) {
      return { r: pair(0), g: pair(1), b: pair(2), a: h.length === 8 ? pair(3) / 255 : 1 };
    }
    return null;
  }

  const fn = /^([a-z]+)\(([\s\S]*)\)$/i.exec(s);
  if (!fn) {
    const word = s.toLowerCase();
    if (word === "transparent") return { r: 0, g: 0, b: 0, a: 0 };
    const named = NAMED_COLORS.get(word);
    if (!named) return null;
    const [r, g, b] = hexToRgb(named);
    return { r, g, b, a: 1 };
  }

  const name = fn[1].toLowerCase();
  const args = fn[2];
  // A nested function is a reference to something else — not a literal colour.
  if (args.includes("(")) return null;
  const nums = numbers(args);
  if (nums.length < 3) return null;

  if (name === "rgb" || name === "rgba") {
    return {
      r: channel(nums[0]),
      g: channel(nums[1]),
      b: channel(nums[2]),
      a: alphaAt(nums, 3),
    };
  }
  if (name === "hsl" || name === "hsla") {
    const [r, g, b] = hslToRgb(degrees(nums[0]), nums[1].v / 100, nums[2].v / 100);
    return { r, g, b, a: alphaAt(nums, 3) };
  }
  if (name === "oklch") {
    const L = clamp(nums[0].unit === "%" ? nums[0].v / 100 : nums[0].v, 0, 1);
    const { rgb } = lchToRgb({ L, C: clamp(nums[1].v, 0, 0.5), h: degrees(nums[2]) });
    return { r: rgb[0], g: rgb[1], b: rgb[2], a: alphaAt(nums, 3) };
  }
  return null;
}

/**
 * Onyx's `rgb` token idiom — three bare 0…255 numbers, `"232 217 160"`.
 *
 * Not a CSS colour: it is the *inside* of one, so that `--lane-a-rgb` can be
 * used both as `rgb(var(--lane-a-rgb) / 0.4)` in the stylesheet and as a tint
 * on a canvas. The canvas palette and the contrast audit both have to read it,
 * so it lives next to the parser rather than in each of them.
 */
export function parseRgbTriplet(input: string): Rgba | null {
  const m = /^([+-]?[\d.]+)[\s,]+([+-]?[\d.]+)[\s,]+([+-]?[\d.]+)$/.exec(input.trim());
  if (!m) return null;
  const [r, g, b] = [+m[1], +m[2], +m[3]];
  if (![r, g, b].every(Number.isFinite)) return null;
  return { r: clamp(r, 0, 255), g: clamp(g, 0, 255), b: clamp(b, 0, 255), a: 1 };
}

/** The same colour with its alpha multiplied — how every canvas fade is built. */
export const scaleAlpha = (c: Rgba, factor: number): Rgba => ({
  ...c,
  a: clamp(c.a * factor, 0, 1),
});
