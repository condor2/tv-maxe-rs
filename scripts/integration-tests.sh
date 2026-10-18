#!/usr/bin/env bash
# Integration tests (libmpv + GTK) on a virtual display, like in CI.
#
#   scripts/integration-tests.sh x11          # Xvfb
#   scripts/integration-tests.sh wayland      # weston --backend=headless
#   scripts/integration-tests.sh x11 headers  # only tests whose name contains "headers"
#
# Requires: xvfb (xvfb-run) for x11, weston for wayland, dbus-daemon (MPRIS tests).
set -euo pipefail

mode=${1:-x11}
shift || true

export TVMAXE_REQUIRE_DISPLAY=1
# Software OpenGL (llvmpipe), identical on every machine; no Vulkan/zink attempts.
export LIBGL_ALWAYS_SOFTWARE=1
export GALLIUM_DRIVER=llvmpipe

cmd=(cargo test -p tvmaxe --test player --test ui --test mpris -- "$@")

case "$mode" in
x11)
    unset WAYLAND_DISPLAY
    GDK_BACKEND=x11 xvfb-run -a -s "-screen 0 1280x1024x24" "${cmd[@]}"
    ;;
wayland)
    runtime=$(mktemp -d)
    chmod 700 "$runtime"
    export XDG_RUNTIME_DIR=$runtime
    socket=wayland-tvmaxe-test
    weston --backend=headless --socket="$socket" --idle-time=0 >"$runtime/weston.log" 2>&1 &
    weston_pid=$!
    trap 'kill $weston_pid 2>/dev/null || true; rm -rf "$runtime"' EXIT
    for _ in $(seq 100); do
        [[ -S $runtime/$socket ]] && break
        sleep 0.1
    done
    if [[ ! -S $runtime/$socket ]]; then
        echo "weston did not start:" >&2
        cat "$runtime/weston.log" >&2
        exit 1
    fi
    unset DISPLAY
    WAYLAND_DISPLAY=$socket GDK_BACKEND=wayland "${cmd[@]}"
    ;;
*)
    echo "usage: $0 x11|wayland [filter…]" >&2
    exit 2
    ;;
esac
