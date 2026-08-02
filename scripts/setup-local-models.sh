#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOCAL_DIR="${ROOT}/.local"
SRC_DIR="${LOCAL_DIR}/src"
BIN_DIR="${LOCAL_DIR}/bin"
MODEL_DIR="${LOCAL_DIR}/models"
BUILD_DIR="${LOCAL_DIR}/build"
MODEL_ENDPOINT="${MODEL_ENDPOINT:-https://www.modelscope.cn/models}"
MODEL_REVISION="${MODEL_REVISION:-master}"

mkdir -p "${SRC_DIR}" "${BIN_DIR}" "${MODEL_DIR}" "${BUILD_DIR}"

clone_if_missing() {
  local url="$1"
  local directory="$2"
  if [[ ! -d "${directory}/.git" ]]; then
    git clone --depth 1 "${url}" "${directory}"
  fi
}

download() {
  local repo="$1"
  local file="$2"
  local destination="$3"
  local sha256="${4:-}"
  mkdir -p "$(dirname "${destination}")"

  if [[ -f "${destination}" && -n "${sha256}" ]]; then
    if [[ "$(shasum -a 256 "${destination}" | awk '{print $1}')" == "${sha256}" ]]; then
      echo "[ok] ${destination}"
      return
    fi
  elif [[ -f "${destination}" ]]; then
    echo "[ok] ${destination}"
    return
  fi

  echo "[download] ${repo}/${file}"
  local url="${MODEL_ENDPOINT}/${repo}/resolve/${MODEL_REVISION}/${file}"
  if command -v aria2c >/dev/null 2>&1; then
    aria2c --continue=true --max-connection-per-server=8 --split=8 \
      --min-split-size=4M --file-allocation=none --auto-file-renaming=false \
      --allow-overwrite=true --max-tries=8 --retry-wait=3 --summary-interval=10 \
      --dir="$(dirname "${destination}")" --out="$(basename "${destination}")" \
      "${url}"
  else
    curl --fail --location --retry 8 --retry-all-errors --continue-at - \
      --output "${destination}" "${url}"
  fi

  if [[ -n "${sha256}" ]]; then
    local actual
    actual="$(shasum -a 256 "${destination}" | awk '{print $1}')"
    if [[ "${actual}" != "${sha256}" ]]; then
      echo "Checksum mismatch for ${destination}" >&2
      exit 1
    fi
  fi
}

clone_if_missing "https://github.com/antirez/qwen-asr.git" "${SRC_DIR}/qwen-asr"
clone_if_missing "https://github.com/ggml-org/llama.cpp.git" "${SRC_DIR}/llama.cpp"

ASR_PATCH="${ROOT}/scripts/patches/qwen-asr-low-latency.patch"
if git -C "${SRC_DIR}/qwen-asr" apply --check "${ASR_PATCH}"; then
  git -C "${SRC_DIR}/qwen-asr" apply "${ASR_PATCH}"
elif git -C "${SRC_DIR}/qwen-asr" apply --reverse --check "${ASR_PATCH}"; then
  echo "[ok] Qwen3 ASR low-latency CLI patch"
else
  echo "Qwen3 ASR source is incompatible with ${ASR_PATCH}" >&2
  exit 1
fi

echo "[build] Qwen3 ASR with Apple Accelerate"
make -C "${SRC_DIR}/qwen-asr" blas
cp "${SRC_DIR}/qwen-asr/qwen_asr" "${BIN_DIR}/qwen_asr"

echo "[build] llama.cpp translator"
cmake -S "${SRC_DIR}/llama.cpp" -B "${BUILD_DIR}/llama.cpp" \
  -DCMAKE_BUILD_TYPE=Release \
  -DGGML_METAL=OFF \
  -DGGML_ACCELERATE=ON \
  -DLLAMA_CURL=OFF \
  -DLLAMA_BUILD_SERVER=ON \
  -DLLAMA_BUILD_TOOLS=ON \
  -DLLAMA_BUILD_TESTS=OFF \
  -DLLAMA_BUILD_EXAMPLES=ON
cmake --build "${BUILD_DIR}/llama.cpp" --target llama-completion --parallel 8
cp "${BUILD_DIR}/llama.cpp/bin/llama-completion" "${BIN_DIR}/llama-completion"
cp "${BUILD_DIR}/llama.cpp/bin/"*.dylib "${BIN_DIR}/"

ASR_DIR="${MODEL_DIR}/qwen3-asr-0.6b"
download "Qwen/Qwen3-ASR-0.6B" "config.json" "${ASR_DIR}/config.json"
download "Qwen/Qwen3-ASR-0.6B" "generation_config.json" "${ASR_DIR}/generation_config.json"
download "Qwen/Qwen3-ASR-0.6B" "vocab.json" "${ASR_DIR}/vocab.json"
download "Qwen/Qwen3-ASR-0.6B" "merges.txt" "${ASR_DIR}/merges.txt"

LLM_DIR="${MODEL_DIR}/qwen3-0.6b"

echo "[download] model weights"
download "Qwen/Qwen3-ASR-0.6B" "model.safetensors" "${ASR_DIR}/model.safetensors" \
  "79d6cbd4c98c7bbffe9db2edac07f56cd6637d0d5944b27f6c2b8353840323ea"
download "Qwen/Qwen3-0.6B-GGUF" "Qwen3-0.6B-Q8_0.gguf" "${LLM_DIR}/Qwen3-0.6B-Q8_0.gguf" \
  "9465e63a22add5354d9bb4b99e90117043c7124007664907259bd16d043bb031"

echo
echo "Local model setup complete."
echo "Binaries: ${BIN_DIR}"
echo "Models:   ${MODEL_DIR}"
