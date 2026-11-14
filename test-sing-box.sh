#!/bin/bash
set -euo pipefail

interface=${1:-en0}
control_proxy=${2:-}
listen_port=17890

if ! [[ "$interface" =~ ^[[:alnum:]]+$ ]] || ! ifconfig "$interface" >/dev/null 2>&1; then
  printf 'Сетевой интерфейс не найден: %s\n' "$interface" >&2
  exit 1
fi
if ! command -v sing-box >/dev/null; then
  printf 'Команда sing-box отсутствует в PATH.\n' >&2
  exit 1
fi

test_dir=$(mktemp -d "${TMPDIR:-/tmp}/sing-box-test.XXXXXX")
config="$test_dir/config.json"
log="$test_dir/sing-box.log"
proxy_pid=

cleanup() {
  if [[ -n "$proxy_pid" ]]; then
    kill "$proxy_pid" 2>/dev/null || true
    wait "$proxy_pid" 2>/dev/null || true
  fi
  rm -rf -- "$test_dir"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

cat > "$config" <<EOF
{
  "log": {"level": "debug"},
  "dns": {"servers": [{"type": "local", "tag": "system-dns"}]},
  "inbounds": [{
    "type": "mixed",
    "listen": "127.0.0.1",
    "listen_port": $listen_port,
    "set_system_proxy": false
  }],
  "outbounds": [{"type": "direct", "tag": "physical"}],
  "route": {
    "default_interface": "$interface",
    "default_domain_resolver": "system-dns",
    "final": "physical"
  }
}
EOF

sing-box check -D "$test_dir" -c "$config"
sing-box run -D "$test_dir" -c "$config" > "$log" 2>&1 &
proxy_pid=$!

for ((attempt = 0; attempt < 40; attempt++)); do
  if grep -Fq "started at 127.0.0.1:$listen_port" "$log" || ! kill -0 "$proxy_pid" 2>/dev/null; then
    break
  fi
  sleep 0.1
done
if ! kill -0 "$proxy_pid" 2>/dev/null || ! grep -Fq "started at 127.0.0.1:$listen_port" "$log"; then
  cat "$log" >&2
  printf 'Локальный прокси не запустился.\n' >&2
  exit 1
fi

probe() {
  local label=$1 proxy=$2 url=$3 family=$4 ip timings status=0
  shift 4
  timings=$(curl --disable --noproxy '' --proxy "$proxy" --connect-timeout 8 --max-time 12 -fsS \
    --output "$test_dir/ip" --write-out 'CONNECT=%{http_connect}, TLS=%{time_appconnect}s' "$@" "$url") || status=$?
  if ((status != 0)); then
    printf '%s: недоступно (curl=%s, %s)\n' "$label" "$status" "$timings"
    return 1
  fi
  ip=$(< "$test_dir/ip")
  if [[ "$family" = 4 && "$ip" =~ ^[0-9.]+$ && "$ip" == *.* ]] ||
     [[ "$family" = 6 && "$ip" =~ ^[0-9a-fA-F:]+$ && "$ip" == *:* ]]; then
    printf '%s: %s\n' "$label" "$ip"
  else
    printf '%s: ответ без IPv%s\n' "$label" "$family"
    return 1
  fi
}

proxy_address="127.0.0.1:$listen_port"
local_proxy="http://$proxy_address"
ipv4_url=https://api.ipify.org
ipv6_url=https://api6.ipify.org
printf 'Исходящий интерфейс sing-box: %s\n' "$interface"
probe 'Обычный IPv4' "$control_proxy" "$ipv4_url" 4 || true
if ! probe 'Через sing-box IPv4' "$local_proxy" "$ipv4_url" 4; then
  printf '\nДополнительные проверки IPv4\n'
  if ! probe 'Через sing-box IPv4, AWS' "$local_proxy" https://checkip.amazonaws.com 4; then
    probe "curl через $interface, IPv4" '' "$ipv4_url" 4 --interface "if!$interface" --ipv4 || true
    probe 'SOCKS5, DNS на стороне sing-box' "socks5h://$proxy_address" "$ipv4_url" 4 || true
    probe 'SOCKS5, DNS на стороне curl' "socks5://$proxy_address" "$ipv4_url" 4 --ipv4 || true
  fi
  printf '\nЖурнал sing-box для IPv4\n'
  tail -n 24 "$log"
  printf '\n'
fi
probe 'Обычный IPv6' "$control_proxy" "$ipv6_url" 6 || true
probe 'Через sing-box IPv6' "$local_proxy" "$ipv6_url" 6 || true
