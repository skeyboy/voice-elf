#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
local_dir="$repo_root/.local"
venv_dir="$local_dir/venvs/qwen-tts-mlx"
run_dir="$local_dir/run/qwen-tts"
pid_file="$run_dir/server.pid"
log_file="$run_dir/server.log"
uv_bin="$local_dir/bin/uv"
uv_cache_dir="$local_dir/cache/uv"
python_install_dir="$local_dir/python"
python_version=3.12.13
uv_version=0.11.32
mlx_audio_version=0.4.8
port=${QWEN_TTS_PORT:-18085}
env_model=$(awk -F= '$1 == "TTS_QWEN_MODEL" {sub(/^[^=]*=/, ""); print; exit}' "$repo_root/.env" 2>/dev/null || true)
model=${TTS_QWEN_MODEL:-${env_model:-mlx-community/Qwen3-TTS-12Hz-0.6B-CustomVoice-bf16}}

export UV_CACHE_DIR="$uv_cache_dir"
export UV_MANAGED_PYTHON=1
export UV_NO_MODIFY_PATH=1
export UV_PYTHON_INSTALL_DIR="$python_install_dir"
export HF_HOME="$local_dir/cache/huggingface"
export HUGGINGFACE_HUB_CACHE="$HF_HOME/hub"

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
  mkdir -p "$local_dir/bin" "$uv_cache_dir"
  curl -LsSf "https://astral.sh/uv/$uv_version/install.sh" | \
    env UV_INSTALL_DIR="$local_dir/bin" UV_NO_MODIFY_PATH=1 sh
}

setup() {
  if test "$(uname -s)" != Darwin || test "$(uname -m)" != arm64; then
    echo "The managed Qwen TTS runtime uses MLX and requires Apple Silicon macOS." >&2
    echo "Use vLLM-Omni on Linux/CUDA and point TTS_QWEN_BASE_URL at that service." >&2
    exit 1
  fi
  bootstrap_uv
  "$uv_bin" python install "$python_version" --install-dir "$python_install_dir" --no-bin
  if test ! -x "$venv_dir/bin/python"; then
    mkdir -p "$(dirname "$venv_dir")"
    "$uv_bin" venv --python "$python_version" "$venv_dir"
  fi
  "$uv_bin" pip install --python "$venv_dir/bin/python" "mlx-audio[server]==$mlx_audio_version"
  QWEN_TTS_SETUP_MODEL="$model" "$venv_dir/bin/python" - <<'PY'
import os
from huggingface_hub import snapshot_download

snapshot_download(os.environ["QWEN_TTS_SETUP_MODEL"])
PY
  echo "Qwen3-TTS MLX runtime and model are ready: $model"
}

start() {
  if is_running; then
    echo "Qwen3-TTS is already running (pid $(tr -d '[:space:]' < "$pid_file"))"
    exit 0
  fi
  test -x "$venv_dir/bin/mlx_audio.server" || {
    echo "Qwen3-TTS MLX runtime is not installed; run '$0 setup' first" >&2
    exit 1
  }
  mkdir -p "$run_dir" "$HF_HOME"
  nohup "$venv_dir/bin/mlx_audio.server" \
    --host 127.0.0.1 \
    --port "$port" \
    </dev/null >"$log_file" 2>&1 &
  server_pid=$!
  echo "$server_pid" > "$pid_file"
  echo "Qwen3-TTS starting at http://127.0.0.1:$port/v1 (pid $server_pid)"
}

enable() {
  setup
  start
}

status() {
  if ! is_running; then
    echo "Qwen3-TTS is stopped"
    exit 1
  fi
  server_pid=$(tr -d '[:space:]' < "$pid_file")
  if curl --get --fail --silent --max-time 5 \
    --data-urlencode "model=$model" \
    "http://127.0.0.1:$port/v1/audio/voices" >/dev/null; then
    echo "Qwen3-TTS is ready at http://127.0.0.1:$port/v1 (pid $server_pid)"
  else
    echo "Qwen3-TTS is running but the API is not ready (pid $server_pid)"
    exit 1
  fi
}

doctor() {
  echo "Project root: $repo_root"
  echo "Virtual environment: $venv_dir"
  echo "Model: $model"
  echo "Model cache: $HUGGINGFACE_HUB_CACHE"
  test "$(uname -s)" = Darwin && test "$(uname -m)" = arm64 || {
    echo "Status: MLX requires Apple Silicon macOS" >&2
    exit 1
  }
  test -x "$venv_dir/bin/python" || { echo "Status: runtime is not installed" >&2; exit 1; }
  "$venv_dir/bin/python" -c \
    'import mlx, mlx_audio; print("Python imports: ok")'
  QWEN_TTS_DOCTOR_MODEL="$model" "$venv_dir/bin/python" - <<'PY'
import os
from huggingface_hub import scan_cache_dir

model = os.environ["QWEN_TTS_DOCTOR_MODEL"]
repos = {repo.repo_id for repo in scan_cache_dir().repos}
if model not in repos:
    raise SystemExit(f"Status: model is not downloaded: {model}")
print("Model cache: ok")
PY
}

stop() {
  if ! is_running; then
    echo "Qwen3-TTS is already stopped"
    exit 0
  fi
  server_pid=$(tr -d '[:space:]' < "$pid_file")
  kill "$server_pid"
  for _ in {1..50}; do
    if ! kill -0 "$server_pid" 2>/dev/null; then
      : > "$pid_file"
      echo "Qwen3-TTS stopped"
      exit 0
    fi
    sleep 0.1
  done
  echo "Qwen3-TTS did not stop within five seconds (pid $server_pid)" >&2
  exit 1
}

action=${1:-}
case "$action" in
  setup) setup ;;
  enable) enable ;;
  start) start ;;
  status) status ;;
  stop) stop ;;
  logs) tail -n 160 -f "$log_file" ;;
  doctor) doctor ;;
  *) usage; exit 2 ;;
esac
