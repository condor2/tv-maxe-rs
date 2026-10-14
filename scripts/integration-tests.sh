#!/usr/bin/env bash
# Testele de integrare (libmpv + GTK) pe un ecran virtual, ca în CI.
#
#   scripts/integration-tests.sh x11         # Xvfb
#   scripts/integration-tests.sh wayland     # weston --backend=headless
#   scripts/integration-tests.sh x11 antete  # doar testele care conțin „antete”
#
# Necesită: xvfb (xvfb-run) pentru x11, weston pentru wayland.
set -euo pipefail

mode=${1:-x11}
shift || true

export TVMAXE_REQUIRE_DISPLAY=1
# OpenGL software (llvmpipe), identic pe orice mașină; fără încercări Vulkan/zink.
export LIBGL_ALWAYS_SOFTWARE=1
export GALLIUM_DRIVER=llvmpipe

cmd=(cargo test -p tvmaxe --test player --test ui -- "$@")

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
        echo "weston nu a pornit:" >&2
        cat "$runtime/weston.log" >&2
        exit 1
    fi
    unset DISPLAY
    WAYLAND_DISPLAY=$socket GDK_BACKEND=wayland "${cmd[@]}"
    ;;
*)
    echo "folosire: $0 x11|wayland [filtru…]" >&2
    exit 2
    ;;
esac
