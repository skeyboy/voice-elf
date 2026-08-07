#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
local_dir="$repo_root/.local"
source_dir="$local_dir/src/index-tts"
model_dir="${TTS_INDEX_MODEL_DIR:-${INDEX_TTS_MODEL_DIR:-$local_dir/models/tts/index-tts2}}"
run_dir="$local_dir/run/index-tts"
pid_file="$run_dir/server.pid"
log_file="$run_dir/server.log"
uv_bin="$local_dir/bin/uv"
uv_cache_dir="$local_dir/cache/uv"
port=${INDEX_TTS_PORT:-18084}
uv_version=0.11.32
repository_url="${INDEX_TTS_REPOSITORY_URL:-https://github.com/index-tts/index-tts.git}"
model_source="${INDEX_TTS_MODEL_SOURCE:-huggingface}"

export UV_CACHE_DIR="$uv_cache_dir"
export UV_NO_MODIFY_PATH=1
export HF_HOME="$local_dir/cache/huggingface"
export HUGGINGFACE_HUB_CACHE="$HF_HOME/hub"
export MPLCONFIGDIR="$local_dir/cache/matplotlib"

usage() {
  echo "usage: $0 setup|enable|start|status|stop|logs|doctor"
}

is_running() {
  test -f "$pid_file" && kill -0 "$(tr -d '[:space:]' < "$pid_file")" 2>/dev/null
}

bootstrap_uv() {
  if test -x "$uv_bin"; then
    return
  fi
  command -v curl >/dev/null 2>&1 || {
    echo "curl is required to install the project-local uv runtime" >&2
    exit 1
  }
  mkdir -p "$local_dir/bin" "$uv_cache_dir"
  curl -LsSf "https://astral.sh/uv/$uv_version/install.sh" | \
    env UV_INSTALL_DIR="$local_dir/bin" UV_NO_MODIFY_PATH=1 sh
}

model_is_ready() {
  local file
  for file in config.yaml bpe.model gpt.pth s2mel.pth wav2vec2bert_stats.pt; do
    test -f "$model_dir/$file" || return 1
  done
}

auxiliary_models_are_ready() {
  local file
  for file in \
    hf_cache/w2v-bert-2.0/config.json \
    hf_cache/w2v-bert-2.0/preprocessor_config.json \
    hf_cache/w2v-bert-2.0/model.safetensors \
    hf_cache/semantic_codec_model.safetensors \
    hf_cache/campplus_cn_common.bin \
    hf_cache/bigvgan/config.json \
    hf_cache/bigvgan/bigvgan_generator.pt; do
    test -f "$model_dir/$file" || return 1
  done
}

reference_is_ready() {
  local reference_audio
  reference_audio="${TTS_INDEX_DEFAULT_REFERENCE_AUDIO:-$source_dir/examples/voice_01.wav}"
  test -f "$reference_audio" || return 1
  test "$(head -c 4 "$reference_audio")" = "RIFF"
}

setup() {
  bootstrap_uv
  if test ! -d "$source_dir/.git"; then
    mkdir -p "$(dirname "$source_dir")"
    GIT_LFS_SKIP_SMUDGE=1 git clone --depth 1 "$repository_url" "$source_dir"
  fi
  if test -n "${INDEX_TTS_REVISION:-}"; then
    if test -n "$(git -C "$source_dir" status --porcelain --untracked-files=no)"; then
      echo "IndexTTS source has local changes; refusing to switch revisions" >&2
      exit 1
    fi
    git -C "$source_dir" fetch origin "$INDEX_TTS_REVISION"
    git -C "$source_dir" checkout --detach "$INDEX_TTS_REVISION"
  fi
  "$uv_bin" sync --project "$source_dir" --frozen
  mkdir -p "$model_dir"
  INDEX_TTS_SETUP_MODEL_DIR="$model_dir" INDEX_TTS_SETUP_SOURCE_DIR="$source_dir" \
    INDEX_TTS_SETUP_MODEL_SOURCE="$model_source" \
    "$uv_bin" run --project "$source_dir" python - <<'PY'
import os
from pathlib import Path

model_dir = Path(os.environ["INDEX_TTS_SETUP_MODEL_DIR"])
source_dir = Path(os.environ["INDEX_TTS_SETUP_SOURCE_DIR"])
model_source = os.environ["INDEX_TTS_SETUP_MODEL_SOURCE"].strip().lower()
if model_source == "modelscope":
    from modelscope import snapshot_download
    snapshot_download("IndexTeam/IndexTTS-2", local_dir=str(model_dir))
elif model_source == "huggingface":
    from huggingface_hub import snapshot_download
    snapshot_download("IndexTeam/IndexTTS-2", local_dir=model_dir)
else:
    raise SystemExit("INDEX_TTS_MODEL_SOURCE must be 'huggingface' or 'modelscope'")

try:
    from indextts.utils.model_download import ensure_config_available, ensure_models_available
    ensure_config_available(str(model_dir))
    if model_source == "modelscope":
        from modelscope.hub.file_download import model_file_download

        w2v_dir = model_dir / "hf_cache" / "w2v-bert-2.0"
        w2v_dir.mkdir(parents=True, exist_ok=True)
        for filename in (
            "config.json",
            "configuration.json",
            "preprocessor_config.json",
            "model.safetensors",
        ):
            if not (w2v_dir / filename).is_file():
                model_file_download(
                    model_id="AI-ModelScope/w2v-bert-2.0",
                    file_path=filename,
                    local_dir=str(w2v_dir),
                )
    ensure_models_available(str(model_dir))
except (ImportError, TypeError):
    pass

reference = source_dir / "examples" / "voice_01.wav"
if not reference.is_file() or reference.read_bytes()[:4] != b"RIFF":
    from indextts.utils.examples_downloader import ensure_examples_available
    previous = Path.cwd()
    os.chdir(source_dir)
    try:
        ensure_examples_available()
    finally:
        os.chdir(previous)
PY
  model_is_ready || {
    echo "IndexTTS2 model download is incomplete" >&2
    exit 1
  }
  auxiliary_models_are_ready || {
    echo "IndexTTS2 auxiliary model download is incomplete" >&2
    exit 1
  }
  reference_is_ready || {
    echo "IndexTTS2 default reference audio is missing or invalid" >&2
    exit 1
  }
  echo "IndexTTS2 runtime and model are ready"
  echo "Source revision: $(git -C "$source_dir" rev-parse HEAD)"
}

start() {
  if is_running; then
    echo "IndexTTS2 is already running (pid $(tr -d '[:space:]' < "$pid_file"))"
    exit 0
  fi
  test -x "$uv_bin" && test -f "$source_dir/pyproject.toml" || {
    echo "IndexTTS2 is not installed; run '$0 setup' first" >&2
    exit 1
  }
  model_is_ready || {
    echo "IndexTTS2 model is missing; run '$0 setup' first" >&2
    exit 1
  }
  auxiliary_models_are_ready || {
    echo "IndexTTS2 auxiliary models are missing; run '$0 setup' first" >&2
    exit 1
  }
  reference_is_ready || {
    echo "IndexTTS2 reference audio is missing; run '$0 setup' first" >&2
    exit 1
  }
  mkdir -p "$run_dir" "$MPLCONFIGDIR"
  service_args=(
    --host 127.0.0.1
    --port "$port"
    --source-dir "$source_dir"
    --model-dir "$model_dir"
  )
  if test "${INDEX_TTS_FP16:-false}" = true; then
    service_args+=(--fp16)
  fi
  nohup env PYTHONUNBUFFERED=1 PYTHONPATH="$source_dir${PYTHONPATH:+:$PYTHONPATH}" \
    "$uv_bin" run --project "$source_dir" \
    --with fastapi --with uvicorn --with python-multipart \
    python "$repo_root/scripts/index-tts-service.py" \
    "${service_args[@]}" \
    </dev/null >"$log_file" 2>&1 &
  server_pid=$!
  echo "$server_pid" > "$pid_file"
  echo "IndexTTS2 starting at http://127.0.0.1:$port (pid $server_pid)"
}

enable() {
  setup
  start
}

status() {
  if ! is_running; then
    echo "IndexTTS2 is stopped"
    exit 1
  fi
  server_pid=$(tr -d '[:space:]' < "$pid_file")
  curl_args=(--fail --silent --max-time 3)
  if test -n "${TTS_INDEX_API_KEY:-}"; then
    curl_args+=(-H "Authorization: Bearer $TTS_INDEX_API_KEY")
  fi
  if curl "${curl_args[@]}" "http://127.0.0.1:$port/health" >/dev/null; then
    echo "IndexTTS2 is ready at http://127.0.0.1:$port (pid $server_pid)"
  else
    echo "IndexTTS2 is running but still initializing (pid $server_pid)"
  fi
}

doctor() {
  echo "Project root: $repo_root"
  echo "uv: $uv_bin"
  echo "IndexTTS source: $source_dir"
  echo "Model: $model_dir"
  test -x "$uv_bin" || { echo "Status: uv is not installed" >&2; exit 1; }
  test -f "$source_dir/pyproject.toml" || { echo "Status: source is not installed" >&2; exit 1; }
  model_is_ready || { echo "Status: model is not downloaded or incomplete" >&2; exit 1; }
  auxiliary_models_are_ready || { echo "Status: auxiliary models are not downloaded or incomplete" >&2; exit 1; }
  reference_is_ready || { echo "Status: reference audio is missing or invalid" >&2; exit 1; }
  "$uv_bin" run --project "$source_dir" python -c \
    'from indextts.infer_v2 import IndexTTS2; print("Python imports: ok")'
  echo "Source revision: $(git -C "$source_dir" rev-parse HEAD)"
}

stop() {
  if ! is_running; then
    echo "IndexTTS2 is already stopped"
    exit 0
  fi
  server_pid=$(tr -d '[:space:]' < "$pid_file")
  kill "$server_pid"
  for _ in {1..30}; do
    if ! kill -0 "$server_pid" 2>/dev/null; then
      : > "$pid_file"
      echo "IndexTTS2 stopped"
      exit 0
    fi
    sleep 0.1
  done
  echo "IndexTTS2 did not stop within three seconds (pid $server_pid)" >&2
  exit 1
}

action=${1:-}
case "$action" in
  setup) setup ;;
  enable) enable ;;
  start) start ;;
  status) status ;;
  stop) stop ;;
  logs) tail -n 120 -f "$log_file" ;;
  doctor) doctor ;;
  *) usage; exit 2 ;;
esac
