import { useStore } from "../lib/store";

export default function Toasts() {
  const toasts = useStore((s) => s.toasts);
  const dismiss = useStore((s) => s.dismissToast);

  if (toasts.length === 0) return null;

  return (
    <div className="toasts">
      {toasts.map((t) => (
        <div key={t.id} className="toast" data-kind={t.kind} onClick={() => dismiss(t.id)}>
          <i />
          <span>{t.message}</span>
        </div>
      ))}
    </div>
  );
}
