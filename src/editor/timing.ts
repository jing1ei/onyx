import { logWarn } from '../lib/log';

// Slow-path diagnostics contain timings only, never filenames or sample data.
export function editorTiming(operation: string) {
  const start = performance.now();
  let previous = start;
  const stages: Record<string, number> = {};
  return {
    step(name: string) {
      const now = performance.now(); stages[name] = Math.round(now - previous); previous = now;
    },
    finish() {
      const totalMs = Math.round(performance.now() - start);
      performance.clearMeasures(`onyx-${operation}`);
      performance.measure(`onyx-${operation}`, {start, detail:{totalMs, stages}});
      if (totalMs >= 100) logWarn(`Slow editor ${operation}`, {totalMs, stages});
    },
  };
}
