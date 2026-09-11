/** Linear gain across the selected samples only; both endpoints are exact. */
export function fadeGain(position: number, direction: 'in' | 'out', power = 1) {
  const value = Math.pow(Math.max(0, Math.min(1, position)), Math.max(.125, Math.min(8, power)));
  return direction === 'in' ? value : 1 - value;
}
export function fadeSamples(data: Float32Array, start: number, end: number, direction: 'in' | 'out', power = 1) {
  start = Math.max(0, Math.min(data.length, Math.floor(start)));
  end = Math.max(start, Math.min(data.length, Math.floor(end)));
  const length = end - start;
  for (let i = 0; i < length; i++) {
    data[start + i] *= length === 1 ? 0 : fadeGain(i / (length - 1), direction, power);
  }
}
