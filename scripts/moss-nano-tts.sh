#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
local_dir="$repo_root/.local"
source_dir="$repo_root/.local/src/MOSS-TTS-Nano"
venv_dir="$repo_root/.local/venvs/moss-tts-nano"
run_dir="$repo_root/.local/run/moss-nano-tts"
pid_file="$run_dir/server.pid"
log_file="$run_dir/server.log"
uv_bin="$local_dir/bin/uv"
python_install_dir="$local_dir/python"
uv_cache_dir="$local_dir/cache/uv"
requirements_lock="$repo_root/scripts/moss-nano-tts-requirements.lock"
python_version=3.12.13
openfst_version=1.8.4
openfst_prefix="$local_dir/openfst/$openfst_version"
openfst_archive="$local_dir/cache/downloads/openfst-$openfst_version.tar.gz"
openfst_sha256=a8ebbb6f3d92d07e671500587472518cfc87cb79b9a654a5a8abb2d0eb298016
port=${MOSS_NANO_PORT:-18083}
revision=cc7bdf19c7639c0870dab22045a33b442760f6be
uv_version=0.11.32

export UV_CACHE_DIR="$uv_cache_dir"
export UV_MANAGED_PYTHON=1
export UV_NO_MODIFY_PATH=1
export UV_PYTHON_INSTALL_DIR="$python_install_dir"
export HF_HOME="$local_dir/cache/huggingface"
export HUGGINGFACE_HUB_CACHE="$HF_HOME/hub"
export XDG_CACHE_HOME="$local_dir/cache"
export DYLD_LIBRARY_PATH="$openfst_prefix/lib${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}"
export LD_LIBRARY_PATH="$openfst_prefix/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

usage() {
  echo "usage: $0 setup|start|status|stop|logs|doctor"
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
  echo "Installing uv $uv_version into $local_dir/bin"
  curl -LsSf "https://astral.sh/uv/$uv_version/install.sh" | \
    env UV_INSTALL_DIR="$local_dir/bin" UV_NO_MODIFY_PATH=1 sh
}

install_python() {
  bootstrap_uv
  mkdir -p "$python_install_dir"
  "$uv_bin" python install "$python_version" --install-dir "$python_install_dir" --no-bin
}

install_openfst() {
  if test -f "$openfst_prefix/include/fst/extensions/mpdt/compose.h"; then
    return
  fi

  command -v make >/dev/null 2>&1 || {
    echo "make is required to build the project-local OpenFST dependency" >&2
    exit 1
  }
  mkdir -p "$(dirname "$openfst_archive")" "$local_dir/build"
  if test ! -f "$openfst_archive"; then
    echo "Downloading OpenFST $openfst_version"
    curl -fL "https://openfst.org/twiki/pub/FST/FstDownload/openfst-$openfst_version.tar.gz" \
      -o "$openfst_archive"
  fi
  echo "$openfst_sha256  $openfst_archive" | shasum -a 256 --check

  build_dir="$local_dir/build/openfst-$openfst_version"
  if test ! -x "$build_dir/configure"; then
    mkdir -p "$build_dir"
    tar -xzf "$openfst_archive" --strip-components=1 -C "$build_dir"
  fi
  jobs=$(sysctl -n hw.logicalcpu 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)
  echo "Building OpenFST $openfst_version in the project-local runtime"
  (
    cd "$build_dir"
    ./configure \
      --prefix="$openfst_prefix" \
      --enable-far \
      --enable-mpdt \
      --enable-ngram-fsts \
      --enable-pdt \
      --disable-dependency-tracking
    make -j "$jobs"
    make install
  )
}

install_text_normalization() {
  if "$venv_dir/bin/python" -c 'import pynini, tn' >/dev/null 2>&1; then
    return
  fi

  if "$uv_bin" pip install --python "$venv_dir/bin/python" \
    "WeTextProcessing==1.2.0"; then
    return
  fi
  if test "$(uname -s)" != Darwin; then
    echo "WeTextProcessing installation failed; install a platform pynini wheel and retry" >&2
    exit 1
  fi

  install_openfst
  env \
    CPPFLAGS="-I$openfst_prefix/include" \
    CPLUS_INCLUDE_PATH="$openfst_prefix/include" \
    LDFLAGS="-L$openfst_prefix/lib -Wl,-rpath,$openfst_prefix/lib" \
    LIBRARY_PATH="$openfst_prefix/lib" \
    "$uv_bin" pip install --python "$venv_dir/bin/python" "pynini==2.1.7"
  "$uv_bin" pip install --python "$venv_dir/bin/python" \
    "WeTextProcessing==1.2.0"
}

setup() {
  install_python
  if test ! -d "$source_dir/.git"; then
    mkdir -p "$(dirname "$source_dir")"
    git clone https://github.com/OpenMOSS/MOSS-TTS-Nano.git "$source_dir"
  fi
  if test -n "$(git -C "$source_dir" status --porcelain --untracked-files=no)"; then
    echo "MOSS-TTS-Nano source has local changes; refusing to replace its revision" >&2
    exit 1
  fi
  git -C "$source_dir" fetch origin "$revision"
  git -C "$source_dir" checkout --detach "$revision"

  if test ! -x "$venv_dir/bin/python"; then
    mkdir -p "$(dirname "$venv_dir")"
    "$uv_bin" venv --python "$python_version" "$venv_dir"
  fi
  installed_python_version=$("$venv_dir/bin/python" -c \
    'import platform; print(platform.python_version())')
  if test "$installed_python_version" != "$python_version"; then
    echo "Existing virtual environment uses Python $installed_python_version, expected $python_version: $venv_dir" >&2
    echo "Move it aside and rerun '$0 setup'." >&2
    exit 1
  fi

  "$uv_bin" pip install --python "$venv_dir/bin/python" \
    -r "$requirements_lock"
  "$uv_bin" pip install --python "$venv_dir/bin/python" \
    --no-deps -e "$source_dir"
  install_text_normalization
  echo "MOSS-TTS-Nano ONNX runtime is ready in $venv_dir"
}

start() {
  if is_running; then
    echo "MOSS-TTS-Nano is already running (pid $(tr -d '[:space:]' < "$pid_file"))"
    exit 0
  fi
  if test ! -x "$venv_dir/bin/moss-tts-nano"; then
    echo "MOSS-TTS-Nano is not installed; run '$0 setup' first" >&2
    exit 1
  fi
  mkdir -p "$run_dir"
  nohup "$venv_dir/bin/moss-tts-nano" serve \
    --backend onnx \
    --execution-provider cpu \
    --cpu-threads "${MOSS_NANO_CPU_THREADS:-4}" \
    --host 127.0.0.1 \
    --port "$port" \
    </dev/null >"$log_file" 2>&1 &
  server_pid=$!
  echo "$server_pid" > "$pid_file"
  echo "MOSS-TTS-Nano starting at http://127.0.0.1:$port (pid $server_pid)"
  echo "First startup downloads the ONNX models; inspect progress with '$0 logs'."
}

status() {
  if ! is_running; then
    echo "MOSS-TTS-Nano is stopped"
    exit 1
  fi
  server_pid=$(tr -d '[:space:]' < "$pid_file")
  if curl --fail --silent --max-time 2 "http://127.0.0.1:$port/health" >/dev/null; then
    echo "MOSS-TTS-Nano is ready at http://127.0.0.1:$port (pid $server_pid)"
  else
    echo "MOSS-TTS-Nano is running but still initializing (pid $server_pid)"
  fi
}

doctor() {
  echo "Project root: $repo_root"
  echo "uv: $uv_bin"
  echo "Python installs: $python_install_dir"
  echo "Virtual environment: $venv_dir"
  echo "MOSS source: $source_dir"
  echo "OpenFST: $openfst_prefix"
  echo "Cache root: $local_dir/cache"

  test -x "$uv_bin" || {
    echo "Status: uv is not installed; run '$0 setup'" >&2
    exit 1
  }
  "$uv_bin" --version
  test -x "$venv_dir/bin/python" || {
    echo "Status: Python environment is not installed; run '$0 setup'" >&2
    exit 1
  }
  "$venv_dir/bin/python" --version
  "$venv_dir/bin/python" -c \
    'import fastapi, moss_tts_nano, onnxruntime, pynini, soundfile, tn, torch; print("Python imports: ok")'
  echo "Status: project-local runtime is ready"
}

stop() {
  if ! is_running; then
    echo "MOSS-TTS-Nano is already stopped"
    exit 0
  fi
  server_pid=$(tr -d '[:space:]' < "$pid_file")
  kill "$server_pid"
  for _ in {1..20}; do
    if ! kill -0 "$server_pid" 2>/dev/null; then
      : > "$pid_file"
      echo "MOSS-TTS-Nano stopped"
      exit 0
    fi
    sleep 0.1
  done
  echo "MOSS-TTS-Nano did not stop within two seconds (pid $server_pid)" >&2
  exit 1
}

action=${1:-}
case "$action" in
  setup) setup ;;
  start) start ;;
  status) status ;;
  stop) stop ;;
  logs) tail -n 120 -f "$log_file" ;;
  doctor) doctor ;;
  *) usage; exit 2 ;;
esac
