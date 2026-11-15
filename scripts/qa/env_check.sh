#!/usr/bin/env bash
set -uo pipefail

STRICT=0
if [[ "${1:-}" == "--strict" ]]; then
    STRICT=1
fi

MIN_DISK_GB=15
DEV_ENV_PATH="${HOME}/.config/mq/dev.env"
USER_TESSDATA="${HOME}/.local/share/tessdata"

# Ensure user-space bins are in PATH
export PATH="${HOME}/.cargo/bin:${HOME}/.local/bin:${PATH}"

MISSING_COUNT=0

check_bin() {
    local label="$1"
    local bin="$2"
    local note="${3:-}"
    if command -v "${bin}" >/dev/null 2>&1; then
        local loc
        loc="$(command -v "${bin}")"
        printf "%-22s | %-10s | %s\n" "${label}" "FOUND" "${loc}${note:+ (${note})}"
    else
        printf "%-22s | %-10s | %s\n" "${label}" "MISSING" "${note:-not in PATH}"
        MISSING_COUNT=$((MISSING_COUNT + 1))
    fi
}

check_python_mod() {
    local label="$1"
    local mod="$2"
    if command -v python3 >/dev/null 2>&1 && python3 -c "import ${mod}" >/dev/null 2>&1; then
        local ver
        ver="$(python3 -c "import ${mod}; print(getattr(${mod}, '__version__', 'ok'))" 2>/dev/null || echo "ok")"
        printf "%-22s | %-10s | %s\n" "${label}" "FOUND" "python3 module (${ver})"
    else
        printf "%-22s | %-10s | %s\n" "${label}" "MISSING" "pip install --user ${mod}"
        MISSING_COUNT=$((MISSING_COUNT + 1))
    fi
}

check_tess_lang() {
    local label="$1"
    local lang="$2"
    local prefix=""
    if tesseract --list-langs 2>/dev/null | grep -qx "${lang}"; then
        printf "%-22s | %-10s | %s\n" "${label}" "FOUND" "system tessdata"
        return 0
    fi
    if [[ -d "${USER_TESSDATA}" ]] && TESSDATA_PREFIX="${USER_TESSDATA}" tesseract --list-langs 2>/dev/null | grep -qx "${lang}"; then
        printf "%-22s | %-10s | %s\n" "${label}" "FOUND" "TESSDATA_PREFIX=${USER_TESSDATA}"
        return 0
    fi
    printf "%-22s | %-10s | %s\n" "${label}" "MISSING" "lang=${lang}"
    MISSING_COUNT=$((MISSING_COUNT + 1))
}

echo "================================================================================"
echo "                         MQ QA ENVIRONMENT CHECK                                "
echo "================================================================================"
printf "%-22s | %-10s | %s\n" "Tool / Resource" "Status" "Details"
echo "-----------------------+------------+-------------------------------------------"

check_bin "Xvfb" "Xvfb" "pkg: xorg-server-xvfb"
check_bin "xdotool" "xdotool" "pkg: xdotool"
check_bin "convert" "convert" "pkg: imagemagick"
check_bin "import" "import" "pkg: imagemagick"
check_bin "montage" "montage" "pkg: imagemagick"
check_bin "tesseract" "tesseract" "pkg: tesseract"
check_tess_lang "tesseract-eng" "eng"
check_tess_lang "tesseract-rus" "rus"
check_bin "python3" "python3"
check_python_mod "python-pillow" "PIL"
check_python_mod "python-numpy" "numpy"
check_bin "glxinfo" "glxinfo" "pkg: mesa-utils"
check_bin "jq" "jq"
check_bin "sccache" "sccache" "cargo install sccache"
check_bin "Xwayland" "Xwayland" "optional fallback"

echo "-----------------------+------------+-------------------------------------------"

# Disk space check (>= 15 GB on /home or /)
CHECK_PATH="${HOME}"
if [[ ! -d "${CHECK_PATH}" ]]; then
    CHECK_PATH="/"
fi
AVAIL_KB="$(df -Pk "${CHECK_PATH}" | awk 'NR==2 {print $4}')"
AVAIL_GB=$((AVAIL_KB / 1024 / 1024))
DISK_STATUS="PASS"
if [[ "${AVAIL_GB}" -lt "${MIN_DISK_GB}" ]]; then
    DISK_STATUS="FAIL"
fi
printf "%-22s | %-10s | %s\n" "Disk (${CHECK_PATH})" "${DISK_STATUS}" "${AVAIL_GB} GB available (min ${MIN_DISK_GB} GB)"

# dev.env check (NEVER print contents!)
if [[ -f "${DEV_ENV_PATH}" ]]; then
    PERMS="$(stat -c '%a' "${DEV_ENV_PATH}" 2>/dev/null || echo '?')"
    printf "%-22s | %-10s | %s\n" "dev.env" "FOUND" "${DEV_ENV_PATH} (mode ${PERMS}, contents hidden)"
else
    printf "%-22s | %-10s | %s\n" "dev.env" "MISSING" "${DEV_ENV_PATH} not found"
fi

echo "================================================================================"

if [[ "${DISK_STATUS}" != "PASS" ]]; then
    echo "ERROR: Insufficient disk space (${AVAIL_GB} GB < ${MIN_DISK_GB} GB)." >&2
    exit 1
fi

if [[ "${STRICT}" -eq 1 && "${MISSING_COUNT}" -gt 0 ]]; then
    echo "ERROR: ${MISSING_COUNT} required tool(s) missing (--strict)." >&2
    exit 1
fi

exit 0
