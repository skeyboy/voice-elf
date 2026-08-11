#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
local_dir="$repo_root/.local"
source_dir="$local_dir/src/FunASR"
venv_dir="$local_dir/venvs/funasr"
run_dir="$local_dir/run/funasr"
pid_file="$run_dir/server.pid"
log_file="$run_dir/server.log"
uv_bin="$local_dir/bin/uv"
uv_cache_dir="$local_dir/cache/uv"
python_install_dir="$local_dir/python"
python_version=3.12.13
uv_version=0.11.32
funasr_version=1.4.1
modelscope_version=1.39.1
torch_version=2.11.0
revision=62b784b2b95ddc601b1e19abe99529b4ae34f07f
port=${FUNASR_PORT:-10095}
cpu_threads=${FUNASR_CPU_THREADS:-4}

export UV_CACHE_DIR="$uv_cache_dir"
export UV_MANAGED_PYTHON=1
export UV_NO_MODIFY_PATH=1
export UV_PYTHON_INSTALL_DIR="$python_install_dir"
export MODELSCOPE_CACHE="$local_dir/cache/modelscope"
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
  bootstrap_uv
  "$uv_bin" python install "$python_version" --install-dir "$python_install_dir" --no-bin
  revision_file="$source_dir/.voice-elf-revision"
  if test ! -f "$revision_file" || test "$(tr -d '[:space:]' < "$revision_file")" != "$revision"; then
    server_dir="$source_dir/runtime/python/websocket"
    server_file="$server_dir/funasr_wss_server.py"
    downloaded=$(mktemp "${TMPDIR:-/tmp}/voice-elf-funasr-server.XXXXXX")
    trap 'rm -f "$downloaded"' RETURN
    downloaded_ok=false
    for source_url in \
      "https://cdn.jsdelivr.net/gh/modelscope/FunASR@$revision/runtime/python/websocket/funasr_wss_server.py" \
      "https://raw.githubusercontent.com/modelscope/FunASR/$revision/runtime/python/websocket/funasr_wss_server.py"
    do
      if curl --fail --location --retry 3 --retry-all-errors --connect-timeout 15 \
        "$source_url" --output "$downloaded"; then
        downloaded_ok=true
        break
      fi
    done
    if test "$downloaded_ok" != true; then
      echo "Unable to download the pinned FunASR WebSocket server" >&2
      exit 1
    fi
    rm -rf "$source_dir"
    mkdir -p "$server_dir"
    mv "$downloaded" "$server_file"
    printf '%s\n' "$revision" > "$revision_file"
    trap - RETURN
  fi
  if test ! -x "$venv_dir/bin/python"; then
    mkdir -p "$(dirname "$venv_dir")"
    "$uv_bin" venv --python "$python_version" "$venv_dir"
  fi
  "$uv_bin" pip install --python "$venv_dir/bin/python" \
    "torch==$torch_version" \
    "torchaudio==$torch_version" \
    "funasr==$funasr_version" \
    "modelscope==$modelscope_version" \
    "websockets>=15,<16"
  echo "FunASR runtime is installed. Models are downloaded on the first start."
}

start() {
  if is_running; then
    echo "FunASR is already running (pid $(tr -d '[:space:]' < "$pid_file"))"
    exit 0
  fi
  test -x "$venv_dir/bin/python" || {
    echo "FunASR is not installed; run '$0 setup' first" >&2
    exit 1
  }
  server="$source_dir/runtime/python/websocket/funasr_wss_server.py"
  test -f "$server" || {
    echo "FunASR WebSocket server is missing; run '$0 setup' first" >&2
    exit 1
  }
  mkdir -p "$run_dir" "$MODELSCOPE_CACHE" "$HF_HOME"
  nohup "$venv_dir/bin/python" "$server" \
    --host 127.0.0.1 \
    --port "$port" \
    --ngpu 0 \
    --device cpu \
    --ncpu "$cpu_threads" \
    --certfile "" \
    --keyfile "" \
    </dev/null >"$log_file" 2>&1 &
  server_pid=$!
  echo "$server_pid" > "$pid_file"
  echo "FunASR starting at ws://127.0.0.1:$port (pid $server_pid)"
  echo "First startup downloads and loads the Paraformer, VAD, punctuation, and speaker models."
}

enable() {
  setup
  start
}

status() {
  if ! is_running; then
    echo "FunASR is stopped"
    exit 1
  fi
  server_pid=$(tr -d '[:space:]' < "$pid_file")
  if FUNASR_STATUS_URL="ws://127.0.0.1:$port/" "$venv_dir/bin/python" - <<'PY'
import asyncio
import os
import websockets

async def check():
    async with websockets.connect(os.environ["FUNASR_STATUS_URL"], subprotocols=["binary"], open_timeout=3):
        pass

asyncio.run(check())
PY
  then
    echo "FunASR is ready at ws://127.0.0.1:$port (pid $server_pid)"
  else
    echo "FunASR is running but still loading models (pid $server_pid)"
    exit 1
  fi
}

doctor() {
  echo "Project root: $repo_root"
  echo "Source: $source_dir"
  echo "Virtual environment: $venv_dir"
  echo "Model cache: $MODELSCOPE_CACHE"
  test -x "$uv_bin" || { echo "Status: uv is not installed" >&2; exit 1; }
  test -x "$venv_dir/bin/python" || { echo "Status: FunASR is not installed" >&2; exit 1; }
  test -f "$source_dir/runtime/python/websocket/funasr_wss_server.py" || {
    echo "Status: official WebSocket server is missing" >&2
    exit 1
  }
  "$venv_dir/bin/python" -c \
    'import funasr, modelscope, torch, torchaudio, websockets; print("Python imports: ok"); print("Torch:", torch.__version__)'
  test -f "$source_dir/.voice-elf-revision" || {
    echo "Status: managed source revision marker is missing" >&2
    exit 1
  }
  echo "Source revision: $(tr -d '[:space:]' < "$source_dir/.voice-elf-revision")"
}

stop() {
  if ! is_running; then
    echo "FunASR is already stopped"
    exit 0
  fi
  server_pid=$(tr -d '[:space:]' < "$pid_file")
  kill "$server_pid"
  for _ in {1..50}; do
    if ! kill -0 "$server_pid" 2>/dev/null; then
      : > "$pid_file"
      echo "FunASR stopped"
      exit 0
    fi
    sleep 0.1
  done
  echo "FunASR did not stop within five seconds (pid $server_pid)" >&2
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
