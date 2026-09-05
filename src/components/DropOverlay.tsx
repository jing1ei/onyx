import { useStore } from "../lib/store";
import { IconWave } from "./Icons";

export default function DropOverlay() {
  const snapshot = useStore((s) => s.snapshot);
  const empty = (snapshot?.playlist.length ?? 0) === 0;

  return (
    <div className="drop-overlay">
      <div className="drop-frame">
        <IconWave size={26} />
        <strong>{empty ? "Drop to play" : "Drop to append"}</strong>
        <span>
          {(snapshot?.supportedExtensions ?? ["flac", "wav", "aiff", "mp3", "m4a", "ogg"])
            .slice(0, 8)
            .join(" · ")}
        </span>
      </div>
    </div>
  );
}
