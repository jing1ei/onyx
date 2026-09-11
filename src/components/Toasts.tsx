import { t } from "../lib/i18n";
import { useStore } from "../lib/store";

export default function Toasts() {
  const toasts = useStore((s) => s.toasts);
  const dismiss = useStore((s) => s.dismissToast);

  if (toasts.length === 0) return null;

  return (
    <div className="toasts">
      {t(toasts.map((toast) => (
        <div key={toast.id} className="toast" data-kind={toast.kind} onClick={() => dismiss(toast.id)}>
          <i />
          <span>{t(toast.message)}</span>
        </div>
      )))}
    </div>
  );
}
