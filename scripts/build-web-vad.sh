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
SOURCE="${ROOT}/target/${TARGET}/release/voice_elf_web_vad.wasm"
HASH="$(shasum -a 256 "${SOURCE}" | awk '{print substr($1, 1, 16)}')"
FILENAME="voice_elf_web_vad.${HASH}.wasm"

find "${OUTPUT}" -maxdepth 1 -type f \
  \( -name 'voice_elf_web_vad.wasm' -o -name 'voice_elf_web_vad.*.wasm' \) \
  -delete
cp "${SOURCE}" "${OUTPUT}/${FILENAME}"
printf '{"file":"%s"}\n' "${FILENAME}" > "${OUTPUT}/manifest.json"

echo "Web VAD ready: ${OUTPUT}/${FILENAME}"
