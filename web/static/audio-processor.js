class MicrophoneTapProcessor extends AudioWorkletProcessor {
  process(inputs) {
    const channels = inputs[0];
    const input = channels?.[0];
    if (!channels || !input || input.length === 0) return true;

    // Web Audio owns and reuses the input buffer, so transfer a single copy to
    // the worker. Resampling, PCM conversion, levels, VAD, and framing live in WASM.
    const samples = new Float32Array(input.length);
    for (const channel of channels) {
      for (let index = 0; index < samples.length; index += 1) {
        samples[index] += channel[index] / channels.length;
      }
    }
    this.port.postMessage({ type: 'samples', payload: samples.buffer }, [samples.buffer]);
    return true;
  }
}

registerProcessor('microphone-tap-processor', MicrophoneTapProcessor);
