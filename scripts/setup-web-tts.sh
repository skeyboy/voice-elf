#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUTPUT="$ROOT/web/static/models/supertonic3"
ENDPOINT="${TTS_MODEL_ENDPOINT:-https://huggingface.co}"
REPOSITORY="Supertone/supertonic-3"

download() {
  local remote="$1"
  local destination="$OUTPUT/$2"
  mkdir -p "$(dirname "$destination")"
  if [ ! -s "$destination" ]; then
    curl --fail --location --retry 8 --retry-all-errors --continue-at - \
      "$ENDPOINT/$REPOSITORY/resolve/main/$remote" --output "$destination"
  fi
}

download onnx/text_encoder.onnx onnx/text_encoder.onnx
download onnx/duration_predictor.onnx onnx/duration_predictor.onnx
download onnx/vector_estimator.onnx onnx/vector_estimator.onnx
download onnx/vocoder.onnx onnx/vocoder.onnx
download onnx/tts.json onnx/tts.json
download onnx/unicode_indexer.json onnx/unicode_indexer.json
download voice_styles/M1.json voice_styles/M1.json
download voice_styles/F1.json voice_styles/F1.json
download LICENSE LICENSE
du -sh "$OUTPUT"
