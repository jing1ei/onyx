// Exact summaries; edge samples are read directly. Editor buffers are immutable.
const blockSize = 256;
const cache = new WeakMap<AudioBuffer, WaveIndex[]>();
export class WaveIndex {
  private peaks: Float32Array;
  private squares: Float64Array;
  constructor(private data: Float32Array) {
    const count = Math.ceil(data.length / blockSize);
    this.peaks = new Float32Array(count);
    this.squares = new Float64Array(count);
    for (let b = 0; b < count; b++) {
      let peak = 0, sum = 0;
      for (let i = b * blockSize; i < Math.min(data.length, (b + 1) * blockSize); i++) {
        const v = data[i]; peak = Math.max(peak, Math.abs(v)); sum += v * v;
      }
      this.peaks[b] = peak; this.squares[b] = sum;
    }
  }
  range(start: number, end: number) {
    start = Math.max(0, Math.min(this.data.length, Math.floor(start)));
    end = Math.max(start, Math.min(this.data.length, Math.floor(end)));
    let peak = 0, squares = 0, i = start;
    while (i < end) {
      if (i % blockSize === 0 && i + blockSize <= end) {
        peak = Math.max(peak, this.peaks[i / blockSize]);
        squares += this.squares[i / blockSize]; i += blockSize;
      } else {
        const v = this.data[i++]; peak = Math.max(peak, Math.abs(v)); squares += v * v;
      }
    }
    return { peak, rms: end > start ? Math.sqrt(squares / (end - start)) : 0 };
  }
}
export function waveIndexes(buffer: AudioBuffer) {
  let indexes = cache.get(buffer);
  if (!indexes) {
    indexes = Array.from({length: buffer.numberOfChannels}, (_, c) => new WaveIndex(buffer.getChannelData(c)));
    cache.set(buffer, indexes);
  }
  return indexes;
}
