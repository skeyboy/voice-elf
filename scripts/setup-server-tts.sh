#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
download_dir="$repo_root/.local/downloads"
model_root="$repo_root/.local/models/tts"
kokoro_name="kokoro-int8-multi-lang-v1_1"
supertonic_name="sherpa-onnx-supertonic-3-tts-int8-2026-05-11"
kokoro_archive="$download_dir/kokoro-int8-multi-lang-v1_1.tar.bz2"
supertonic_archive="$download_dir/$supertonic_name.tar.bz2"
kokoro_url="https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-int8-multi-lang-v1_1.tar.bz2"
supertonic_url="https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/$supertonic_name.tar.bz2"

mkdir -p "$download_dir" "$model_root"

download() {
  local url="$1"
  local destination="$2"
  if [[ ! -s "$destination" ]]; then
    curl --fail --location --retry 3 --output "$destination.part" "$url"
    mv "$destination.part" "$destination"
  fi
}

install_model() {
  local archive="$1"
  local expected_dir="$2"
  if [[ ! -d "$model_root/$expected_dir" ]]; then
    tar -xjf "$archive" -C "$model_root"
  fi
}

download "$kokoro_url" "$kokoro_archive"
download "$supertonic_url" "$supertonic_archive"
install_model "$kokoro_archive" "$kokoro_name"
install_model "$supertonic_archive" "$supertonic_name"

test -f "$model_root/$kokoro_name/model.int8.onnx"
test -f "$model_root/$supertonic_name/vocoder.int8.onnx"

echo "Server TTS models are ready under $model_root"
