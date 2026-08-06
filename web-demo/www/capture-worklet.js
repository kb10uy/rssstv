/**
 * Buffers capture quanta and posts them to the page.
 *
 * The decoding itself stays on the main thread: it is far heavier than a render
 * quantum's budget, and a stall there is an audible glitch rather than a dropped
 * frame. All this does is gather 128-frame quanta into blocks worth sending.
 */
class Capture extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.blockSamples = options?.processorOptions?.blockSamples ?? 2048;
    this.block = new Float32Array(this.blockSamples);
    this.filled = 0;
  }

  process(inputs) {
    const channel = inputs[0]?.[0];
    if (channel === undefined) {
      return true;
    }
    let offset = 0;
    while (offset < channel.length) {
      const take = Math.min(this.blockSamples - this.filled, channel.length - offset);
      this.block.set(channel.subarray(offset, offset + take), this.filled);
      this.filled += take;
      offset += take;
      if (this.filled === this.blockSamples) {
        this.port.postMessage(this.block, [this.block.buffer]);
        this.block = new Float32Array(this.blockSamples);
        this.filled = 0;
      }
    }
    return true;
  }
}

registerProcessor('capture', Capture);
