#!/usr/bin/env bash
# ==============================================================================
#  MQ Launcher — Portable Windows Archive Builder
#  Produces: dist/mq-windows-x86_64-portable.zip (flat root layout)
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

DIST_DIR="${REPO_ROOT}/dist"
PACKAGE_NAME="mq-windows-x86_64-portable"
ARCHIVE_PATH="${DIST_DIR}/${PACKAGE_NAME}.zip"

echo "=== Building MQ Windows Portable Archive ==="

# Resolve binary path
EXE_CANDIDATE="${1:-}"
if [[ -z "${EXE_CANDIDATE}" ]]; then
    if [[ -f "${REPO_ROOT}/target/x86_64-pc-windows-msvc/release/mq.exe" ]]; then
        EXE_CANDIDATE="${REPO_ROOT}/target/x86_64-pc-windows-msvc/release/mq.exe"
    elif [[ -f "${REPO_ROOT}/target/x86_64-pc-windows-gnu/release/mq.exe" ]]; then
        EXE_CANDIDATE="${REPO_ROOT}/target/x86_64-pc-windows-gnu/release/mq.exe"
    elif [[ -f "${REPO_ROOT}/target/release/mq.exe" ]]; then
        EXE_CANDIDATE="${REPO_ROOT}/target/release/mq.exe"
    else
        echo "Error: mq.exe not found in target directories. Please build it first:"
        echo "  cargo build --release --target x86_64-pc-windows-msvc"
        echo "Or specify path as argument: $0 /path/to/mq.exe"
        exit 1
    fi
fi

echo "Using binary: ${EXE_CANDIDATE}"
mkdir -p "${DIST_DIR}"

STAGE_DIR="$(mktemp -d)"
cleanup() {
    rm -rf "${STAGE_DIR}"
}
trap cleanup EXIT

# Copy payload items flat at archive root (matches release.yml, Scoop, and Explorer Extract All)
cp "${EXE_CANDIDATE}" "${STAGE_DIR}/mq.exe"
cp "${SCRIPT_DIR}/portable.flag" "${STAGE_DIR}/portable.flag"
cp "${SCRIPT_DIR}/mq.ico" "${STAGE_DIR}/mq.ico"
cp "${SCRIPT_DIR}/run.bat" "${STAGE_DIR}/run.bat"
cp "${SCRIPT_DIR}/README-portable.txt" "${STAGE_DIR}/README.txt"
if [[ -f "${REPO_ROOT}/THIRD_PARTY_LICENSES.md" ]]; then
    cp "${REPO_ROOT}/THIRD_PARTY_LICENSES.md" "${STAGE_DIR}/THIRD_PARTY_LICENSES.md"
fi
if [[ -f "${REPO_ROOT}/LICENSE" ]]; then
    cp "${REPO_ROOT}/LICENSE" "${STAGE_DIR}/LICENSE"
fi
mkdir -p "${STAGE_DIR}/data"

# Build zip archive
rm -f "${ARCHIVE_PATH}"
(
    cd "${STAGE_DIR}"
    zip -r -9 "${ARCHIVE_PATH}" .
)

echo "Portable package created successfully:"
ls -lh "${ARCHIVE_PATH}"
if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${ARCHIVE_PATH}"
fi
