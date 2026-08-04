#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_DIR="${ROOT}/.local/run/public-tunnel"
WAIT_SECONDS="${PUBLIC_TUNNEL_WAIT_SECONDS:-45}"
HEALTH_WAIT_SECONDS="${PUBLIC_TUNNEL_HEALTH_WAIT_SECONDS:-15}"

usage() {
  cat <<'EOF'
用法：
  ./scripts/public-tunnel.sh start [production|dev|all]
  ./scripts/public-tunnel.sh stop [production|dev|all]
  ./scripts/public-tunnel.sh restart [production|dev|all]
  ./scripts/public-tunnel.sh status [production|dev|all]

默认操作：start production
EOF
}

find_cloudflared() {
  if [[ -n "${CLOUDFLARED:-}" && -x "${CLOUDFLARED}" ]]; then
    printf '%s\n' "${CLOUDFLARED}"
  elif command -v cloudflared >/dev/null 2>&1; then
    command -v cloudflared
  elif [[ -x /usr/local/opt/cloudflared/bin/cloudflared ]]; then
    printf '%s\n' /usr/local/opt/cloudflared/bin/cloudflared
  else
    echo "未找到 cloudflared，请先执行：brew install cloudflared" >&2
    return 1
  fi
}

normalize_target() {
  case "${1:-production}" in
    production|prod) printf '%s\n' production ;;
    dev|debug) printf '%s\n' dev ;;
    all) printf '%s\n' all ;;
    *) echo "未知环境：$1" >&2; usage >&2; return 1 ;;
  esac
}

origin_for() {
  [[ "$1" == production ]] && printf '%s\n' http://127.0.0.1:3001 || printf '%s\n' http://127.0.0.1:5173
}

label_for() {
  [[ "$1" == production ]] && printf '%s\n' "生产站点" || printf '%s\n' "Vite 调试站点"
}

pid_file() { printf '%s/%s.pid\n' "${RUN_DIR}" "$1"; }
url_file() { printf '%s/%s.url\n' "${RUN_DIR}" "$1"; }
log_file() { printf '%s/%s.log\n' "${RUN_DIR}" "$1"; }
plist_file() { printf '%s/%s.plist\n' "${RUN_DIR}" "$1"; }
launchd_label() { printf 'com.voice-elf.public-tunnel.%s\n' "$1"; }

uses_launchd() {
  [[ "$(uname -s)" == Darwin ]] && command -v launchctl >/dev/null 2>&1
}

launchd_service_exists() {
  launchctl print "gui/$(id -u)/$(launchd_label "$1")" >/dev/null 2>&1
}

launchd_pid() {
  launchctl print "gui/$(id -u)/$(launchd_label "$1")" 2>/dev/null \
    | awk '/^[[:space:]]*pid = [0-9]+/{print $3; exit}'
}

write_launchd_plist() {
  local target="$1" binary="$2" origin="$3" log="$4" plist label host_header
  plist="$(plist_file "${target}")"
  label="$(launchd_label "${target}")"
  host_header=""
  if [[ "${target}" == dev ]]; then
    host_header=$'      <string>--http-host-header</string>\n      <string>localhost</string>'
  fi
  cat > "${plist}" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>${label}</string>
    <key>ProgramArguments</key>
    <array>
      <string>${binary}</string>
      <string>tunnel</string>
      <string>--no-autoupdate</string>
      <string>--protocol</string>
      <string>http2</string>
      <string>--url</string>
      <string>${origin}</string>
${host_header}
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ThrottleInterval</key>
    <integer>5</integer>
    <key>StandardOutPath</key>
    <string>${log}</string>
    <key>StandardErrorPath</key>
    <string>${log}</string>
  </dict>
</plist>
EOF
  plutil -lint "${plist}" >/dev/null
}

is_running() {
  local target="$1" file pid command
  if uses_launchd; then
    launchd_service_exists "${target}" || return 1
    pid="$(launchd_pid "${target}")"
    [[ "${pid}" =~ ^[0-9]+$ ]] || return 1
    printf '%s\n' "${pid}" > "$(pid_file "${target}")"
    kill -0 "${pid}" 2>/dev/null
    return
  fi
  file="$(pid_file "${target}")"
  [[ -f "${file}" ]] || return 1
  pid="$(tr -d '[:space:]' < "${file}")"
  [[ "${pid}" =~ ^[0-9]+$ ]] || return 1
  kill -0 "${pid}" 2>/dev/null || return 1
  command="$(ps -p "${pid}" -o command= 2>/dev/null || true)"
  [[ "${command}" == *"cloudflared tunnel"* ]]
}

read_public_url() {
  local target="$1" file log
  file="$(url_file "${target}")"
  log="$(log_file "${target}")"
  if [[ -s "${log}" ]]; then
    grep -Eo 'https://[a-z0-9-]+\.trycloudflare\.com' "${log}" | tail -n 1 || true
  elif [[ -s "${file}" ]]; then
    tr -d '[:space:]' < "${file}"
  fi
}

status_one() {
  local target="$1" label origin url pid
  label="$(label_for "${target}")"
  origin="$(origin_for "${target}")"
  if is_running "${target}"; then
    pid="$(tr -d '[:space:]' < "$(pid_file "${target}")")"
    url="$(read_public_url "${target}")"
    [[ -n "${url}" ]] && printf '%s\n' "${url}" > "$(url_file "${target}")"
    echo "${label}：运行中 (PID ${pid})"
    echo "  本地：${origin}"
    [[ -n "${url}" ]] && echo "  外网：${url}" || echo "  外网：正在获取地址"
  else
    echo "${label}：未运行"
    rm -f "$(pid_file "${target}")" "$(url_file "${target}")"
  fi
}

start_one() {
  local target="$1" label origin binary log pid url elapsed health_elapsed verified
  label="$(label_for "${target}")"
  origin="$(origin_for "${target}")"
  binary="$(find_cloudflared)"
  log="$(log_file "${target}")"

  if is_running "${target}"; then
    status_one "${target}"
    return
  fi
  if ! curl --fail --silent --show-error --max-time 3 "${origin}/api/health" >/dev/null; then
    echo "${label}尚未启动：${origin}" >&2
    [[ "${target}" == production ]] \
      && echo "请先启动后端服务。" >&2 \
      || echo "请先在 web 目录执行 npm run dev。" >&2
    return 1
  fi

  rm -f "$(pid_file "${target}")" "$(url_file "${target}")" "${log}"
  echo "正在创建${label}外网隧道..."
  if uses_launchd; then
    write_launchd_plist "${target}" "${binary}" "${origin}" "${log}"
    launchctl bootout "gui/$(id -u)/$(launchd_label "${target}")" >/dev/null 2>&1 || true
    launchctl bootstrap "gui/$(id -u)" "$(plist_file "${target}")"
    pid=""
  elif [[ "${target}" == production ]]; then
    nohup "${binary}" tunnel --no-autoupdate --protocol http2 --url "${origin}" >"${log}" 2>&1 < /dev/null &
    pid=$!
    printf '%s\n' "${pid}" > "$(pid_file "${target}")"
  else
    nohup "${binary}" tunnel --no-autoupdate --protocol http2 --url "${origin}" \
      --http-host-header localhost >"${log}" 2>&1 < /dev/null &
    pid=$!
    printf '%s\n' "${pid}" > "$(pid_file "${target}")"
  fi

  elapsed=0
  while (( elapsed < WAIT_SECONDS )); do
    if ! is_running "${target}"; then
      echo "${label}隧道启动失败，日志：${log}" >&2
      tail -n 20 "${log}" >&2 || true
      rm -f "$(pid_file "${target}")"
      return 1
    fi
    url="$(read_public_url "${target}")"
    if [[ -n "${url}" ]] && grep -q 'Registered tunnel connection' "${log}"; then
      break
    fi
    sleep 1
    elapsed=$((elapsed + 1))
  done

  if [[ -z "${url}" ]] || ! grep -q 'Registered tunnel connection' "${log}"; then
    echo "${label}未能在 ${WAIT_SECONDS}s 内完成隧道注册。" >&2
    echo "  查看日志：${log}" >&2
    return 1
  fi

  printf '%s\n' "${url}" > "$(url_file "${target}")"
  verified=false
  health_elapsed=0
  while (( health_elapsed < HEALTH_WAIT_SECONDS )); do
    if curl --fail --silent --max-time 5 "${url}/api/health" >/dev/null 2>&1; then
      verified=true
      break
    fi
    sleep 1
    health_elapsed=$((health_elapsed + 1))
  done

  echo "${label}部署完成"
  echo "  外网地址：${url}"
  echo "  本地地址：${origin}"
  echo "  日志文件：${log}"
  if [[ "${verified}" == true ]]; then
    echo "  外网检查：通过"
  else
    echo "  外网检查：本机 DNS 尚未生效，请稍后打开上方地址"
  fi
}

stop_one() {
  local target="$1" label pid elapsed
  label="$(label_for "${target}")"
  if uses_launchd && launchd_service_exists "${target}"; then
    pid="$(launchd_pid "${target}")"
    launchctl bootout "gui/$(id -u)/$(launchd_label "${target}")"
    elapsed=0
    while (( elapsed < 10 )); do
      if ! launchd_service_exists "${target}" \
        && { [[ ! "${pid}" =~ ^[0-9]+$ ]] || ! kill -0 "${pid}" 2>/dev/null; }; then
        break
      fi
      sleep 1
      elapsed=$((elapsed + 1))
    done
    if launchd_service_exists "${target}" \
      || { [[ "${pid}" =~ ^[0-9]+$ ]] && kill -0 "${pid}" 2>/dev/null; }; then
      echo "${label}隧道未在 10s 内退出" >&2
      return 1
    fi
    rm -f "$(pid_file "${target}")" "$(url_file "${target}")" "$(plist_file "${target}")"
    echo "${label}隧道已停止"
    return
  fi
  if ! is_running "${target}"; then
    echo "${label}：未运行"
    rm -f "$(pid_file "${target}")" "$(url_file "${target}")"
    return
  fi
  pid="$(tr -d '[:space:]' < "$(pid_file "${target}")")"
  kill "${pid}"
  elapsed=0
  while kill -0 "${pid}" 2>/dev/null && (( elapsed < 10 )); do
    sleep 1
    elapsed=$((elapsed + 1))
  done
  if kill -0 "${pid}" 2>/dev/null; then
    echo "${label}未在 10s 内退出，请检查 PID ${pid}" >&2
    return 1
  fi
  rm -f "$(pid_file "${target}")" "$(url_file "${target}")"
  echo "${label}隧道已停止"
}

run_for_target() {
  local action="$1" target="$2"
  if [[ "${target}" == all ]]; then
    "${action}_one" production
    "${action}_one" dev
  else
    "${action}_one" "${target}"
  fi
}

mkdir -p "${RUN_DIR}"
ACTION="${1:-start}"
TARGET="$(normalize_target "${2:-production}")"

case "${ACTION}" in
  start|stop|status)
    run_for_target "${ACTION}" "${TARGET}"
    ;;
  restart)
    run_for_target stop "${TARGET}"
    run_for_target start "${TARGET}"
    ;;
  help|-h|--help)
    usage
    ;;
  *)
    echo "未知操作：${ACTION}" >&2
    usage >&2
    exit 1
    ;;
esac
