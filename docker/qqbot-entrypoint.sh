#!/bin/sh
set -eu

napcat_host="${NAPCAT_HOST:-host.docker.internal}"
napcat_http_port="${NAPCAT_HOST_HTTP_PORT:-3001}"
napcat_ws_port="${NAPCAT_HOST_WS_PORT:-6701}"

socat TCP-LISTEN:3000,bind=127.0.0.1,reuseaddr,fork TCP:"${napcat_host}":"${napcat_http_port}" &
http_proxy_pid=$!
socat TCP-LISTEN:6700,bind=127.0.0.1,reuseaddr,fork TCP:"${napcat_host}":"${napcat_ws_port}" &
ws_proxy_pid=$!

cleanup() {
    kill "${http_proxy_pid}" "${ws_proxy_pid}" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

exec qqbot-server
