#!/usr/bin/env bash
set -euo pipefail

# Isolated QA sandbox for MQ Launcher.
# Sets MQ_DATA_DIR=/tmp/mq-qa/<run>, shares asset cache, never touches ~/.local/share/mq,
# and never reads or prints ~/.config/mq/dev.env.

KEEP=0
PRINT_ENV=0
SHARED_ASSETS=1
RUN_ID=""

usage() {
    cat <<'EOF'
Usage: sandbox.sh [OPTIONS] [-- COMMAND...]

Options:
  --keep              Keep MQ_DATA_DIR after exit for debugging
  --run <id>          Custom run identifier (default: run-<timestamp>-<pid>)
  --no-shared-assets  Do not symlink /tmp/mq-qa/shared-assets into MQ_DATA_DIR/assets
  --print-env         Create sandbox, print export MQ_DATA_DIR=..., and exit (implies --keep)
  -h, --help          Show this help

Example:
  scripts/qa/sandbox.sh -- cargo run --bin mq -- instances list
  scripts/qa/sandbox.sh --keep --run smoke1 -- ./target/debug/mq doctor
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --keep)
            KEEP=1
            shift
            ;;
        --print-env)
            PRINT_ENV=1
            KEEP=1
            shift
            ;;
        --no-shared-assets)
            SHARED_ASSETS=0
            shift
            ;;
        --run)
            RUN_ID="${2:-}"
            if [[ -z "${RUN_ID}" ]]; then
                echo "ERROR: --run requires a value" >&2
                exit 2
            fi
            shift 2
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

if [[ -z "${RUN_ID}" ]]; then
    RUN_ID="run-$(date +%Y%m%d-%H%M%S)-$$"
fi

# Sanitize RUN_ID to prevent path traversal
if [[ "${RUN_ID}" == *"/"* || "${RUN_ID}" == *".."* ]]; then
    echo "ERROR: invalid --run id '${RUN_ID}'" >&2
    exit 2
fi

QA_ROOT="/tmp/mq-qa"
SANDBOX_DIR="${QA_ROOT}/${RUN_ID}"
SHARED_ASSETS_DIR="${QA_ROOT}/shared-assets"
REAL_USER_DATA="${HOME}/.local/share/mq"

# Hard safety guard: never touch ~/.local/share/mq
if [[ "${SANDBOX_DIR}" == "${REAL_USER_DATA}"* ]]; then
    echo "ERROR: Refusing to use real user data dir: ${SANDBOX_DIR}" >&2
    exit 1
fi

mkdir -p "${SANDBOX_DIR}"
chmod 700 "${SANDBOX_DIR}"

if [[ "${SHARED_ASSETS}" -eq 1 ]]; then
    mkdir -p "${SHARED_ASSETS_DIR}"
    if [[ ! -e "${SANDBOX_DIR}/assets" ]]; then
        ln -s "${SHARED_ASSETS_DIR}" "${SANDBOX_DIR}/assets"
    fi
fi

export MQ_DATA_DIR="${SANDBOX_DIR}"

cleanup() {
    local rc=$?
    if [[ "${KEEP}" -eq 0 && -d "${SANDBOX_DIR}" && "${SANDBOX_DIR}" == /tmp/mq-qa/* ]]; then
        rm -rf "${SANDBOX_DIR}"
    elif [[ "${KEEP}" -eq 1 && "${PRINT_ENV}" -eq 0 ]]; then
        echo "[sandbox] Kept MQ_DATA_DIR=${SANDBOX_DIR}" >&2
    fi
    return "${rc}"
}
trap cleanup EXIT INT TERM

if [[ "${PRINT_ENV}" -eq 1 ]]; then
    printf 'export MQ_DATA_DIR=%q\n' "${SANDBOX_DIR}"
    exit 0
fi

if [[ $# -eq 0 ]]; then
    echo "MQ_DATA_DIR=${SANDBOX_DIR}"
    exit 0
fi

"$@"
