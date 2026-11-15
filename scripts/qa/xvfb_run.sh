#!/usr/bin/env bash
set -euo pipefail

# Headless X11 wrapper for MQ Launcher QA.
# Primary: Xvfb :99 -screen 0 1920x1080x24
# Fallback (when Xvfb is unavailable): weston --backend=headless-backend.so --xwayland
# Exports: DISPLAY, WINIT_UNIX_BACKEND=x11, LIBGL_ALWAYS_SOFTWARE=1, TESSDATA_PREFIX (if user tessdata exists).

DISPLAY_NUM="99"
SCREEN_SPEC="1920x1080x24"
SCREENSHOT_OUT=""
WAIT_SECS="2"
NO_FALLBACK=0

usage() {
    cat <<'EOF'
Usage: xvfb_run.sh [OPTIONS] [-- COMMAND...]

Options:
  --display <num>        X display number without colon (default: 99, auto-increments if busy)
  --screen <WxHxD>       Screen geometry (default: 1920x1080x24)
  --screenshot <path>    Capture root window with `import -window root <path>`
  --wait <secs>          Seconds to wait before taking screenshot (default: 2)
  --no-fallback          Fail if Xvfb is not installed (do not use weston+Xwayland)
  -h, --help             Show this help

Example:
  scripts/qa/xvfb_run.sh --screenshot /tmp/qa.png --wait 2 -- glxinfo -B
  scripts/qa/xvfb_run.sh -- ./target/debug/mq
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --display)
            DISPLAY_NUM="${2#:}"
            shift 2
            ;;
        --screen)
            SCREEN_SPEC="$2"
            shift 2
            ;;
        --screenshot)
            SCREENSHOT_OUT="$2"
            shift 2
            ;;
        --wait)
            WAIT_SECS="$2"
            shift 2
            ;;
        --no-fallback)
            NO_FALLBACK=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --)
            shift
            break
            ;;
        *)
            break
            ;;
    esac
done

SERVER_PID=""
SERVER_LOG=""

cleanup() {
    local rc=$?
    if [[ -n "${SERVER_PID}" ]] && kill -0 "${SERVER_PID}" 2>/dev/null; then
        kill "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
    if [[ -n "${SERVER_LOG}" && -f "${SERVER_LOG}" ]]; then
        rm -f "${SERVER_LOG}"
    fi
    return "${rc}"
}
trap cleanup EXIT INT TERM

if command -v Xvfb >/dev/null 2>&1; then
    while [[ -e "/tmp/.X${DISPLAY_NUM}-lock" || -e "/tmp/.X11-unix/X${DISPLAY_NUM}" ]]; do
        DISPLAY_NUM=$((DISPLAY_NUM + 1))
        if [[ "${DISPLAY_NUM}" -gt 150 ]]; then
            echo "ERROR: No free X display found between :99 and :150" >&2
            exit 1
        fi
    done

    SERVER_LOG="$(mktemp -t mq-xvfb-XXXXXX.log)"
    Xvfb ":${DISPLAY_NUM}" -screen 0 "${SCREEN_SPEC}" -nolisten tcp >"${SERVER_LOG}" 2>&1 &
    SERVER_PID=$!

    for _ in $(seq 1 30); do
        if [[ -e "/tmp/.X11-unix/X${DISPLAY_NUM}" ]]; then
            break
        fi
        if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
            echo "ERROR: Xvfb failed to start:" >&2
            cat "${SERVER_LOG}" >&2
            exit 1
        fi
        sleep 0.1
    done
    export DISPLAY=":${DISPLAY_NUM}"
elif [[ "${NO_FALLBACK}" -eq 0 ]] && command -v weston >/dev/null 2>&1 && command -v Xwayland >/dev/null 2>&1; then
    WIDTH="${SCREEN_SPEC%%x*}"
    REST="${SCREEN_SPEC#*x}"
    HEIGHT="${REST%%x*}"
    SERVER_LOG="$(mktemp -t mq-weston-XXXXXX.log)"
    weston --backend=headless-backend.so --xwayland --width="${WIDTH}" --height="${HEIGHT}" --socket="wayland-mq-qa-$$" >"${SERVER_LOG}" 2>&1 &
    SERVER_PID=$!

    DETECTED_DISPLAY=""
    for _ in $(seq 1 50); do
        if grep -q "xserver listening on display " "${SERVER_LOG}" 2>/dev/null; then
            DETECTED_DISPLAY="$(sed -n 's/.*xserver listening on display \(:[0-9]\+\).*/\1/p' "${SERVER_LOG}" | head -n 1)"
            if [[ -n "${DETECTED_DISPLAY}" ]]; then
                break
            fi
        fi
        if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
            echo "ERROR: weston headless+xwayland failed to start:" >&2
            cat "${SERVER_LOG}" >&2
            exit 1
        fi
        sleep 0.1
    done
    if [[ -z "${DETECTED_DISPLAY}" ]]; then
        echo "ERROR: Timed out waiting for weston Xwayland display" >&2
        cat "${SERVER_LOG}" >&2
        exit 1
    fi
    export DISPLAY="${DETECTED_DISPLAY}"
else
    echo "ERROR: Xvfb is not installed (install xorg-server-xvfb) and no weston+Xwayland fallback available." >&2
    exit 1
fi

export WINIT_UNIX_BACKEND="x11"
export LIBGL_ALWAYS_SOFTWARE="1"
if [[ -d "${HOME}/.local/share/tessdata" && -z "${TESSDATA_PREFIX:-}" ]]; then
    export TESSDATA_PREFIX="${HOME}/.local/share/tessdata"
fi

if [[ -n "${SCREENSHOT_OUT}" ]]; then
    mkdir -p "$(dirname "${SCREENSHOT_OUT}")"
    if [[ $# -eq 0 ]]; then
        sleep "${WAIT_SECS}"
        import -window root "${SCREENSHOT_OUT}"
        exit 0
    fi

    "$@" &
    CMD_PID=$!
    sleep "${WAIT_SECS}"
    import -window root "${SCREENSHOT_OUT}"
    if kill -0 "${CMD_PID}" 2>/dev/null; then
        kill "${CMD_PID}" 2>/dev/null || true
    fi
    wait "${CMD_PID}" 2>/dev/null || true
    exit 0
fi

if [[ $# -eq 0 ]]; then
    echo "DISPLAY=${DISPLAY}"
    exit 0
fi

"$@"
