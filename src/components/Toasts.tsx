import { useStore } from "../lib/store";

export default function Toasts() {
  const toasts = useStore((s) => s.toasts);
  const dismiss = useStore((s) => s.dismissToast);

  return (
    <div className="toasts" aria-live="polite" aria-relevant="additions">
      {toasts.map((t) => (
        <div key={t.id} className="toast" data-kind={t.kind} onClick={() => dismiss(t.id)}>
          <i />
          <span>{t.message}</span>
          <button className="toast-dismiss" aria-label="Dismiss notification" onClick={() => dismiss(t.id)}>×</button>
        </div>
      ))}
    </div>
  );
}
