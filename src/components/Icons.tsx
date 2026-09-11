import { t } from "../lib/i18n";
/** Hand-drawn inline SVG icons. No icon font, no emoji, 1.4 px hairlines. */

interface IconProps {
  size?: number;
  className?: string;
}

const base = (size: number) => ({
  width: size,
  height: size,
  viewBox: "0 0 16 16",
  fill: "none" as const,
  xmlns: "http://www.w3.org/2000/svg",
});

export function IconPlay({ size = 14, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M5 3.4v9.2L12.6 8 5 3.4z" fill="currentColor" />
    </svg>
  );
}

export function IconPause({ size = 14, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M5.2 3.4h1.9v9.2H5.2zM8.9 3.4h1.9v9.2H8.9z" fill="currentColor" />
    </svg>
  );
}

export function IconPrev({ size = 14, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M4.4 3.6v8.8M12 3.8L5.8 8l6.2 4.2V3.8z" stroke="currentColor" strokeWidth="1.3" strokeLinejoin="round" />
    </svg>
  );
}

export function IconNext({ size = 14, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M11.6 3.6v8.8M4 3.8L10.2 8 4 12.2V3.8z" stroke="currentColor" strokeWidth="1.3" strokeLinejoin="round" />
    </svg>
  );
}

export function IconLoop({ size = 14, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <path
        d="M3 6.6a3 3 0 013-3h6M13 9.4a3 3 0 01-3 3H4"
        stroke="currentColor"
        strokeWidth="1.25"
        strokeLinecap="round"
      />
      <path d="M10.6 1.8l2 1.8-2 1.8M5.4 10.6l-2 1.8 2 1.8" stroke="currentColor" strokeWidth="1.25" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

export function IconVolume({ size = 14, muted = false, className }: IconProps & { muted?: boolean }) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M3 6.2h2.2L8 3.6v8.8L5.2 9.8H3V6.2z" fill="currentColor" />
      {t(muted ? (
        <path d="M10.4 6.2l3 3.6M13.4 6.2l-3 3.6" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" />
      ) : (
        <path
          d="M10.2 5.9a2.9 2.9 0 010 4.2M12.1 4.2a5.4 5.4 0 010 7.6"
          stroke="currentColor"
          strokeWidth="1.15"
          strokeLinecap="round"
        />
      ))}
    </svg>
  );
}

export function IconEq({ size = 14, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M4 2.6v3.1M4 8.9v4.5M8 2.6v6.6M8 12.4v1M12 2.6v1.9M12 7.7v5.7" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" />
      <circle cx="4" cy="7.3" r="1.5" stroke="currentColor" strokeWidth="1.2" />
      <circle cx="8" cy="10.8" r="1.5" stroke="currentColor" strokeWidth="1.2" />
      <circle cx="12" cy="6.1" r="1.5" stroke="currentColor" strokeWidth="1.2" />
    </svg>
  );
}

export function IconGear({ size = 14, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <circle cx="8" cy="8" r="2.1" stroke="currentColor" strokeWidth="1.2" />
      <path
        d="M8 1.9v1.4M8 12.7v1.4M14.1 8h-1.4M3.3 8H1.9M12.3 3.7l-1 1M4.7 11.3l-1 1M12.3 12.3l-1-1M4.7 4.7l-1-1"
        stroke="currentColor"
        strokeWidth="1.2"
        strokeLinecap="round"
      />
    </svg>
  );
}

export function IconKeys({ size = 14, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <rect x="1.6" y="4.2" width="12.8" height="7.6" rx="1.4" stroke="currentColor" strokeWidth="1.15" />
      <path d="M4.2 6.8h.01M6.6 6.8h.01M9 6.8h.01M11.4 6.8h.01M4.9 9.3h6.2" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" />
    </svg>
  );
}

export function IconBlind({ size = 14, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M1.8 8s2.4-4 6.2-4 6.2 4 6.2 4-2.4 4-6.2 4S1.8 8 1.8 8z" stroke="currentColor" strokeWidth="1.15" />
      <circle cx="8" cy="8" r="1.6" stroke="currentColor" strokeWidth="1.15" />
      <path d="M2.6 13.4L13.4 2.6" stroke="currentColor" strokeWidth="1.15" strokeLinecap="round" />
    </svg>
  );
}

export function IconClose({ size = 12, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" strokeWidth="1.25" strokeLinecap="round" />
    </svg>
  );
}

export function IconPlus({ size = 12, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M8 3.4v9.2M3.4 8h9.2" stroke="currentColor" strokeWidth="1.25" strokeLinecap="round" />
    </svg>
  );
}

/* Window controls, drawn only where the OS does not draw its own (SPEC §4:
   `decorations: false`; macOS keeps its traffic lights). Deliberately the same
   1.25 px hairline as everything else rather than the native Windows glyphs —
   a faithful copy of one platform's caption buttons looks wrong on the other. */

export function IconMinimize({ size = 12, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M3.5 8.5h9" stroke="currentColor" strokeWidth="1.25" strokeLinecap="round" />
    </svg>
  );
}

export function IconMaximize({ size = 12, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <rect
        x="3.6"
        y="3.6"
        width="8.8"
        height="8.8"
        rx="1.2"
        stroke="currentColor"
        strokeWidth="1.25"
      />
    </svg>
  );
}

export function IconWave({ size = 14, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <path
        d="M1.8 8h1.4M4.6 5.2v5.6M7 2.6v10.8M9.4 4.4v7.2M11.8 6.4v3.2M14.2 7.3v1.4"
        stroke="currentColor"
        strokeWidth="1.2"
        strokeLinecap="round"
      />
    </svg>
  );
}
