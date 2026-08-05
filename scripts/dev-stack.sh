#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_DIR="${ROOT}/.local/run/dev-stack"
SERVER_PORT="${VOICE_ELF_DEV_SERVER_PORT:-3001}"
WEB_PORT="${VOICE_ELF_DEV_WEB_PORT:-5173}"
MOSS_PORT="${MOSS_NANO_PORT:-18083}"
START_TIMEOUT="${VOICE_ELF_DEV_START_TIMEOUT:-120}"

usage() {
  cat <<'EOF'
用法：
  ./scripts/dev-stack.sh start
  ./scripts/dev-stack.sh stop
  ./scripts/dev-stack.sh restart
  ./scripts/dev-stack.sh status
  ./scripts/dev-stack.sh logs [moss|server|web]

也可以执行：make dev / make dev-stop / make dev-status
EOF
}

uses_launchd() {
  [[ "$(uname -s)" == Darwin ]] && command -v launchctl >/dev/null 2>&1
}

uses_screen() {
  command -v screen >/dev/null 2>&1
}

label_for() { printf 'com.voice-elf.dev.%s\n' "$1"; }
screen_name() { printf 'voice-elf-dev-%s\n' "$1"; }
pid_file() { printf '%s/%s.pid\n' "${RUN_DIR}" "$1"; }
log_file() { printf '%s/%s.log\n' "${RUN_DIR}" "$1"; }
plist_file() { printf '%s/%s.plist\n' "${RUN_DIR}" "$1"; }

port_for() {
  case "$1" in
    moss) printf '%s\n' "${MOSS_PORT}" ;;
    server) printf '%s\n' "${SERVER_PORT}" ;;
    web) printf '%s\n' "${WEB_PORT}" ;;
    *) return 1 ;;
  esac
}

name_for() {
  case "$1" in
    moss) printf '%s\n' 'MOSS-TTS-Nano' ;;
    server) printf '%s\n' 'Voice Elf 服务端' ;;
    web) printf '%s\n' 'Voice Elf Web' ;;
    *) return 1 ;;
  esac
}

health_url_for() {
  case "$1" in
    moss) printf 'http://127.0.0.1:%s/health\n' "${MOSS_PORT}" ;;
    server) printf 'http://127.0.0.1:%s/api/health\n' "${SERVER_PORT}" ;;
    web) printf 'http://127.0.0.1:%s/api/health\n' "${WEB_PORT}" ;;
    *) return 1 ;;
  esac
}

expected_command_for() {
  case "$1" in
    moss) printf '%s\n' 'moss-tts-nano serve' ;;
    server) printf '%s\n' 'target/debug/voice-elf-server' ;;
    web) printf '%s\n' 'vite' ;;
    *) return 1 ;;
  esac
}

launchd_exists() {
  launchctl print "gui/$(id -u)/$(label_for "$1")" >/dev/null 2>&1
}

launchd_pid() {
  launchctl print "gui/$(id -u)/$(label_for "$1")" 2>/dev/null \
    | awk '/^[[:space:]]*pid = [0-9]+/{print $3; exit}'
}

screen_pid() {
  local session
  session="$(screen_name "$1")"
  screen -ls 2>/dev/null | awk -v suffix=".${session}" \
    'index($1, suffix) { split($1, parts, "."); print parts[1]; exit }'
}

screen_exists() {
  local pid
  pid="$(screen_pid "$1")"
  [[ "${pid}" =~ ^[0-9]+$ ]] && kill -0 "${pid}" 2>/dev/null
}

managed_pid() {
  local component="$1" file pid command expected listener
  if uses_screen; then
    screen_exists "${component}" || return 1
    listener="$(port_pids "$(port_for "${component}")" | head -n 1)"
    pid="${listener:-$(screen_pid "${component}")}"
    [[ "${pid}" =~ ^[0-9]+$ ]] || return 1
    printf '%s\n' "${pid}" > "$(pid_file "${component}")"
    printf '%s\n' "${pid}"
    return
  fi
  if uses_launchd; then
    launchd_exists "${component}" || return 1
    pid="$(launchd_pid "${component}")"
    [[ "${pid}" =~ ^[0-9]+$ ]] || return 1
    kill -0 "${pid}" 2>/dev/null || return 1
    printf '%s\n' "${pid}" > "$(pid_file "${component}")"
    printf '%s\n' "${pid}"
    return
  fi

  file="$(pid_file "${component}")"
  [[ -f "${file}" ]] || return 1
  pid="$(tr -d '[:space:]' < "${file}")"
  [[ "${pid}" =~ ^[0-9]+$ ]] || return 1
  kill -0 "${pid}" 2>/dev/null || return 1
  command="$(ps -p "${pid}" -o command= 2>/dev/null || true)"
  expected="$(expected_command_for "${component}")"
  [[ "${command}" == *"${expected}"* ]] || return 1
  printf '%s\n' "${pid}"
}

port_pids() {
  lsof -tiTCP:"$1" -sTCP:LISTEN 2>/dev/null | sort -u || true
}

assert_port_available() {
  local component="$1" port managed occupied pid command
  port="$(port_for "${component}")"
  managed="$(managed_pid "${component}" 2>/dev/null || true)"
  occupied="$(port_pids "${port}")"
  [[ -z "${occupied}" ]] && return
  if [[ -n "${managed}" ]] && [[ "${occupied}" == *"${managed}"* ]]; then
    return
  fi

  echo "端口 ${port} 已被非 dev-stack 进程占用：" >&2
  while IFS= read -r pid; do
    [[ -n "${pid}" ]] || continue
    command="$(ps -p "${pid}" -o command= 2>/dev/null || true)"
    echo "  PID ${pid}: ${command}" >&2
  done <<< "${occupied}"
  echo "为避免误杀进程，dev-stack 未继续启动。" >&2
  return 1
}

database_url() {
  [[ -f "${ROOT}/.env" ]] || return
  awk '/^DATABASE_URL=/{sub(/^DATABASE_URL=/, ""); print; exit}' "${ROOT}/.env"
}

check_postgres() {
  local url
  url="$(database_url)"
  [[ -n "${url}" ]] || {
    echo "PostgreSQL：未配置 DATABASE_URL，服务端将使用内存存储"
    return
  }
  if command -v pg_isready >/dev/null 2>&1; then
    if ! pg_isready --dbname="${url}" --timeout=3 >/dev/null; then
      echo "PostgreSQL 不可用，请先启动数据库：${url%%\?*}" >&2
      return 1
    fi
  else
    if ! nc -z 127.0.0.1 5432 >/dev/null 2>&1; then
      echo "PostgreSQL 端口 5432 不可用，请先启动数据库" >&2
      return 1
    fi
  fi
  echo "PostgreSQL：可用"
}

prepare_dependencies() {
  check_postgres
  if ! "${ROOT}/scripts/moss-nano-tts.sh" doctor >/dev/null 2>&1; then
    if [[ "${VOICE_ELF_DEV_AUTO_SETUP:-1}" != 1 ]]; then
      echo "MOSS Python 环境缺失，请执行 ./scripts/moss-nano-tts.sh setup" >&2
      return 1
    fi
    "${ROOT}/scripts/moss-nano-tts.sh" setup
  fi

  if [[ ! -d "${ROOT}/web/node_modules" ]]; then
    if [[ -f "${ROOT}/web/package-lock.json" ]]; then
      (cd "${ROOT}/web" && npm ci)
    else
      (cd "${ROOT}/web" && npm install)
    fi
  fi
  "${ROOT}/scripts/build-web-vad.sh"
  cargo build --manifest-path "${ROOT}/Cargo.toml" --bin voice-elf-server
}

run_component() {
  local component="$1"
  export PATH="${ROOT}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:${PATH:-}"
  case "${component}" in
    moss)
      export HF_HOME="${ROOT}/.local/cache/huggingface"
      export HUGGINGFACE_HUB_CACHE="${HF_HOME}/hub"
      export XDG_CACHE_HOME="${ROOT}/.local/cache"
      export DYLD_LIBRARY_PATH="${ROOT}/.local/openfst/1.8.4/lib${DYLD_LIBRARY_PATH:+:${DYLD_LIBRARY_PATH}}"
      export LD_LIBRARY_PATH="${ROOT}/.local/openfst/1.8.4/lib${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}"
      exec "${ROOT}/.local/venvs/moss-tts-nano/bin/moss-tts-nano" serve \
        --backend onnx \
        --execution-provider cpu \
        --cpu-threads "${MOSS_NANO_CPU_THREADS:-4}" \
        --host 127.0.0.1 \
        --port "${MOSS_PORT}"
      ;;
    server)
      export VOICE_ELF_BIND="0.0.0.0:${SERVER_PORT}"
      cd "${ROOT}"
      exec "${ROOT}/target/debug/voice-elf-server"
      ;;
    web)
      cd "${ROOT}/web"
      exec "${ROOT}/web/node_modules/.bin/vite" \
        --host 0.0.0.0 \
        --port "${WEB_PORT}" \
        --strictPort
      ;;
    *) echo "未知组件：${component}" >&2; return 2 ;;
  esac
}

write_launchd_plist() {
  local component="$1" plist label log
  plist="$(plist_file "${component}")"
  label="$(label_for "${component}")"
  log="$(log_file "${component}")"
  cat > "${plist}" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>${label}</string>
    <key>ProgramArguments</key>
    <array>
      <string>/bin/bash</string>
      <string>${ROOT}/scripts/dev-stack.sh</string>
      <string>_run</string>
      <string>${component}</string>
    </array>
    <key>WorkingDirectory</key>
    <string>${ROOT}</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ThrottleInterval</key>
    <integer>3</integer>
    <key>StandardOutPath</key>
    <string>${log}</string>
    <key>StandardErrorPath</key>
    <string>${log}</string>
  </dict>
</plist>
EOF
  plutil -lint "${plist}" >/dev/null
}

start_component() {
  local component="$1" pid log
  if pid="$(managed_pid "${component}" 2>/dev/null)"; then
    echo "$(name_for "${component}")：已运行 (PID ${pid})"
    return 10
  fi
  assert_port_available "${component}"
  log="$(log_file "${component}")"
  : > "${log}"

  if uses_screen; then
    if uses_launchd && launchd_exists "${component}"; then
      launchctl bootout "gui/$(id -u)/$(label_for "${component}")" >/dev/null 2>&1 || true
    fi
    screen -dmS "$(screen_name "${component}")" \
      /bin/bash "${ROOT}/scripts/dev-stack.sh" _run_logged "${component}"
  elif uses_launchd; then
    launchctl bootout "gui/$(id -u)/$(label_for "${component}")" >/dev/null 2>&1 || true
    write_launchd_plist "${component}"
    launchctl bootstrap "gui/$(id -u)" "$(plist_file "${component}")"
  else
    nohup "${ROOT}/scripts/dev-stack.sh" _run "${component}" \
      </dev/null >"${log}" 2>&1 &
    printf '%s\n' "$!" > "$(pid_file "${component}")"
  fi

  for _ in {1..20}; do
    if pid="$(managed_pid "${component}" 2>/dev/null)"; then
      echo "$(name_for "${component}")：正在启动 (PID ${pid})"
      return 0
    fi
    sleep 0.25
  done
  echo "$(name_for "${component}")启动失败，日志：${log}" >&2
  tail -n 30 "${log}" >&2 || true
  return 1
}

wait_healthy() {
  local component="$1" url elapsed pid log
  url="$(health_url_for "${component}")"
  log="$(log_file "${component}")"
  elapsed=0
  while (( elapsed < START_TIMEOUT )); do
    if curl --fail --silent --max-time 3 "${url}" >/dev/null 2>&1; then
      pid="$(managed_pid "${component}" 2>/dev/null || true)"
      echo "$(name_for "${component}")：就绪 (PID ${pid})"
      return
    fi
    if ! managed_pid "${component}" >/dev/null 2>&1; then
      echo "$(name_for "${component}")启动进程已退出，日志：${log}" >&2
      tail -n 40 "${log}" >&2 || true
      return 1
    fi
    sleep 1
    elapsed=$((elapsed + 1))
  done
  echo "$(name_for "${component}")未在 ${START_TIMEOUT}s 内就绪，日志：${log}" >&2
  tail -n 40 "${log}" >&2 || true
  return 1
}

stop_component() {
  local component="$1" pid port elapsed
  pid="$(managed_pid "${component}" 2>/dev/null || true)"
  if uses_screen && screen_exists "${component}"; then
    screen -S "$(screen_name "${component}")" -X quit >/dev/null 2>&1 || true
  elif uses_launchd && launchd_exists "${component}"; then
    launchctl bootout "gui/$(id -u)/$(label_for "${component}")" >/dev/null
  elif [[ -n "${pid}" ]]; then
    kill "${pid}" 2>/dev/null || true
  else
    rm -f "$(pid_file "${component}")" "$(plist_file "${component}")"
    echo "$(name_for "${component}")：未运行"
    return
  fi

  port="$(port_for "${component}")"
  elapsed=0
  while [[ -n "$(port_pids "${port}")" ]] && (( elapsed < 10 )); do
    sleep 1
    elapsed=$((elapsed + 1))
  done
  if [[ -n "${pid}" ]] && kill -0 "${pid}" 2>/dev/null; then
    command="$(ps -p "${pid}" -o command= 2>/dev/null || true)"
    if [[ "${command}" == *"$(expected_command_for "${component}")"* ]]; then
      kill -KILL "${pid}" 2>/dev/null || true
    fi
  fi
  if [[ -n "$(port_pids "${port}")" ]]; then
    echo "端口 ${port} 仍被其他进程占用，未对非 dev-stack 进程执行停止" >&2
    return 1
  fi
  rm -f "$(pid_file "${component}")" "$(plist_file "${component}")"
  echo "$(name_for "${component}")：已停止，端口 ${port} 已释放"
}

status_component() {
  local component="$1" pid url port occupied
  url="$(health_url_for "${component}")"
  port="$(port_for "${component}")"
  if pid="$(managed_pid "${component}" 2>/dev/null)"; then
    if curl --fail --silent --max-time 2 "${url}" >/dev/null 2>&1; then
      echo "$(name_for "${component}")：运行正常 (PID ${pid}, 端口 ${port})"
    else
      echo "$(name_for "${component}")：进程运行中，尚未就绪 (PID ${pid}, 端口 ${port})"
    fi
    return
  fi
  occupied="$(port_pids "${port}")"
  if [[ -n "${occupied}" ]]; then
    echo "$(name_for "${component}")：未托管，但端口 ${port} 被 PID ${occupied//$'\n'/,} 占用"
  else
    echo "$(name_for "${component}")：未运行 (端口 ${port} 空闲)"
  fi
}

start_all() {
  local started=() component result index
  mkdir -p "${RUN_DIR}"
  for component in moss server web; do
    assert_port_available "${component}"
  done
  prepare_dependencies

  for component in moss server web; do
    result=0
    start_component "${component}" || result=$?
    if (( result == 0 )); then
      started+=("${component}")
    elif (( result != 10 )); then
      for ((index=${#started[@]} - 1; index >= 0; index--)); do
        stop_component "${started[index]}" || true
      done
      return "${result}"
    fi
    if ! wait_healthy "${component}"; then
      for ((index=${#started[@]} - 1; index >= 0; index--)); do
        stop_component "${started[index]}" || true
      done
      return 1
    fi
  done

  echo
  echo "Voice Elf 开发环境已启动"
  echo "  Web：    http://127.0.0.1:${WEB_PORT}"
  echo "  Server： http://127.0.0.1:${SERVER_PORT}"
  echo "  MOSS：   http://127.0.0.1:${MOSS_PORT}"
  echo "  日志：   ${RUN_DIR}"
}

stop_all() {
  local failed=0 component
  for component in web server moss; do
    stop_component "${component}" || failed=1
  done
  return "${failed}"
}

status_all() {
  status_component moss
  status_component server
  status_component web
  check_postgres || true
}

show_logs() {
  local component="${1:-}"
  if [[ -n "${component}" ]]; then
    case "${component}" in moss|server|web) ;; *) usage >&2; return 2 ;; esac
    tail -n 120 -f "$(log_file "${component}")"
  else
    for component in moss server web; do
      echo "===== $(name_for "${component}") ====="
      tail -n 40 "$(log_file "${component}")" 2>/dev/null || true
    done
  fi
}

action="${1:-start}"
case "${action}" in
  start) start_all ;;
  stop) stop_all ;;
  restart) stop_all; start_all ;;
  status) status_all ;;
  logs) show_logs "${2:-}" ;;
  _run) run_component "${2:-}" ;;
  _run_logged)
    mkdir -p "${RUN_DIR}"
    exec >> "$(log_file "${2:-}")" 2>&1
    run_component "${2:-}"
    ;;
  -h|--help|help) usage ;;
  *) usage >&2; exit 2 ;;
esac
