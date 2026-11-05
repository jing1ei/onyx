import { useStore } from "../lib/store";
import "../styles/connection.css";

/** Shared startup/recovery state for all three windows. */
export default function ConnectionStatus() {
  const snapshot = useStore((s) => s.snapshot);
  const connected = useStore((s) => s.connected);
  const error = useStore((s) => s.lastError);
  if (connected && snapshot) return null;
  if (!error) return snapshot ? null : <div className="connecting" role="status">Connecting to engine…</div>;
  return (
    <div className="connection-error" role="alert">
      <strong>Can’t connect to the audio engine</strong>
      <span>{error}</span>
      <button className="solid-btn" onClick={() => window.location.reload()}>Reconnect</button>
    </div>
  );
}
