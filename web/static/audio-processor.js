class MicrophoneTapProcessor extends AudioWorkletProcessor {
  process(inputs) {
    const input = inputs[0]?.[0];
    if (!input || input.length === 0) return true;

    // Web Audio owns and reuses the input buffer, so transfer a single copy to
    // the worker. Resampling, PCM conversion, levels, VAD, and framing live in WASM.
    const samples = new Float32Array(input);
    this.port.postMessage({ type: 'samples', payload: samples.buffer }, [samples.buffer]);
    return true;
  }
}

registerProcessor('microphone-tap-processor', MicrophoneTapProcessor);
