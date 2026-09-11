import { useStore } from './store';
let switcher: ((editing: boolean) => Promise<void>) | null = null;
let pending = false;
export function registerModeSwitcher(fn: (editing: boolean) => Promise<void>) {
  switcher = fn;
  return () => { if (switcher === fn) switcher = null; };
}
export async function switchMode(editing: boolean) {
  if (pending || editing === useStore.getState().editorActive) return;
  pending = true;
  useStore.getState().setModeError(null);
  useStore.getState().setModeSwitching(true);
  try {
    if (!switcher) throw new Error('编辑器尚未准备好，请稍后重试');
    await switcher(editing);
  } catch (error) {
    useStore.getState().setModeError(error instanceof Error ? error.message : String(error));
  } finally { pending = false; useStore.getState().setModeSwitching(false); }
}
export function samePath(a: string, b: string) {
  const normalize = (path: string) => path.replace(/\\/g, '/').replace(/^\/\/\?\/UNC\//i, '//').replace(/^\/\/\?\//, '').toLowerCase();
  return normalize(a) === normalize(b);
}
