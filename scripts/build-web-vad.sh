#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="wasm32-unknown-unknown"
OUTPUT="${ROOT}/web/static/wasm"

if ! rustup target list --installed | grep -qx "${TARGET}"; then
  rustup target add "${TARGET}"
fi

cargo build \
  --manifest-path "${ROOT}/Cargo.toml" \
  --package voice-elf-web-vad \
  --target "${TARGET}" \
  --release

mkdir -p "${OUTPUT}"
cp "${ROOT}/target/${TARGET}/release/voice_elf_web_vad.wasm" \
  "${OUTPUT}/voice_elf_web_vad.wasm"

echo "Web VAD ready: ${OUTPUT}/voice_elf_web_vad.wasm"
