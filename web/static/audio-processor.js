class MicrophoneTapProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.externalActive = false;
    this.externalChunks = [];
    this.externalOffset = 0;
    this.externalSamples = 0;
    this.port.onmessage = (event) => {
      if (event.data?.type === 'external-start') {
        this.externalActive = true;
      } else if (event.data?.type === 'external-stop') {
        this.externalActive = false;
        this.externalChunks = [];
        this.externalOffset = 0;
        this.externalSamples = 0;
      } else if (event.data?.type === 'external-pcm' && event.data.payload) {
        this.enqueueExternal(
          new Int16Array(event.data.payload),
          event.data.sampleRate || sampleRate,
        );
      }
    };
  }

  enqueueExternal(input, inputSampleRate) {
    if (input.length === 0) return;
    const ratio = inputSampleRate / sampleRate;
    const outputLength = Math.max(1, Math.floor(input.length / ratio));
    const output = new Float32Array(outputLength);
    for (let index = 0; index < outputLength; index += 1) {
      const position = index * ratio;
      const left = Math.min(input.length - 1, Math.floor(position));
      const right = Math.min(input.length - 1, left + 1);
      const fraction = position - left;
      output[index] = (input[left] + (input[right] - input[left]) * fraction) / 32768;
    }
    this.externalChunks.push(output);
    this.externalSamples += output.length;
    const maximumSamples = sampleRate * 2;
    while (this.externalSamples > maximumSamples && this.externalChunks.length > 1) {
      const removed = this.externalChunks.shift();
      this.externalSamples -= removed.length - this.externalOffset;
      this.externalOffset = 0;
    }
  }

  readExternal() {
    const chunk = this.externalChunks[0];
    if (!chunk) return 0;
    const value = chunk[this.externalOffset];
    this.externalOffset += 1;
    this.externalSamples -= 1;
    if (this.externalOffset >= chunk.length) {
      this.externalChunks.shift();
      this.externalOffset = 0;
    }
    return value;
  }

  process(inputs) {
    const channels = inputs[0];
    const input = channels?.[0];
    const hasMicrophone = Boolean(channels && input && input.length > 0);
    if (!hasMicrophone && !this.externalActive) return true;

    // Web Audio owns and reuses the input buffer, so transfer a single copy to
    // the worker. Resampling, PCM conversion, levels, VAD, and framing live in WASM.
    const samples = new Float32Array(hasMicrophone ? input.length : 128);
    const mixedGain = hasMicrophone && this.externalActive ? Math.SQRT1_2 : 1;
    if (hasMicrophone) {
      for (const channel of channels) {
        for (let index = 0; index < samples.length; index += 1) {
          samples[index] += (channel[index] / channels.length) * mixedGain;
        }
      }
    }
    if (this.externalActive) {
      for (let index = 0; index < samples.length; index += 1) {
        samples[index] += this.readExternal() * mixedGain;
      }
    }
    this.port.postMessage({ type: 'samples', payload: samples.buffer }, [samples.buffer]);
    return true;
  }
}

registerProcessor('microphone-tap-processor', MicrophoneTapProcessor);
