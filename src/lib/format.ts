/** Number / time formatting. Typographic minus (−, U+2212) everywhere it shows. */
import { t } from './i18n';

const MINUS = "\u2212";
const INF = "\u2212\u221E"; // −∞

/** `m:ss` under an hour, `h:mm:ss` above. Negative / NaN → `0:00`. */
export function formatTime(secs: number): string {
  if (!Number.isFinite(secs) || secs < 0) secs = 0;
  const total = Math.floor(secs);
  const s = total % 60;
  const m = Math.floor(total / 60) % 60;
  const h = Math.floor(total / 3600);
  const ss = s < 10 ? `0${s}` : `${s}`;
  if (h > 0) return `${h}:${m < 10 ? `0${m}` : m}:${ss}`;
  return `${m}:${ss}`;
}

/** `m:ss.cs` — used by the waveform hover read-out. */
export function formatTimeFine(secs: number): string {
  if (!Number.isFinite(secs) || secs < 0) secs = 0;
  const cs = Math.floor((secs - Math.floor(secs)) * 100);
  return `${formatTime(secs)}.${cs < 10 ? `0${cs}` : cs}`;
}

function fixMinus(text: string): string {
  return text.replace(/-/g, MINUS);
}

/** dBFS read-out, `−∞` below −90 dB. */
export function formatDb(db: number, digits = 1): string {
  if (!Number.isFinite(db) || db <= -90) return INF;
  return fixMinus(db.toFixed(digits));
}

/** Always-signed dB, for trims and gains: `+1.4`, `−2.0`, `0.0`. */
export function formatSignedDb(db: number, digits = 1): string {
  if (!Number.isFinite(db)) return "0.0";
  const v = db.toFixed(digits);
  if (Math.abs(db) < 0.05) return (0).toFixed(digits);
  return db > 0 ? `+${v}` : fixMinus(v);
}

/** LUFS / LU read-out, one decimal, `−∞` below −70 LUFS. */
export function formatLufs(lufs: number, digits = 1): string {
  if (!Number.isFinite(lufs) || lufs <= -70) return INF;
  return fixMinus(lufs.toFixed(digits));
}

export function formatLu(lu: number, digits = 1): string {
  if (!Number.isFinite(lu)) return "0.0";
  return fixMinus(lu.toFixed(digits));
}

/** `96.0 kHz`, `44.1 kHz`, `192 kHz`. */
export function formatSampleRate(hz: number): string {
  if (!Number.isFinite(hz) || hz <= 0) return "—";
  const k = hz / 1000;
  return `${k >= 100 ? k.toFixed(0) : k.toFixed(1)} kHz`;
}

function formatBitrate(kbps: number | null): string | null {
  if (kbps == null || !Number.isFinite(kbps) || kbps <= 0) return null;
  if (kbps >= 1000) return `${(kbps / 1000).toFixed(1)} Mbps`;
  return `${Math.round(kbps)} kbps`;
}

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "—";
  const units = ["B", "KB", "MB", "GB"];
  let v = bytes;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v >= 100 || i === 0 ? v.toFixed(0) : v.toFixed(1)} ${units[i]}`;
}

function formatChannels(channels: number): string {
  if (channels === 1) return t("mono");
  if (channels === 2) return t("stereo");
  return `${channels} ch`;
}

/** Linear gain (0..1) → dBFS. `-Infinity` at silence. */
export function linToDb(lin: number): number {
  return lin <= 0 ? -Infinity : 20 * Math.log10(lin);
}

/** dBFS → linear gain, clamped to 0..1. */
export function dbToLin(db: number): number {
  return Math.max(0, Math.min(1, Math.pow(10, db / 20)));
}

/** Volume read-out for the transport: `−12.4`, `0.0`, `−∞`. */
export function formatVolumeDb(lin: number): string {
  if (lin <= 0.0005) return INF;
  return formatSignedDb(linToDb(lin)).replace(/^\+/, "");
}

/**
 * Exact-binomial p-value read-out. Very small values are reported as an upper
 * bound rather than as a suspiciously precise `0.000`.
 */
export function formatPValue(p: number | null): string {
  if (p == null || !Number.isFinite(p)) return "\u2014";
  if (p < 0.001) return "< 0.001";
  return p.toFixed(3);
}

/**
 * `MP4 · AAC · 256 kbps · 44.1 kHz · stereo`, and `MIDI · GM · <bank> · …` for
 * a rendered MIDI file.
 *
 * Mirrors `TrackInfo::format_badge()` in `crates/onyx-core/src/types.rs`
 * (SPEC §17/§18). The container leads because codec alone is ambiguous —
 * AAC in an `.m4a` and AAC in a `.mov` are not the same thing to somebody
 * chasing a delivery problem — and it is dropped when it would only repeat the
 * codec, so a FLAC does not read `FLAC · FLAC`. The container comes from the
 * file's *content*, so a `.wav` holding an MP3 says `MP3` here.
 */
export function formatBadge(
  info: {
    codec: string;
    container?: string;
    bitsPerSample: number | null;
    sampleRate: number;
    channels: number;
    bitrateKbps?: number | null;
    isLossless?: boolean;
    synthBank?: string | null;
  } | null,
): string {
  if (!info) return "";
  // A MIDI file has no codec and no bit depth; the bank is what decides what
  // you actually hear, so it takes the slot both of them would have had.
  if (info.synthBank) {
    return ["MIDI", "GM", info.synthBank, formatSampleRate(info.sampleRate), "stereo"].join(
      " \u00B7 ",
    );
  }
  const codec = info.codec.toUpperCase();
  const parts: string[] = [];
  const container = (info.container ?? "").trim();
  if (container && container.toUpperCase() !== codec) parts.push(container);
  parts.push(codec);
  if (info.bitsPerSample) parts.push(`${info.bitsPerSample} bit`);
  else {
    const br = formatBitrate(info.bitrateKbps ?? null);
    if (br) parts.push(br);
  }
  parts.push(formatSampleRate(info.sampleRate));
  parts.push(formatChannels(info.channels));
  return parts.join(" \u00B7 ");
}
